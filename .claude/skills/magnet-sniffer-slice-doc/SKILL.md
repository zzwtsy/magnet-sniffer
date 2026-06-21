---
name: magnet-sniffer-slice-doc
description: Draft or extend a vertical-slice design doc under docs/NN-name/ for the magnet-sniffer Rust workspace. Use when the user asks to "写下一片 / 写 slice N / 落盘 04-backend-skeleton" 等同结构的中文设计文档。Reads existing slices (00-overview, 01-skeleton, 02-storage, 03-protocol) as the style template, gates writing on user-approved discussion, and updates the slice index in docs/00-overview/index.md.
---

# Magnet-Sniffer 切片文档撰写

为 `magnet-sniffer` workspace 撰写下一篇垂直切片设计文档。所有切片共享同一套结构、风格与中文写作约定 —— 本 skill 把这些约定固化下来，避免每次重复说明。

## 触发条件

用户提到下列任一情形：

- "写 slice N / 写下一片 / 落盘 04-backend-skeleton" 等同结构请求
- "把刚才讨论的 XX 切片写进 docs"
- "更新 00-overview 的切片索引"

如果用户只是想**讨论**某个切片设计而**未要求落盘**，**不要**立刻调用本 skill 创建文件 —— 先把方案聊清楚，等用户明确说"写进 docs / 落盘 / 写文档"再开始。

## 必须遵循的约定

### 1. 文件位置与命名

- 路径：`docs/NN-kebab-name/index.md`，编号两位数从 00 起，按 [docs/00-overview/index.md](../../../docs/00-overview/index.md) 第 5 节"实施切片索引"分配
- 复杂切片可在同一文件夹再放子文档（例如 `dht-routing-table.md`、`bep9-state-machine.md`），但 `index.md` 始终是入口

### 2. 中文与术语

- **正文一律中文**；技术术语保留英文原文（`Cargo.toml`、`tonic`、`Infohash` 等）
- 代码块、表格、proto/SQL 字段名英文；**注释中文**
- 不要混入"我们 / 我们的"等啰嗦主语；句子直接从动词或名词开始

### 3. 必备结构（参考 03-protocol 落盘版）

每篇 `index.md` 至少包含：

1. **第一行标题**：`# Slice N — <中文短标题>`
2. **导航 blockquote**：`> 上一片：[Slice N-1 — XXX](../NN-prev/index.md)`（00-overview 例外，它是入口）
3. **§1 目标与范围** — 明确"范围内 / 范围外"两个子列表
4. **架构 / 数据流 mermaid** — 至少 1 张 `flowchart` 或 `sequenceDiagram`
5. **文件组织 / 目录树** — 用 ` ```text ` 代码块画
6. **关键决策表** — Markdown 表格，列至少 3 列（维度 / 选型 / 说明 或 等价）
7. **取舍说明 / 反方案** — 写明拒绝了哪些备选与理由
8. **下一片预告 / 验收项** — 末尾收口

### 4. 写作风格

- **决策性而非教程性**：写"为什么这么选"，不写"什么是 gRPC"
- **每张表 / 每段都要有信息密度**：不写"这一节将介绍 XX"这种空话
- 限制动词："决定 / 选用 / 拒绝 / 拆分 / 收敛"等明确动作；少用"考虑 / 可能 / 或许"
- 链接相邻切片用相对路径 `../NN-name/index.md`；链接源码用 `../../sniffer-xxx/...`

### 5. Mermaid 约定

- 子图（`subgraph`）用于物理边界（agent / backend / 网络）
- 单向数据流用 `-->`，双向 `<-->`，控制/异步用 `-.->` 虚线
- 标签短，必要时用 `<br/>` 换行；不写超过 4 个词的边标签

### 6. proto / SQL 注释规则（跨切片硬约定）

- proto 字段：**一律加中文业务注释**，自解释字段（如 `id`、`created_at`）例外
- SQL DDL：**所有列加 `COMMENT`**；分区/索引策略写在切片文档而非裸 SQL 里

## 工作流

### 步骤 1 — 加载上下文

读以下三份基线，建立风格 / 术语 / 当前进度：

1. [docs/00-overview/index.md](../../../docs/00-overview/index.md) — 看第 5 节哪些切片"已落盘"、哪些"待讨论"
2. 上一篇已落盘切片的 `index.md` — 抄结构与导航
3. 用户当前对话里讨论过的设计要点

### 步骤 2 — 与用户对齐范围

如果用户**没有**先期讨论就直接说"写 slice 4"，**先反问**：

- 范围内 / 范围外的边界
- 与前后切片的接口
- 任何已经定的关键技术选型

不要凭空补设计 —— 切片文档是**对已达成共识的总结**，不是脑暴。

### 步骤 3 — 撰写

按上述"必备结构"逐节写。可参考 [docs/03-protocol/index.md](../../../docs/03-protocol/index.md) 作为最完整的范例（含 mermaid + 文件组织 + 决策表 + 取舍）。

### 步骤 4 — 更新索引

写完 `docs/NN-name/index.md` 后，**必须**编辑 [docs/00-overview/index.md](../../../docs/00-overview/index.md) 第 5 节的状态表，把对应行 `待讨论` 改为 `✅ 已落盘`，并把链接补上：

```markdown
| Slice 4 | [`docs/04-backend-skeleton/`](../04-backend-skeleton/index.md) | ✅ 已落盘 |
```

### 步骤 5 — 自检

- [ ] 第一行 `# Slice N — XXX` 标题格式
- [ ] 至少 1 张 mermaid
- [ ] §1 同时有"范围内"和"范围外"
- [ ] 至少 1 张决策表
- [ ] 中文正文 + 英文术语 + 中文注释
- [ ] 00-overview 索引已更新

## 反模式（避免）

- ❌ 用户只讨论了 30% 就开始写整篇切片
- ❌ 把教程性内容（"什么是 BEP-5"）塞进切片 —— 那是参考资料，不是设计文档
- ❌ 切片之间互相复制大段相同内容；共享内容应该在 00-overview 或专门的 reference 文档
- ❌ mermaid 图过大（> 15 节点）—— 拆成多张
- ❌ 创建了切片文件却忘了更新 00-overview 索引
