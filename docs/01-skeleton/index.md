# Slice 1 — 工程骨架与基础设施

> 上一片：[Slice 0 — 项目概览与架构总纲](../00-overview/index.md)

## 1. 目标与范围

把 workspace 从"3 crate 骨架 + 仅日志"扩展到"5 crate 完整骨架 + 共享类型与构建管线"，作为后续所有切片的地基。**只动结构与基础类型，不引入任何业务逻辑**（不写 DHT、不写 sqlx 调用、不写 axum 路由）。

**范围内**：

- 新增 `sniffer-protocol`、`sniffer-dht` 两个 lib crate；workspace `members` 注册。
- 扩充 `sniffer-common`：`Infohash` 类型（含 sqlx feature 门控桥接）、错误类型骨架、双栈地址辅助、ID newtype。
- 补全 `[workspace.dependencies]` 全量声明（仅声明，谁用谁加 `workspace = true`）。
- 构建管线：tonic-build 占位、sqlx 离线模式约定。
- `.env.example`、目录约定。

**范围外**：proto 文件具体内容（Slice 3）、表 schema（Slice 2）、backend/agent 业务模块（Slice 4/5）、magnet URI 解析（边缘需求，Slice 10/12 在 backend 内部实现）。

## 2. Crate 结构落地

```mermaid
flowchart TD
    common[sniffer-common<br/>已存在 扩充]
    proto[sniffer-protocol<br/>🆕 lib]
    dht[sniffer-dht<br/>🆕 lib]
    agent[sniffer-agent<br/>已存在]
    backend[sniffer-backend<br/>已存在]

    common --> dht
    common --> proto
    common --> agent
    common --> backend
    proto --> agent
    proto --> backend
    dht --> agent
```

最终目录：

```text
magnet-sniffer/
├── Cargo.toml                  # workspace 根: members + workspace.deps + lints + profile
├── Cargo.lock
├── rust-toolchain.toml
├── rustfmt.toml
├── .editorconfig / .gitattributes / .gitignore
├── .env.example                # 🆕
├── .sqlx/                      # 🆕 sqlx 离线缓存目录, 入库 (Slice 2 起填充)
├── docs/
│   ├── 00-overview/index.md
│   └── 01-skeleton/index.md    # 🆕 本文档
├── sniffer-common/             # 已有, 扩充
│   ├── Cargo.toml              # 加 [features] sqlx = ["dep:sqlx"]
│   └── src/
│       ├── lib.rs
│       ├── logger.rs           # 已实现
│       ├── infohash.rs         # 🆕 Infohash + sqlx 桥接(feature 门控)
│       ├── error.rs            # 🆕 CommonError(thiserror)
│       ├── net.rs              # 🆕 双栈地址辅助
│       └── ids.rs              # 🆕 AgentId/TaskId newtype
├── sniffer-protocol/           # 🆕
│   ├── Cargo.toml
│   ├── build.rs                # tonic-build 占位
│   ├── proto/.gitkeep
│   └── src/lib.rs              # pub mod pb; pub mod model;
├── sniffer-dht/                # 🆕
│   ├── Cargo.toml
│   └── src/lib.rs              # 占位; Slice 6 起实质实现
├── sniffer-agent/              # 已有
└── sniffer-backend/            # 已有, main.rs 在 Slice 4 改异步
```

## 3. `sniffer-common` 扩充

### 3.1 `Infohash`（v1 only，预留 v2 扩展）

BitTorrent v1 infohash 是 **20 字节 SHA-1**。本期只支持 v1。

```rust
#[derive(Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct Infohash([u8; 20]);

impl Infohash {
    pub const fn from_bytes(b: [u8; 20]) -> Self;
    pub fn from_hex(s: &str) -> Result<Self, CommonError>;
    pub fn compute_v1(info_dict_bytes: &[u8]) -> Self;     // sha1
    pub const fn as_bytes(&self) -> &[u8; 20];
    pub fn to_hex(&self) -> String;                         // lowercase
}

// Display/Debug = lowercase hex
// serde::{Serialize, Deserialize} = hex string
// 默认零额外依赖
```

**v2 扩展位（注释级别，本期不实现）**：

```rust
// 未来若需支持 v2 (BEP-52, SHA-256, 32 bytes):
//   pub struct InfohashV2([u8; 32]);
//   pub enum AnyInfohash { V1(Infohash), V2(InfohashV2) }
// 当前类型保持 [u8; 20] 单一形态, 不预先引入 enum 污染全栈签名。
// schema 层在 Slice 2 用 hash_kind 字段预留扩展。
```

### 3.2 sqlx 桥接（feature 门控，方案 C）

`sniffer-common` 加 feature：

