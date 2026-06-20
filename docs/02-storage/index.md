# Slice 2 — 数据模型与存储层

> 上一片：[Slice 1 — 工程骨架与基础设施](../01-skeleton/index.md)

## 1. 目标与范围

把 backend 内部的存储层从零搭起：完整 PostgreSQL schema、迁移文件、扩展启用、`db` 模块骨架、本地开发脚手架（PG + Meilisearch）。**只产出结构，不写函数体**（Repository 函数体在 Slice 4 backend 骨架接入时填）。

**范围内**：

- 14 张表的 schema（含 `dht_observations` / `agent_metrics_history` / `fetch_failures` 三张分区表 + 两张聚合表）。
- 索引策略（BRIN 时序、B-tree 复合、部分索引）。**不**建任何 pg_trgm GIN 索引 —— 搜索全部外置 Meilisearch。
- sqlx 内置 migrate：`migrations/` 目录组织、文件命名约定、forward-only。
- v2/hybrid 扩展位：`hash_kind` + `hybrid_group_id`，v1-only 阶段占位不启用。
- backend `db` 模块骨架（Repository 风格签名，函数体 `todo!()`）。
- 本地开发脚手架：`docker-compose.yml`（PG 16 + Meilisearch）+ `scripts/db-up.sh` / `db-up.ps1`。
- 第一次 `cargo sqlx prepare` 跑通 + `.sqlx/` 入库。
- 每个业务字段都有 `COMMENT ON COLUMN`（自解释字段如 `id` / `*_at` / `created_at` 例外）。

**范围外**：实际的 INSERT/SELECT 逻辑（Slice 4）、ingest pipeline（Slice 9）、Meilisearch 同步管道与查询路径（Slice 10）、聚合 job 实现（Slice 11/13）、地域字段填充（Slice 13）。

## 2. 整体表结构总览

```mermaid
erDiagram
    agents ||--o{ torrents : "discovered by"
    agents ||--o{ torrent_peers : "observed by"
    agents ||--o{ dht_observations : "observer"
    agents ||--o{ malicious_events : "reporter"
    agents ||--o{ task_queue : "target"
    agents ||--o{ agent_metrics_history : "metrics of"
    agents ||--o{ agent_metrics_daily : "rollup of"
    agents ||--o{ fetch_failures : "from"

    torrents ||--o{ torrent_peers : "swarm"
    torrents ||--o{ malware_samples : "classified as"
    torrents ||--o{ torrent_trackers : "announced via"
    torrents ||--o{ fetch_failures : "fetch err"

    peers ||--o{ torrent_peers : "in swarm"
    peers ||--o{ malicious_events : "evidence"

    trackers ||--o{ torrent_trackers : "linked"

    dht_nodes ||--o{ dht_observations : "observed"
```

存储分层：

```mermaid
flowchart LR
    PG[(PostgreSQL<br/>source of truth)]
    MS[(Meilisearch<br/>派生搜索索引)]
    PG -->|Slice 10 起<br/>异步同步管道| MS
```

PG 是唯一权威数据源。Meilisearch 是衍生索引，可随时全量重建，故障不影响主路径。

## 3. 类型与字段公约

| 维度 | 选择 | 理由 |
| --- | --- | --- |
| 主键 infohash | `text` (40 字符 lowercase hex；v2 后兼容 64 字符) | 与 Slice 1 sqlx 桥接对齐；可读；未来 v2 64 字符同列容纳 |
| IP | `inet` | 原生 CIDR 运算；v4/v6 同列 |
| 时间 | `timestamptz` | 与 sqlx `time` feature 对齐；UTC 内存模型 |
| ID（agent/task） | `uuid` | 全局唯一；与 `AgentId`/`TaskId` newtype 对齐 |
| 灵活字段 | `jsonb` | 索引可建；用于 evidence、capabilities、metrics 快照、files 列表 |
| 软删除 | **不用** | 嗅探数据是观测事实，不删；过期靠分区 + 清理作业 |
| 计数器 | `int` | 范围足够；不优化为 smallint |
| 外键 ON DELETE | `CASCADE`（torrents → 子表）；agent 引用 `SET NULL` | agent 可下线，观测事实保留；种子-peer 关联随种子消亡 |
| 字段注释 | 业务字段一律 `COMMENT ON COLUMN`；自解释字段（`id`/`*_at`/`created_at`/`first_seen`/`last_seen`）例外 | 让 schema 自描述 |

## 4. 表 schema（按主题分组）

> 完整 SQL 在 `migrations/` 文件里。本节正文展示建表语句精简版 + 索引；`COMMENT ON COLUMN` 单独集中在 4.x 节末尾，避免淹没结构。

### 4.1 种子主表