```toml
[features]
default = []
sqlx = ["dep:sqlx"]

[dependencies]
sqlx = { workspace = true, optional = true }
```

`infohash.rs` 内：

```rust
#[cfg(feature = "sqlx")]
mod sqlx_impl {
    use super::Infohash;
    use sqlx::{Decode, Encode, Postgres, Type, postgres::PgTypeInfo};

    // 选择 PG 表示: TEXT (40-char lowercase hex)
    // 理由: 便于人工查询/日志可读; pg_trgm 索引可直接用; 占用 40B vs bytea 20B 在 PG 里
    //       因 toast 阈值/对齐影响有限, 而可读性收益大。
    impl Type<Postgres> for Infohash {
        fn type_info() -> PgTypeInfo { <String as Type<Postgres>>::type_info() }
    }
    impl<'q> Encode<'q, Postgres> for Infohash { /* to_hex 后委托 String */ }
    impl<'r> Decode<'r, Postgres> for Infohash { /* String -> from_hex */ }
}
```

`sniffer-backend/Cargo.toml` 里：

```toml
sniffer-common = { workspace = true, features = ["sqlx"] }
```

agent / dht / protocol 的 `sniffer-common` 引用**不开 feature**，零额外编译开销。

### 3.3 `error::CommonError`（严格 thiserror 类型化）

```rust
#[derive(thiserror::Error, Debug)]
pub enum CommonError {
    #[error("invalid infohash hex: expected 40 chars, got {0}")]
    InfohashHexLength(usize),

    #[error("invalid infohash hex char")]
    InfohashHexChar(#[from] hex::FromHexError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
```

各 crate 自有 `Error` 通过 `#[from]` 串。**不引入 `anyhow`** 到 `common`（agent/backend 二进制层可用 anyhow 做最外层兜底，但 lib 全部走 thiserror）。

### 3.4 `net` —— 双栈辅助

```rust
pub enum AddressFamily { V4, V6 }

impl AddressFamily {
    pub fn of(addr: &SocketAddr) -> Self;
    pub fn of_ip(ip: &IpAddr) -> Self;
}

/// 判断是否为公网可路由 unicast 地址。
/// 用于 announce_peer 决策: NAT 后(私网/loopback/link-local) 必须返回 false。
pub fn is_global_unicast(ip: &IpAddr) -> bool;

/// DHT compact node 编码 (BEP-5 IPv4 + BEP-32 IPv6)
pub fn compact_v4(addr: &SocketAddrV4) -> [u8; 6];
pub fn compact_v6(addr: &SocketAddrV6) -> [u8; 18];
pub fn parse_compact_v4(b: &[u8; 6]) -> SocketAddrV4;
pub fn parse_compact_v6(b: &[u8; 18]) -> SocketAddrV6;
```

只做标准库薄包装，不引入 `ipnet`/`maxminddb`（地域分析在 Slice 13 才用）。

### 3.5 `ids`

```rust
#[derive(Copy, Clone, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
pub struct AgentId(pub Uuid);

#[derive(Copy, Clone, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
pub struct TaskId(pub Uuid);

// 各自实现 FromStr / Display, 与 Uuid 直通
```

newtype 防止裸 `Uuid` 在签名里被混淆为不同语义的 ID。

### 3.6 `lib.rs`

```rust
pub mod logger;
pub mod infohash;
pub mod error;
pub mod net;
pub mod ids;

pub use error::CommonError;
pub use infohash::Infohash;
pub use ids::{AgentId, TaskId};
pub use net::AddressFamily;
```

## 4. `[workspace.dependencies]` 补全

集中声明，使用方加 `workspace = true`。**版本号在 Slice 1 实施时用 `cargo add --workspace` 锁到当时最新稳定版**，下表只列 crate 与 features。

| 类别 | crate | features | 用途 |
| --- | --- | --- | --- |
| 既有 | tokio | full | 已声明 |
| 既有 | serde | derive | 已声明 |
| 既有 | anyhow | — | 二进制层兜底 |
| 既有 | thiserror | — | 全部 lib crate |
| 既有 | tracing / tracing-subscriber / tracing-appender | env-filter,fmt,std,time | 已声明 |
| 既有 | dotenvy | — | 已声明 |
| **改** | time | macros, **formatting**, **parsing**, **serde** | 全栈时间统一；补 features |
| 🆕 | serde_json | — | DTO/evidence/config |
| 🆕 | hex | — | Infohash hex 编解码 |
| 🆕 | uuid | v4, serde | AgentId/TaskId |
| 🆕 | dashmap | — | agent 并发去重缓存 |
| 🆕 | bytes | — | BT 协议缓冲 |
| 🆕 | rand | — | DHT node id |
| 🆕 | bendy | — | bencode |
| 🆕 | sha1 | — | infohash 计算 |
| 🆕 | prost | — | proto 类型 |
| 🆕 | tonic | transport, tls | gRPC 双端 |
| 🆕 | tonic-build | — | proto codegen (build-deps) |
| 🆕 | axum | macros | backend REST |
| 🆕 | tower | — | 中间件基座 |
| 🆕 | tower-http | cors, trace, compression-gzip | 中间件 |
| 🆕 | sqlx | runtime-tokio-rustls, postgres, macros, migrate, time, uuid, ipnetwork, json | **不开 default features**（避免拉 chrono/mysql/sqlite） |
| 🆕 | sniffer-protocol | path | workspace 内部 |
| 🆕 | sniffer-dht | path | workspace 内部 |

`sniffer-common` 自身的 `[features]`：

```toml
[features]
default = []
sqlx = ["dep:sqlx"]
```

## 5. 构建管线

### 5.1 `sniffer-protocol/build.rs`（占位）

最小实现：扫描 `proto/` 目录，无 `.proto` 时直接返回；有则交给 `tonic_build::compile_protos`。这样 Slice 3 加入真 proto 时无需改 build.rs。

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_dir = std::path::Path::new("proto");
    if !proto_dir.exists() { return Ok(()); }
    let protos: Vec<_> = std::fs::read_dir(proto_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "proto"))
        .map(|e| e.path())
        .collect();
    if protos.is_empty() { return Ok(()); }
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&protos, &[proto_dir])?;
    Ok(())
}
```

`lib.rs` 通过 `tonic::include_proto!("sniffer.v1");` 引入到 `pub mod pb`（Slice 3 阶段才有内容）。

### 5.2 sqlx 离线模式约定

#### 背景

sqlx 的 `query!`/`query_as!` 宏在**编译期**做 SQL 类型校验：解析 SQL、查表结构、推断字段类型、校验参数。这要求 `cargo build` 时**能连一个跑着 schema 的 PG**，否则编译失败。

这在三种场景下不可接受：CI 环境不一定有 DB；新同事 clone 后第一次 build 时还没建库；离线工作。

#### 解决方式：sqlx prepare + `.sqlx/` 缓存

`cargo sqlx prepare --workspace` 把所有 `query!`/`query_as!` 的 SQL + 推断元数据序列化成 JSON 存到项目根 `.sqlx/`。设 `SQLX_OFFLINE=true` 后，宏从 `.sqlx/` 读取缓存而不连数据库。

#### 项目约定

```mermaid
flowchart LR
    Dev[开发者本地<br/>有 PG] -->|cargo sqlx prepare| Sqlx[.sqlx/ 目录]
    Sqlx -->|提交| Git
    Git -->|拉取| CI[CI / 新同事]
    CI -->|SQLX_OFFLINE=true| Build[cargo build 不需 DB]
```

| 项 | 约定 |
| --- | --- |
| `.sqlx/` 目录 | **入库**（review 友好、CI 零配置、官方推荐做法） |
| `DATABASE_URL` | 走 `.env`，不入库（开发者本地各自配置） |
| CI 默认环境变量 | `SQLX_OFFLINE=true` |
| 改 SQL/schema 后 | 必须本地跑 `cargo sqlx prepare --workspace` 并把更新后的 `.sqlx/` 一并提交 |
| 缺失 prepare | CI 编译失败（变更 SQL 但忘 prepare 会被 CI 拦下） |

#### 配套脚本（Slice 2 落地）

提供 `scripts/db-up.sh`（或 `db-up.ps1`）：docker compose 起 PG → 跑 migration → `cargo sqlx prepare --workspace`。降低新同事上手成本；CI 不依赖此脚本（直接走 `SQLX_OFFLINE`）。

具体表 schema 与第一次 prepare 在 Slice 2 落地。

### 5.3 `.env.example`

```log
# Logging
RUST_LOG=info,sniffer_agent=debug,sniffer_backend=debug

# Backend (Slice 2/4 起使用)
DATABASE_URL=postgres://magnet:magnet@localhost:5432/magnet_sniffer
BACKEND_GRPC_LISTEN=[::]:50051
BACKEND_REST_LISTEN=[::]:8080
SQLX_OFFLINE=true