```sql
CREATE TABLE torrents (
    infohash         text         PRIMARY KEY,
    hash_kind        text         NOT NULL DEFAULT 'sha1'
                     CHECK (hash_kind IN ('sha1')),
    hybrid_group_id  uuid         NULL,
    name             text,
    encoding         text,
    is_private       boolean      NOT NULL DEFAULT false,
    size_bytes       bigint,
    file_count       int,
    files_json       jsonb,
    discovery_method text         NOT NULL,
    source_agent_id  uuid         REFERENCES agents(agent_id) ON DELETE SET NULL,
    first_found_at   timestamptz  NOT NULL DEFAULT now(),
    last_seen_at     timestamptz  NOT NULL DEFAULT now(),
    found_count      int          NOT NULL DEFAULT 1,
    is_fake          boolean      NOT NULL DEFAULT false,
    malware_flag     boolean      NOT NULL DEFAULT false
);

CREATE INDEX torrents_first_found    ON torrents (first_found_at DESC);
CREATE INDEX torrents_size           ON torrents (size_bytes);
CREATE INDEX torrents_fake           ON torrents (last_seen_at DESC) WHERE is_fake;
CREATE INDEX torrents_malware        ON torrents (last_seen_at DESC) WHERE malware_flag;
CREATE INDEX torrents_hybrid_group   ON torrents (hybrid_group_id) WHERE hybrid_group_id IS NOT NULL;
CREATE INDEX torrents_private        ON torrents (last_seen_at DESC) WHERE is_private;
```

注释（业务字段）：

```sql
COMMENT ON COLUMN torrents.infohash         IS 'BT v1: 40 字符 lowercase sha1 hex; 未来 v2: 64 字符 sha256 hex';
COMMENT ON COLUMN torrents.hash_kind        IS '当前仅 sha1 (v1); 未来通过放宽 CHECK 约束支持 sha256 (v2)';
COMMENT ON COLUMN torrents.hybrid_group_id  IS '同一物理种子 (BEP-52) 的 v1+v2 记录共享此 uuid; v1-only 阶段永远 NULL';
COMMENT ON COLUMN torrents.name             IS 'info dict 中的权威种子名; agent 已按 encoding 字段解码为 UTF-8 后上报';
COMMENT ON COLUMN torrents.encoding         IS 'info dict 中声明的文本编码 (e.g. UTF-8 / GBK / Shift_JIS); NULL = 未声明';
COMMENT ON COLUMN torrents.is_private       IS 'BEP-27 私有种子标志; 默认从公开搜索 API 过滤掉';
COMMENT ON COLUMN torrents.size_bytes       IS '冗余字段: files_json 中所有文件 length 之和; 为查询性能保留';
COMMENT ON COLUMN torrents.file_count       IS '冗余字段: files_json 数组长度; 为查询性能保留';
COMMENT ON COLUMN torrents.files_json       IS 'jsonb 数组 [{path, length}]; 不在 PG 内独立搜索, 文件名搜索由 Meilisearch 承担';
COMMENT ON COLUMN torrents.discovery_method IS '该 infohash 在 agent 处的发现路径: dht_get_peers / dht_announce / task';
COMMENT ON COLUMN torrents.source_agent_id  IS '首次上报该种子的 agent; agent 删除后置 NULL';
COMMENT ON COLUMN torrents.found_count      IS '冗余字段: ingest 在每次重新观测时 +1 的计数器';
COMMENT ON COLUMN torrents.is_fake          IS '由 Slice 11 伪种子检测规则置位';
COMMENT ON COLUMN torrents.malware_flag     IS '由 Slice 11 恶意软件分类器置位';
```

### 4.2 peer 与 swarm

```sql
CREATE TABLE peers (
    peer_pk      bigserial PRIMARY KEY,
    ip           inet      NOT NULL,
    port         int       NOT NULL CHECK (port > 0 AND port < 65536),
    client_name  text,
    first_seen   timestamptz NOT NULL DEFAULT now(),
    last_seen    timestamptz NOT NULL DEFAULT now(),
    seen_count   int       NOT NULL DEFAULT 1,
    is_malicious boolean   NOT NULL DEFAULT false,
    UNIQUE (ip, port)
);

CREATE INDEX peers_ip        ON peers (ip);
CREATE INDEX peers_malicious ON peers (last_seen DESC) WHERE is_malicious;

CREATE TABLE torrent_peers (
    id              bigserial PRIMARY KEY,
    infohash        text   NOT NULL REFERENCES torrents(infohash) ON DELETE CASCADE,
    peer_pk         bigint NOT NULL REFERENCES peers(peer_pk) ON DELETE CASCADE,
    first_seen      timestamptz NOT NULL DEFAULT now(),
    last_seen       timestamptz NOT NULL DEFAULT now(),
    source_agent_id uuid   REFERENCES agents(agent_id) ON DELETE SET NULL,
    UNIQUE (infohash, peer_pk)
);

CREATE INDEX torrent_peers_peer ON torrent_peers (peer_pk);
```

注释：

```sql
COMMENT ON COLUMN peers.client_name      IS 'agent 端从 BT peer_id 前缀解析得到 (e.g. -qB4500- => "qBittorrent 4.5.0"); 原始 peer_id 解析后丢弃以节省空间';
COMMENT ON COLUMN peers.seen_count       IS '冗余字段: 每次观测时 +1, 用于 Slice 11 churn 分析';
COMMENT ON COLUMN peers.is_malicious     IS '由 Slice 11 规则引擎在该 peer 命中 Sybil / leech / probe 启发式时置位';
COMMENT ON COLUMN torrent_peers.source_agent_id IS '观测到该 swarm 成员关系的 agent; agent 删除后置 NULL';
```

### 4.3 DHT 观测

```sql
CREATE TABLE dht_nodes (
    node_id            bytea PRIMARY KEY,
    ip                 inet  NOT NULL,
    port               int   NOT NULL,
    first_seen         timestamptz NOT NULL DEFAULT now(),
    last_seen          timestamptz NOT NULL DEFAULT now(),
    is_suspicious      boolean NOT NULL DEFAULT false,
    suspicion_reasons  jsonb
);

CREATE INDEX dht_nodes_ip         ON dht_nodes (ip);
CREATE INDEX dht_nodes_suspicious ON dht_nodes (last_seen DESC) WHERE is_suspicious;

CREATE TABLE dht_observations (
    observed_at        timestamptz NOT NULL,
    observer_agent_id  uuid        NOT NULL REFERENCES agents(agent_id) ON DELETE SET NULL,
    node_id_prefix     bytea,
    ip                 inet  NOT NULL,
    port               int   NOT NULL,
    query_type         text  NOT NULL CHECK (query_type IN ('get_peers','announce_peer')),
    direction          text  NOT NULL CHECK (direction IN ('in','out')),
    infohash           text,
    suspicious         boolean NOT NULL DEFAULT false
) PARTITION BY RANGE (observed_at);

-- 初始建当前月 + 下月分区, 后续由 Slice 13 维护脚本接管
CREATE INDEX dht_obs_time_brin ON dht_observations USING brin (observed_at);
CREATE INDEX dht_obs_infohash  ON dht_observations (infohash) WHERE infohash IS NOT NULL;
CREATE INDEX dht_obs_ip        ON dht_observations (ip);
```

注释：

```sql
COMMENT ON COLUMN dht_nodes.node_id           IS 'DHT 节点 id 全量 20 字节; 此处作为身份标识保留 (仅 SELECT 使用)';
COMMENT ON COLUMN dht_nodes.suspicion_reasons IS 'jsonb 数组, e.g. ["sybil_same_ip","predictable_id"]';
COMMENT ON COLUMN dht_observations.node_id_prefix IS '观测到的 DHT node id 前 4 字节; 用于 Sybil/diversity 分析, 避免每行存储 20B 全量';
COMMENT ON COLUMN dht_observations.query_type     IS '观测到的 DHT 消息类型; 仅记录 get_peers / announce_peer; find_node 与 ping 在 agent 端过滤';
COMMENT ON COLUMN dht_observations.direction      IS 'in = 收到对方查询/响应; out = 我方主动发起';
COMMENT ON COLUMN dht_observations.infohash       IS '仅 get_peers / announce_peer 查询有值; ping 等无关查询为 NULL';
COMMENT ON COLUMN dht_observations.suspicious     IS '由 Slice 11 DHT 投毒检测器在写入时置位';
```

> **分区维护策略详细见子文档**：`docs/02-storage/partitioning.md`（Slice 2 实施时落盘）。

### 4.4 agent 与任务

```sql
CREATE TABLE agents (
    agent_id          uuid PRIMARY KEY,
    hostname          text  NOT NULL,
    version           text  NOT NULL,
    listen_port_v4    int,
    listen_port_v6    int,
    public_ip_v4      inet,
    public_ip_v6      inet,
    region            text,
    capabilities      jsonb NOT NULL DEFAULT '[]'::jsonb,
    registered_at     timestamptz NOT NULL DEFAULT now(),
    last_heartbeat_at timestamptz,
    status            text  NOT NULL DEFAULT 'online'
                      CHECK (status IN ('online','stale','offline')),
    current_metrics   jsonb
);

-- 心跳明细: 30 天 TTL, 月分区
CREATE TABLE agent_metrics_history (
    ts        timestamptz NOT NULL,
    agent_id  uuid        NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,
    metrics   jsonb       NOT NULL,
    PRIMARY KEY (ts, agent_id)
) PARTITION BY RANGE (ts);

CREATE INDEX agent_metrics_agent_ts ON agent_metrics_history (agent_id, ts DESC);

-- 日聚合: 永久保留, 不分区
CREATE TABLE agent_metrics_daily (
    bucket_date  date NOT NULL,
    agent_id     uuid NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,
    metrics      jsonb NOT NULL,
    PRIMARY KEY (bucket_date, agent_id)
);

CREATE TABLE task_queue (
    task_id          uuid PRIMARY KEY,
    kind             text NOT NULL CHECK (kind IN
                       ('crawl_infohash','pause','resume','update_config','announce_infohash')),
    payload          jsonb,
    target_agent_id  uuid REFERENCES agents(agent_id) ON DELETE SET NULL,
    status           text NOT NULL DEFAULT 'pending'
                       CHECK (status IN ('pending','assigned','done','failed','canceled')),
    created_at       timestamptz NOT NULL DEFAULT now(),
    assigned_at      timestamptz,
    completed_at     timestamptz,
    result           jsonb
);

CREATE INDEX task_queue_status ON task_queue (status, created_at) WHERE status IN ('pending','assigned');
CREATE INDEX task_queue_target ON task_queue (target_agent_id) WHERE target_agent_id IS NOT NULL;
```