# Agent (Slice 5 起使用)
AGENT_ID=                          # 空则自动生成 uuid v4 并落本地状态
AGENT_BACKEND_ENDPOINT=http://[::1]:50051
AGENT_DHT_LISTEN_V4=0.0.0.0:6881
AGENT_DHT_LISTEN_V6=[::]:6881
AGENT_PUBLIC_IP=                   # 留空表示 NAT 后, announce_peer 默认关闭
AGENT_ANNOUNCE_PEER=false
```

## 6. 关键技术取舍

### 6.1 v1 only（SHA-1）

**决策**：本期只支持 v1，`Infohash` 内部为单一 `[u8; 20]`，不预先引入 enum。理由：

- 全网现存绝大多数 swarm 仍是 v1；BEP-9 主流路径走 v1 info dict。
- 预先用 `enum AnyInfohash` 会让 proto / DB schema / 索引 / 函数签名全部带 hash 类型字段。
- 留扩展点：`Infohash` 现名保留即代表 v1；未来若加 v2，新增 `InfohashV2` 与 `AnyInfohash` 枚举，迁移面收敛在边界处。
- DB 层在 Slice 2 用 `hash_kind text DEFAULT 'sha1'` 字段预留，加 v2 时只放宽 CHECK 约束。

### 6.2 `Infohash` 与 sqlx 桥接：feature 门控（方案 C）

`sniffer-common` 是被所有 crate 引入的最底层。直接引 sqlx 会让 agent/dht 也编译 sqlx，构建时间显著增加且无必要。

**方案 C（采纳）**：`common` 加可选 sqlx 依赖与 `sqlx` feature；只有 backend 在 `Cargo.toml` 里 `sniffer-common = { workspace = true, features = ["sqlx"] }`。这是 Rust 生态的标准做法（`uuid`/`time`/`ipnet` 都用同样模式）。

PG 表示选择 **TEXT 40-char lowercase hex** 而非 `bytea`：人工查询与日志可读、`pg_trgm` 索引可复用、占用差异在 PG TOAST 阈值下影响有限。

### 6.3 `sniffer-common` 不引 tonic

同 6.2 思路。proto 类型由 `sniffer-protocol` 提供，agent/backend 直接引用 protocol 拿 codegen 类型；common 永远零 RPC 依赖。

### 6.4 proto codegen 输出位置

走 `OUT_DIR` 而非 checked-in `src/pb.rs`。理由：避免人手维护生成代码；CI 一致性靠 `tonic-build` 版本锁。代价：IDE 跳转需 `cargo check` 一次。

### 6.5 sqlx 默认 features 关闭

`sqlx = { ..., default-features = false, features = [...] }`。默认 features 会拉 `chrono` 与 `mysql`/`sqlite`，与"全栈 time、只用 PG"原则相悖。

### 6.6 错误处理：lib 全部 thiserror，bin 可用 anyhow 兜底

- 所有 lib crate（`common`/`protocol`/`dht`）只用 `thiserror`，导出类型化错误。
- 二进制 crate（`agent`/`backend`）的 `main.rs` 与最外层错误聚合可用 `anyhow::Result`，便于错误链打印。**不在内部模块用 anyhow**，保持类型化错误在 backend/agent 内部模块边界传递。

## 7. 验收标准

- `cargo check --workspace` 全绿。
- `cargo check --workspace --features sniffer-common/sqlx`（验证 sqlx 桥接代码可编译；通过 backend crate 引用间接覆盖）全绿。
- `cargo clippy --workspace --all-targets -- -D warnings` 全绿。
- `cargo fmt --check` 全绿。
- `cargo test --workspace` 通过。最低用例：
  - `Infohash::from_hex` round-trip：构造 → `to_hex()` 反向相等。
  - `Infohash::from_hex` 长度错误返回 `CommonError::InfohashHexLength`。
  - `Infohash::compute_v1` 对固定字节流断言已知 sha1（用 BEP 文档示例向量）。
  - `net::is_global_unicast`：v4 私网（10.x、192.168.x、172.16.x）/loopback/link-local 返回 false；公网返回 true。v6 fe80::/ULA(fc00::/7)/loopback/未指定返回 false；2000::/3 公网返回 true。
  - `net::compact_v4`/`compact_v6` round-trip。
- `cargo doc --workspace --no-deps` 无 broken intra-doc link 警告。
- 5 个 crate 全部注册到 workspace；`sniffer-protocol`/`sniffer-dht` 的 `lib.rs` 至少有占位 `pub fn version() -> &'static str { env!("CARGO_PKG_VERSION") }` 让链接通过。
- `.env.example` 入库；`.env` 仍 gitignore。

## 8. 后续延伸（本切片不做但要预留）

- proto codegen 真接入 → Slice 3。
- 表 schema 与 `Infohash` sqlx 桥接的实际使用 → Slice 2。
- `sniffer-dht` 实质实现 → Slice 6 起。
- magnet URI 解析（仅 backend 边缘需求）→ Slice 10/12 在 backend 内部 `magnet.rs` 实现，不进 `common`/`dht`。
- `scripts/db-up.sh` / `db-up.ps1` 配套脚本 → Slice 2。