注释：

```sql
COMMENT ON COLUMN agents.region          IS '自由文本 region 标签, e.g. "us-east" / "cn-shanghai"; 真实地域查询推到 Slice 13';
COMMENT ON COLUMN agents.capabilities    IS 'jsonb 能力数组, e.g. ["announce_peer","ipv6","public_ip"]';
COMMENT ON COLUMN agents.status          IS '由 Slice 4 心跳监控基于 last_heartbeat_at 派生; 不允许用户直接设置';
COMMENT ON COLUMN agents.current_metrics IS '冗余字段: 最近一次心跳的 metrics 快照; 完整历史在 agent_metrics_history';
COMMENT ON COLUMN agent_metrics_history.metrics IS '每次心跳的 jsonb 快照 (计数器 / 速率 / 缓存大小); 30 天 TTL 通过分区 DROP 实现';
COMMENT ON COLUMN agent_metrics_daily.metrics   IS '日级 jsonb rollup (选定指标的 min/max/avg); 永久保留';
COMMENT ON COLUMN task_queue.kind         IS '任务类型, 由 backend 下发; 语义见 Slice 12';
COMMENT ON COLUMN task_queue.target_agent_id IS 'NULL = 任意 agent 可领取; 非空 = 指定 agent';
COMMENT ON COLUMN task_queue.payload      IS 'jsonb 任务载荷, 形态因 kind 而异 (e.g. crawl_infohash 时为 {"infohash": "..."})';
COMMENT ON COLUMN task_queue.result       IS '任务完成时填入的 jsonb 结果或错误详情';
```

### 4.5 抓取失败：详情 + 聚合

```sql
-- 详情: 30 天 TTL, 月分区
CREATE TABLE fetch_failures (
    occurred_at  timestamptz NOT NULL DEFAULT now(),
    id           bigserial,
    infohash     text NOT NULL,
    agent_id     uuid REFERENCES agents(agent_id) ON DELETE SET NULL,
    peer_ip      inet,
    peer_port    int,
    failure_kind text NOT NULL CHECK (failure_kind IN
                   ('connect_timeout','handshake_fail','no_ut_metadata',
                    'metadata_size_overflow','sha1_mismatch','peer_reset','other')),
    detail       text,
    PRIMARY KEY (occurred_at, id)
) PARTITION BY RANGE (occurred_at);

CREATE INDEX fetch_failures_kind     ON fetch_failures (failure_kind, occurred_at DESC);
CREATE INDEX fetch_failures_infohash ON fetch_failures (infohash);

-- 小时聚合: 永久保留, 不分区
CREATE TABLE fetch_failure_stats (
    bucket_hour   timestamptz NOT NULL,
    agent_id      uuid NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,
    failure_kind  text NOT NULL,
    count         bigint NOT NULL,
    PRIMARY KEY (bucket_hour, agent_id, failure_kind)
);

CREATE INDEX fetch_stats_kind_time ON fetch_failure_stats (failure_kind, bucket_hour DESC);
```

注释：

```sql
COMMENT ON COLUMN fetch_failures.infohash     IS '不加 FK: 失败可能发生在 torrents 行尚未创建之时';
COMMENT ON COLUMN fetch_failures.failure_kind IS 'agent 端分类: timeout / handshake / no extension / size overflow / sha1 mismatch / reset / other';
COMMENT ON COLUMN fetch_failures.detail       IS '自由文本错误描述; 可空';
COMMENT ON COLUMN fetch_failure_stats.count   IS '按 (小时, agent, kind) 聚合的计数; 由 Slice 13 小时 rollup job 写入';
```

### 4.6 恶意分析（建表，Slice 11 接业务）

```sql
CREATE TABLE malicious_events (
    id          bigserial PRIMARY KEY,
    category    text NOT NULL CHECK (category IN
                  ('sybil','dht_poisoning','fake_torrent','bad_chunk',
                   'malware_distribution','ddos_reflector','unknown')),
    peer_ip     inet,
    peer_pk     bigint REFERENCES peers(peer_pk) ON DELETE SET NULL,
    infohash    text REFERENCES torrents(infohash) ON DELETE SET NULL,
    agent_id    uuid REFERENCES agents(agent_id) ON DELETE SET NULL,
    confidence  real NOT NULL CHECK (confidence >= 0 AND confidence <= 1),
    evidence    jsonb NOT NULL,
    observed_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX malicious_category_time ON malicious_events (category, observed_at DESC);
CREATE INDEX malicious_peer_ip       ON malicious_events (peer_ip) WHERE peer_ip IS NOT NULL;
CREATE INDEX malicious_infohash      ON malicious_events (infohash) WHERE infohash IS NOT NULL;

CREATE TABLE malware_samples (
    infohash       text PRIMARY KEY REFERENCES torrents(infohash) ON DELETE CASCADE,
    detected_by    text NOT NULL,
    malware_family text,
    detection_data jsonb,
    first_detected timestamptz NOT NULL DEFAULT now()
);
```

注释：

```sql
COMMENT ON COLUMN malicious_events.category   IS '恶意行为分类; 检测规则见 Slice 11';
COMMENT ON COLUMN malicious_events.confidence IS '规则置信度 0..1; 低于阈值默认从 API 过滤';
COMMENT ON COLUMN malicious_events.evidence   IS '按 category 不同的 jsonb 佐证数据, e.g. sybil 时 {"observed_node_ids":["..."]}';
COMMENT ON COLUMN malware_samples.detected_by IS '检测来源, e.g. virustotal / rule:exe-name-pattern';
COMMENT ON COLUMN malware_samples.malware_family IS '已知时填入规范化的恶意软件家族名';
```

### 4.7 tracker 关联

```sql
CREATE TABLE trackers (
    tracker_pk  bigserial PRIMARY KEY,
    url         text UNIQUE NOT NULL,
    scheme      text NOT NULL CHECK (scheme IN ('udp','http','https','ws','wss'))
);

CREATE TABLE torrent_trackers (
    id          bigserial PRIMARY KEY,
    infohash    text   NOT NULL REFERENCES torrents(infohash) ON DELETE CASCADE,
    tracker_pk  bigint NOT NULL REFERENCES trackers(tracker_pk) ON DELETE CASCADE,
    source      text   NOT NULL CHECK (source IN ('info_dict','magnet_tr','observed')),
    first_seen  timestamptz NOT NULL DEFAULT now(),
    last_seen   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (infohash, tracker_pk, source)
);

CREATE INDEX torrent_trackers_th ON torrent_trackers (tracker_pk);
```

注释：

```sql
COMMENT ON COLUMN trackers.url               IS '规范化后的 announce URL; UNIQUE 用于跨种子去重';
COMMENT ON COLUMN trackers.scheme            IS '从 URL 解析的传输协议; 通过 CHECK 约束限定取值';
COMMENT ON COLUMN torrent_trackers.source    IS '该关联的观测来源: info_dict announce-list / magnet ?tr= / 运行时观测';
```

> tracker 实体不维护时间戳列；首/末次观测从 `torrent_trackers` 聚合得到，避免双向同步。

## 5. 迁移文件组织（sqlx 内置 migrate）

```text
migrations/
├── 20260622_000001_extensions.sql        -- btree_gin 等基础扩展（不再需要 pg_trgm）
├── 20260622_000002_agents.sql
├── 20260622_000003_torrents.sql
├── 20260622_000004_trackers.sql          -- trackers + torrent_trackers
├── 20260622_000005_peers.sql             -- peers + torrent_peers
├── 20260622_000006_dht.sql               -- dht_nodes + dht_observations 父表 + 初始 2 个月分区
├── 20260622_000007_tasks.sql             -- task_queue
├── 20260622_000008_metrics.sql           -- agent_metrics_history (分区) + agent_metrics_daily
├── 20260622_000009_failures.sql          -- fetch_failures (分区) + fetch_failure_stats
├── 20260622_000010_malicious.sql         -- malicious_events + malware_samples
└── 20260622_000011_views.sql             -- 看板用 view (Slice 10 起填充, 此片留空文件)
```

**约定**：

- 文件名 `YYYYMMDD_HHMMSS_xxx.sql`，时间戳保证全局序。
- 每个 migration 文件**只做一类原子改动**（建表/加索引/插数据/加注释）。`COMMENT ON COLUMN` 语句紧跟在该表的建表语句之后、同一文件内。
- 一律 forward-only（不写 down 迁移）。理由：嗅探数据是观测事实，回滚 schema 通常意味着丢数据；若需回滚靠 ops 备份恢复。
- 迁移由 backend 启动时**自动执行**（`sqlx::migrate!("./migrations").run(&pool).await`），失败则启动失败。

## 6. backend 内部 `db` 模块骨架

```text
sniffer-backend/src/db/
├── mod.rs            # pub use; 暴露 Db / DbError; sqlx::migrate!() 调用
├── error.rs          # DbError(thiserror): 包装 sqlx::Error + 业务错误
├── pool.rs           # build_pool(database_url, max_conns) -> PgPool
├── torrent.rs        # TorrentRepo
├── peer.rs           # PeerRepo
├── tracker.rs        # TrackerRepo
├── agent.rs          # AgentRepo
├── dht.rs            # DhtRepo
├── malicious.rs      # MaliciousRepo
├── task.rs           # TaskRepo
├── metrics.rs        # MetricsRepo (history + daily)
└── failure.rs        # FetchFailureRepo (details + stats)
```

`mod.rs` 提供的根类型：

```rust
pub struct Db {
    pub pool: PgPool,
    pub torrent: TorrentRepo,
    pub peer: PeerRepo,
    pub tracker: TrackerRepo,
    pub agent: AgentRepo,
    pub dht: DhtRepo,
    pub malicious: MaliciousRepo,
    pub task: TaskRepo,
    pub metrics: MetricsRepo,
    pub failure: FetchFailureRepo,
}

impl Db {
    pub async fn connect(database_url: &str) -> Result<Self, DbError>;
    // build pool + run migrations + 构造各 Repo
}
```

Slice 2 阶段每个 Repo 的方法签名都列出来（参数类型对齐 `sniffer-protocol::model` 与 `sniffer-common::Infohash`），**函数体一律 `todo!()`**。Slice 4 只填实现，不再改签名。

示例（节选）：

```rust
// torrent.rs
pub struct TorrentRepo { /* 持有 PgPool 引用 (Arc) */ }

impl TorrentRepo {
    /// upsert: infohash 已存在则 last_seen/found_count 更新; 不存在则 INSERT
    pub async fn insert_or_touch(&self, t: &TorrentRecord) -> Result<(), DbError> { todo!() }
    pub async fn get(&self, ih: &Infohash) -> Result<Option<TorrentRow>, DbError> { todo!() }
    pub async fn list_by_first_found(&self, limit: i64, offset: i64)
        -> Result<Vec<TorrentRow>, DbError> { todo!() }
}
```

> 注意：`TorrentRepo` 不提供按 name 模糊搜索方法 —— 该职责归 Slice 10 的 Meilisearch 客户端。
> **完整 Repo 签名清单**详见子文档：`docs/02-storage/repo-signatures.md`（Slice 2 实施时落盘）。

## 7. 去重语义

```mermaid
flowchart TD
    A[agent 抓到 metadata] --> B["Infohash::compute_v1<br/>(info_dict_bytes)"]
    B --> C{"backend INSERT<br/>ON CONFLICT(infohash)"}
    C -->|新| D[INSERT torrents<br/>found_count=1]
    C -->|已存在| E["UPDATE last_seen=now()<br/>found_count += 1"]
```

**关键事实**：v1 infohash = sha1(info dict)。同一个 info dict 必产生相同 infohash → **infohash 即 metadata 的内容寻址**。

- `torrents` 表以 infohash 为 PK 是天然去重。
- 若出现"同 infohash 但 metadata 字段不一致"，说明对方 peer 在伪造（Slice 11 信号）。
- 重复 INSERT 走 `ON CONFLICT (infohash) DO UPDATE` 路径只更新 `last_seen_at` / `found_count`。`name` / `files_json` 等权威字段**首抓写入后不再覆盖**（具体策略 Slice 9 ingest 决定）。

`peers` 表用 `(ip, port)` 二元组去重（peer_id 不再存全量）；`trackers` 用 `url` 去重；`torrent_peers` / `torrent_trackers` 用关联键 UNIQUE。

## 8. v2/hybrid 扩展位（v1-only 阶段不启用）

**当前**：

- `torrents.hash_kind` CHECK 只允许 `'sha1'`。
- `torrents.hybrid_group_id` 永远 NULL。

**v2 落地时需要的 migration**（仅作记录，不写在 Slice 2）：

```sql
-- 放宽 hash_kind 允许值
ALTER TABLE torrents DROP CONSTRAINT torrents_hash_kind_check;
ALTER TABLE torrents ADD CONSTRAINT torrents_hash_kind_check
    CHECK (hash_kind IN ('sha1','sha256'));

-- (可选) hybrid_groups 元表, 携带组级元数据
CREATE TABLE hybrid_groups (
    group_id   uuid PRIMARY KEY,
    first_seen timestamptz NOT NULL DEFAULT now(),
    note       text
);
```

`torrents` 表本身结构不动，外键关联表全部沿用 `infohash` 主键。所有看板/分析查询保持 v1 path 不变。

## 9. 本地开发脚手架

### 9.1 `docker-compose.yml`（项目根）

```yaml
services:
  postgres:
    image: postgres:16-alpine
    container_name: magnet-sniffer-pg
    environment:
      POSTGRES_USER: magnet
      POSTGRES_PASSWORD: magnet
      POSTGRES_DB: magnet_sniffer
    ports:
      - "127.0.0.1:5432:5432"
    volumes:
      - magnet-pg-data:/var/lib/postgresql/data
      - ./scripts/pg-init.sql:/docker-entrypoint-initdb.d/init.sql:ro
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U magnet -d magnet_sniffer"]
      interval: 5s
      timeout: 3s
      retries: 10

  meilisearch:
    image: getmeili/meilisearch:v1.10
    container_name: magnet-sniffer-meili
    environment:
      MEILI_MASTER_KEY: ${MEILI_MASTER_KEY:-dev-master-key-change-me}
      MEILI_ENV: development     # 生产改 production
    ports:
      - "127.0.0.1:7700:7700"
    volumes:
      - magnet-meili-data:/meili_data
    healthcheck:
      test: ["CMD", "wget", "--no-verbose", "--tries=1", "--spider", "http://localhost:7700/health"]
      interval: 5s
      timeout: 3s
      retries: 10

volumes:
  magnet-pg-data:
  magnet-meili-data:
```

`scripts/pg-init.sql`：仅启用基础扩展（`btree_gin` 等），表结构由 sqlx migrate 在 backend 启动时建。

### 9.2 `scripts/db-up.sh` / `db-up.ps1`

```bash
#!/usr/bin/env bash
# scripts/db-up.sh
set -euo pipefail

# 1. 起 PG + Meilisearch (后台)
docker compose up -d postgres meilisearch

# 2. 等 PG 就绪
echo "waiting for postgres..."
until docker compose exec -T postgres pg_isready -U magnet -d magnet_sniffer >/dev/null 2>&1; do
    sleep 1
done

# 3. 等 Meilisearch 就绪
echo "waiting for meilisearch..."
until curl -sf http://localhost:7700/health >/dev/null 2>&1; do
    sleep 1
done

# 4. 执行 sqlx migrate (需 cargo install sqlx-cli --no-default-features --features postgres)
export DATABASE_URL="postgres://magnet:magnet@localhost:5432/magnet_sniffer"
sqlx migrate run

# 5. prepare 离线缓存
cargo sqlx prepare --workspace

echo "✅ PG + Meilisearch ready. .sqlx/ updated. Commit it if it changed."
```

PowerShell 版同等逻辑（`db-up.ps1`），便于 Windows 用户。

### 9.3 流程总图

```mermaid
flowchart LR
    A[新同事 clone] --> B[scripts/db-up.sh]
    B --> C[docker 起 PG + Meilisearch]
    B --> D[sqlx migrate run]
    B --> E[cargo sqlx prepare]
    E --> F[.sqlx/ 与 git 一致]

    G[改了 SQL] --> H[scripts/db-up.sh]
    H --> I[更新 .sqlx/]
    I --> J[一并 commit]

    L[CI 跑 build] --> M[SQLX_OFFLINE=true]
    M --> N[读 .sqlx/ 不连 DB]
```

## 10. 关键技术取舍

### 10.1 搜索外置 Meilisearch，PG 不建 GIN trgm

**决策**：所有按 `name` / 文件名 / tracker URL 的模糊搜索能力外置到 Meilisearch。PG 上**不**建任何 pg_trgm GIN 索引。

理由：

- 经验教训：纯 PG `LIKE %x%` 或 trgm GIN 在中文/日韩种子搜索下效果差（按字符切分无语义、typo 容错无）。
- 写入开销：GIN trgm 在每次 INSERT 触发 ~30 个索引项更新，是写放大主要来源。
- 体积：100 万种子的 trgm 索引 ~2 GB，与 Meilisearch 索引（~4 GB）对比并未显著省。

**Meilisearch 选型理由**：Rust 写、单二进制、写入接口直白、中文短文本（种子名）效果可用、与项目语言栈契合。Slice 10 才接同步管道，Slice 2 仅在 docker-compose 中并起服务。

**回退路径**：若 Slice 10 决定不上 Meilisearch，仅按精确 infohash / 完全限定查询访问 PG 即可，模糊搜索能力暂缺；不锁死 schema。

### 10.2 `dht_observations` 用 RANGE 分区 + BRIN 索引

体量预估：单 agent 每秒 ~10 条有意义观测（已过滤 find_node 噪声）× 多 agent → 日级千万行。B-tree 主索引会撑不住，写放大 + 索引体积大。

- **RANGE BY observed_at（按月）**：清理过期数据靠 DROP PARTITION，O(1)；查询 WHERE 时间范围自动 partition pruning。
- **BRIN 索引**：观测按时间顺序写入，物理顺序与 `observed_at` 强相关 → BRIN 极小（KB 级）且范围扫描友好。
- **`infohash` 列加 b-tree 部分索引**（`WHERE infohash IS NOT NULL`）：恶意分析需要按 infohash 反查观测序列。

### 10.3 不存 raw info dict / peer_id 全量 / node_id 全量 / find_node 观测

- **`torrents.raw_info_dict` 不存**：infohash = sha1(info dict)；结构化字段已展开；同 infohash 总能从 DHT 重抓。每百万种子省 5-30 GB。
- **`peers.bt_peer_id` 不存**：peer_id 的前缀已被 agent 解析为 `client_name`，后段是随机数无价值。每条省 20B。
- **`dht_observations.node_id_prefix` 仅 4B**：Sybil/diversity 检测看 IP 维度的 prefix 多样性即可，不需全量 20B。
- **`find_node` 观测不入库**：DHT 路由维护噪声，对分析无意义；`query_type` CHECK 移除该值。observation 体量减 60-70%。

### 10.4 文件树存 `torrents.files_json jsonb` 而非独立 `torrent_files` 表

PG 这边唯一访问模式是"按 infohash 取整个文件树展示详情"。jsonb 整块读写比 join 子表更高效。**搜索文件名的能力交给 Meilisearch**（在文档结构里把 file paths 摊平进搜索文档）。代价：PG 内丧失独立按文件名搜的能力 —— 接受。

### 10.5 `peers` 表 `(ip, port)` 二元组而非三元组去重

去掉 `bt_peer_id` 后，同一 ip:port 在不同时刻可能换 client（升级/重启）；但代理键 `peer_pk` 仍唯一。`client_name` 列在再观测时 UPDATE 为最新，旧 `client_name` 不保留 —— 接受。需要历史 client 变迁则查 `agent_metrics_history` 或将来加专门事件表。

### 10.6 `agent_metrics_history` 30 天明细 + `agent_metrics_daily` 永久聚合

近期高分辨率（分钟级）满足看板趋势曲线；长期低分辨率（天级）满足跨月对比。日聚合表极小（10 agent × 365 行 ≈ 几 MB），永久保留无压力。聚合 job 在 Slice 13 实现。

### 10.7 不用 ON DELETE RESTRICT，统一 CASCADE / SET NULL

种子-文件、种子-peer 关联：`CASCADE`（数据共生）。agent 引用：`SET NULL`（agent 可下线，观测事实保留）。RESTRICT 会让运维操作变成大手术。

### 10.8 时间戳一律 `timestamptz`

PG 内部存储 UTC 微秒，读取时按 session 时区呈现。与 sqlx `time` feature 的 `OffsetDateTime` 直接对齐。**绝不**用 `timestamp without time zone`。

## 11. 体量估算（按 100 万种子 / 10 agent / 30 天稳态）

| 表 | 行数 | 单行 | 总量（含索引） |
| --- | --- | --- | --- |
| `torrents`（含 files_json） | 1M | ~2.2 KB | ~3 GB |
| `peers` | ~10M | ~50 B | ~1 GB |
| `torrent_peers` | ~50M | ~50 B | ~5 GB |
| `dht_observations`（仅 get_peers/announce） | ~1.5亿/月 | ~50 B | ~15 GB（30 天 TTL） |
| `dht_nodes` | ~5M | ~80 B | ~800 MB |
| `agent_metrics_history` | ~864万 | ~500 B | ~5 GB（30 天 TTL） |
| `agent_metrics_daily` | ~3650 | ~500 B | <100 MB |
| `fetch_failures` | ~4000万 | ~150 B | ~6 GB（30 天 TTL） |
| `fetch_failure_stats` | ~50万/年 | ~50 B | <100 MB |
| 其他（agents/tasks/malicious/trackers） | 累计 | — | <1 GB |
| Meilisearch 索引 | 1M 文档 | — | ~4 GB |
| **合计稳态** | | | **~40 GB** |

`dht_observations` + `torrent_peers` 是两个最大头。若空间紧张，可缩短 `dht_observations` TTL（如 14 天）或进一步过滤观测 query_type。

## 12. 验收标准

- `docker compose up -d` 后 PG + Meilisearch 健康检查通过。
- `scripts/db-up.sh` 一键完成：起 PG + MS → migrate → prepare，退出码 0。
- `psql -U magnet -d magnet_sniffer -c '\dt'` 列出 14 张表（不含分区子表）。
- `\dx` 含 `btree_gin`（不含 `pg_trgm`）。
- `\d+ dht_observations` / `agent_metrics_history` / `fetch_failures` 显示 `Partition key: RANGE (...)`，且至少有当前月、下月两个子分区。
- backend 启动时 `sqlx::migrate!()` 跑通，幂等再启动无副作用。
- `cargo build --workspace` 在 `SQLX_OFFLINE=true` 下成功（验证 `.sqlx/` 入库正确）。
- `cargo clippy --workspace -- -D warnings` 全绿。
- `psql -c "\d+ torrents"` 显示每个业务字段都有注释。
- backend `db` 模块每个 Repo 都有 `#[cfg(test)] mod tests` 占位（即便只有 `#[test] fn signature_compiles() {}`），保证签名级回归。

## 13. 后续延伸

- Repository 函数体填充 → Slice 4。
- ingest pipeline（批量 INSERT、ON CONFLICT 路径、事务边界）→ Slice 9。
- Meilisearch 同步管道（mpsc + worker + 异步推送 + 启动重建）→ Slice 10。
- 看板 view / 物化视图 → Slice 10。
- `dht_observations` / `agent_metrics_history` / `fetch_failures` 三张分区表的滚动维护脚本 + `agent_metrics_daily` / `fetch_failure_stats` 聚合 job → Slice 13。
- v2/hybrid migration → 未来专项切片。
