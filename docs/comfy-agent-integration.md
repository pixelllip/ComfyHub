# 与官方 Comfy Agent 的关系：定位、能接什么、不接什么

> 缘起（2026-10）：用户给出 <https://comfy.org/zh-CN/agent>，问"我们有哪些东西可以结合官方 agent 对接过去、
> 不能被他取代"。
>
> 这份文档是**结论 + 证据链**，不是实施方案。每条结论后面都带出处；**没核实的写在 §6，不许当已知用**。
>
> 证据采集方式（要复现看这里）：`docs.comfy.org` 是 Mintlify，**在任意页 URL 后面加 `.md` 就能拿到正文**
> （直接 fetch HTML 只会拿到左侧导航）；`Comfy-Org/ComfyUI_frontend` 的 agent 子树、`Comfy-Org/comfy-cli`
> 的 `comfy_cli/agent/`、`comfy_cli/skills/comfy-agent-permissions/SKILL.md`、`Comfy-Org/Comfy-Desktop`
> 的 `src/main/lib/agentTap.ts` 都是公开可读的；agent 服务端（`Comfy-Org/cloud`）**匿名不可读**。

---

## 1. 一句话定位

> **Comfy Agent 是官方开在 ComfyUI 里的一扇窗，不是这栋楼。**
> 它按设计只看得见"当前画布"；看不见库、看不见产物台账、看不见另一台设备。
> ComfyHub 的位置不是另一扇窗，**是这栋楼的档案室 + 门禁 + 物流**。
>
> 而且要认清一件事：**这扇窗我们伸不进手**（§2.4）。所以正确动作不是"接线"，
> 是把自己做成**任何 agent 都能调用的那个本地能力网关**，然后等它开口。

三条不可替代性，逐条有证据：

| 不可替代性 | 证据 |
| --- | --- |
| **库**（提示词 / 标签 / 角色卡 / 配方 / 记忆 / 附件） | 本地 agent 的沙箱把 readable 范围压在"项目文件夹 + ComfyUI 安装"，凭据/浏览器/整个磁盘/自身 data dir 都在**永不授予**档；附件通道**协议上不收字节**（§2.3、§2.4） |
| **台账**（产物入库 / SHA-256 判重 / NSFW 标记 / 来源） | 官方产物落在 Cloud 资产库或节点里右键另存；没有长期库、没有可复现工作流的沉淀 |
| **多端 + 离线** | 官方云历史与本地历史**永久不同步**（官方原文）；ComfyHub 有 SQLite 本地真源 + 手机 `submit_jobs` 排队 + run-kit 版本化下发 |

反过来，**该主动认输的两块**（别再投工程量，尤其不要因为"官方也有"而加厚）：

- **画布内的图编辑与工作流理解** —— 官方是 Yjs CRDT 权威 + 闭源 agent 服务，正面复刻没有胜算；
- **通用对话体验的"免费额度"部分** —— 官方 Opus 4.8、免费初始 token，
  我们这一侧的差异只能落在**自选模型 / 自选 Key / 对话不出本机**上（§2.3）。

---

## 2. 事实与证据

### 2.1 三条产品线，不许混为一谈

官方文档原话：**"Comfy Agent 与 Comfy MCP 和 Comfy CLI 是相互独立的"**
（<https://docs.comfy.org/zh/agent-tools/in-app-agent.md>）。三条线的形态：

| 产品 | 是什么 | 和 ComfyUI 的关系 |
| --- | --- | --- |
| **Comfy Agent** | ComfyUI 界面里的 Agent 面板；后端是**另一个闭源在线服务** | 前端扩展 + 外部服务；**不是 MCP** |
| **Comfy MCP** | MCP *服务端*（云端 `https://cloud.comfy.org/mcp`；本地 `comfy-mcp` 走 stdio） | 从**外部**驱动 ComfyUI 的客户端 |
| **Comfy CLI** | `comfy` 终端工具 | 本地运维 + 伙伴模型生成 |

旁证：`Comfy-Org/ComfyUI`（master，1489 个 tree 条目）里除 `AGENTS.md` 外**没有任何** `*agent*` / `*mcp*` 文件；
openapi 里也没有 agent 路由。

> ⚠ **因此"MCP 是 Comfy Agent 的扩展面"这句话是错的。** MCP 是并列的另一条线。

### 2.2 官方 Agent 现在到底能干什么

| 维度 | 现状 | 出处 |
| --- | --- | --- |
| 落地形态 | **Comfy Cloud 已 GA**（2026-10-01 公布）；**Desktop 本地版候补名单中、未发布** | [blog](https://blog.comfy.org/p/comfy-agent-the-first-agent-for-craft) · [in-app-agent.md](https://docs.comfy.org/agent-tools/in-app-agent.md) · <https://links.comfy.org/agentalpha> |
| 位置 | ComfyUI 界面内的 Agent 面板；构建门（build）**只在 Cloud 分发版挂载** | `ComfyUI_frontend/src/workbench/extensions/agent/README.md` · ADR `CRDT-FOLLOWER-0025` |
| 能做什么 | 查模板/模型/节点 → **在画布上建图** → 改一段图（增删连节点）→ 运行前校验 → 运行 → 读图像/视频音频属性 → 媒体编辑（裁剪/拼接/配乐） | in-app-agent.md |
| **不能**做什么 | **不能自己建/切标签页**（必须用户在面板里选目标工作流）；云端无任意 shell/文件访问；无模型可选（只有一个模型） | in-app-agent.md |
| 并发与历史 | **最多 5 个并行会话**，各带完整历史；单会话同时只允许一轮（否则 `409 TURN_IN_PROGRESS`） | blog · `agentApiSchema.ts` |
| **历史不同步** | 官方原文：云对话存 Comfy Cloud、本地对话存本机，**两份历史不会同步**；取决于云/本地，不取决于你在哪个窗口打开 | [data-privacy.md](https://docs.comfy.org/zh/agent-tools/in-app-agent-data-privacy.md) |
| 模型与计费 | **Anthropic Claude Opus 4.8**（文档定价表只有这一行）；按 Comfy Credits 扣（输入 1.055 / 5m 缓存写 1.319 / 1h 缓存写 2.110 / 缓存命中 0.106 / 输出 5.275，单位"credits/1K tokens"）；免费初始 token 之后与出图共用同一个 Credits 池；**没有 BYO Key** | in-app-agent.md |
| 运行权限 | 面板上的 `Ask` / `Auto`（线上协议还有第三档 `auto_limited` + 额度上限）；**额度管的是"跑工作流的 GPU"，不是聊天的 token** | `types.gen.d.ts`（`@comfyorg/ingest-types`） |

### 2.3 它每轮往上传什么（这条决定隐私立场）

`AgentPostMessageRequest`（公开类型 `@comfyorg/ingest-types`）逐字段：

| 字段 | 内容 |
| --- | --- |
| `content` | 用户这条消息 |
| `draft.content` | **当前画布的完整存盘格式图**（前端 `app.graph.serialize()` 的输出） |
| `workflow_id` / `current_tab` | 要编辑的目标工作流 |
| `open_tabs[]` | 已开标签页的名字 + 工作流 id（"advisory context, not a grant"） |
| `workflow_references[]` | 显式只读引用，工作区鉴权、去重、**上限 50 条**、名字截断 120 字符 |
| `selection` | 画布选中项 |
| `attachments[]` | **只有文件名**。原文："the agent wires them into the workflow by filename — **it never receives file bytes here**" |
| `client_id` / `client_message_id` / `ask_kinds` | 本地运行的画布执行反馈通道 / 幂等 id / 客户端能渲染哪几种审批卡 |

而且**本地版也不例外**（官方原文）：本地安装存聊天记录与文件，但
**"你的消息与相关工作流信息由 Comfy 的在线服务处理"**。

→ 这就是我们"内容只在自身文件夹、模型自选、可以离线"那条线的对照面。

### 2.4 它读不了我们的库：这是**设计**，不是"还没做"

本地 agent 的安全模型写在 comfy-cli 自带的 `comfy-agent-permissions` SKILL.md 里
（<https://github.com/Comfy-Org/comfy-cli/blob/main/comfy_cli/skills/comfy-agent-permissions/SKILL.md>）：

- **不问就能用**：项目文件夹（`AGENT_PROJECT_DIR`，默认 `<data dir>/project`）读写跑；
  **ComfyUI 安装的 `custom_nodes` 文件夹可写**（安装的其余部分只读，模型从 `comfy model download` 进）；
  沙箱没拒绝的地方可读；网络走 **egress 白名单**（ComfyUI / Comfy Cloud / PyPI / GitHub / Hugging Face / Civitai / 若干模型站）。
- **永不授予**：凭据与浏览器存储（`~/.ssh`、`~/.aws`、`~/.config/gh`、钥匙串、浏览器 profile）、
  **包含这些东西的目录（含 home 目录）**、整个磁盘、**agent 自己的 data dir**、任何叫 `.env` / `.env.*` / `.envrc` 的目录；
  网络上还有遥测端点、共享对象存储根、**loopback/link-local**、通配符、裸名字。
- **授权只认终端**：agent 的 `request_path` / `request_host` 只往 `permissions.json` 写一条 pending，
  原文 **"在聊天里打 yes 不算批准"**；人工跑 `comfy agent allow --approve <id>` 或点面板按钮才生效。
- 数据目录默认 `~/.comfy-agent`（**注意：这与 AGENTS §13"内容一律存在自身文件夹内"正面相反**）。

逐条推到我们身上：

| 它想读我们的… | 结果 |
| --- | --- |
| `<根>\.db` / `<根>\storage` / `<项目根>\skills` | 要逐目录人工批准；项目只要落在 home 或"含敏感目录的目录"下就是一刀切拒绝 |
| 附件原图字节 | 协议上不收（§2.3） |
| 通过网络来取 | egress 白名单 + loopback 永不授予 |

### 2.5 **但今天根本不存在那条通道**

即使我们愿意授权，也**没有一条路能让它调我们**：

- Comfy Agent 自带工具目录（工具清单在闭源服务里），**它不是 MCP 客户端**，不消费第三方 MCP 服务端；
- 它对外只有 13 条 `/api/agent/*` REST + 8 个 `/ws` 事件 + CRDT 帧，
  **没有版本策略、没有开发者文档、没有兼容承诺**，而且 ADR 明文要求前端
  "**永不写共享文档**"、`LGraph`/`LGraphNode`/`LLink` **保持 agent-unaware** —— **设计上就是单向封闭的**；
- 唯一的本地发现契约是 `agent.json`（含 `port` 与 `token`）+ `permissions.json` + `egress-allow.json`，
  但**格式是内部镜像文件、没有规格**，且功能未对用户开放。

> **结论**：在桌面本地版落地、且官方给出 MCP 客户端支持之前，
> **"Comfy Agent 直接调我们的本地服务"做不到 —— 不是难，是不存在这条通道。**

### 2.6 生态信号：别人也在等

因为没有 agent 插件 API，第三方自己在搭桥：
`abuzreq/ComfyUI-Agent-Bridge`（"Human-in-the-loop bridge between the ComfyUI canvas and local AI agents"）
—— 这是 workaround，不是官方 hook。**需求真实存在，官方没给口子。**

### 2.7 官方替我们写的那句需求说明书

`comfy-skills` 的 README 里有一条作者纪律，原文：

> **"Live specifics (never freeze): tell the agent to discover current options at run time via the tools."**
> （"如果你发现自己在命令里写死了模型名、模板名或节点 id，停手，改成指给那个能返回当前集合的工具。"）

也就是：**官方的 skill 体系要求 agent 运行时去问工具，而不是硬编码。**
我们的 MCP Server 正好是本地那个"能被问的工具"。

---

## 3. 可以做的接合点（按"今天能不能落地 × 会不会白费"排序）

| # | 做什么 | 今天能落地 | 会不会白费 | 说明 |
| --- | --- | --- | --- | --- |
| **1** | **MCP Server**（Streamable HTTP 绑 `127.0.0.1` + stdio 入口），只读优先 | ✅ | ❌ 不会 | DSH / Claude Code / Cursor 今天就能用；官方本地版一旦支持 MCP 客户端直接白得。实现上 `server-rs/src/tools/mcp.rs` 那套 JSON-RPC 编解码**反着用**即可 |
| **2** | **产物回流**（本地捕获链路 + 来源标记） | ✅ | ❌ 不会 | `comfyui/comfyhub_capture` + `/history` 轮询原样可用；官方每轮把画布存盘格式一起上传，回流时顺手把 `capture_runs.raw` 补齐 = 产物进库即带可复现工作流 |
| **3** | **技能导出为可粘贴文本** | ✅ | ❌ 不会 | 官方接受 `SKILL.md` 的 `name` + `description` 作为导入格式，但 `description` 必须**单行**（含 CR/LF 直接被拒）、`always: true` **被故意拒绝**。所以导出器要剥 `whenToUse`/`version`/`always` 并把 `description` 压平 |
| **4** | 借 `custom_nodes` 这个**唯一预授权写位**，把库索引落到 ComfyUI 里 | ⚠️ 能做但要先探 | ⚠️ 中等 | 只用到一个官方已预授权的位置，用户不必再授权；但它落在 ComfyUI 进程内、受 ComfyUI 权限约束（与 AGENTS §12 同源），且与"接 Comfy Agent"其实无关。**不进 S1~S3** |
| **5** | `agent.json` / `/health` 只读探测 + 界面状态条 | ✅ | ⚠️ 收益小 | 符合"A 列不出用户只能猜"的纪律，但**已降级为可选**（我们接不上，显示状态只能解释"为什么连不上"） |

**明确不做的（写进文档省得以后有人手痒）**：

1. **不接 `/api/agent/*`**（13 REST + `/ws` CRDT）—— 闭源服务、无版本承诺、ADR 明文单向封闭；
2. **不往 `~/.comfy-agent` 或 Cloud Agent 资产库写任何东西** —— 撞 AGENTS §13 与"内容只在自身文件夹"总原则；
3. **不做"塞个自定义节点当桥"** —— `ComfyUI-Agent-Bridge` 已证明这是 workaround，且它同样绕不过 agent 的路径 fence；
4. **不把库推上云**（哪怕是为了让云 Agent 看见）。

---

## 4. S1 清单（只读 MCP Server）

**范围**：只读。**判据**：全程 0 次文件系统写、0 次上游 LLM 请求。

- **传输**：`127.0.0.1` Streamable HTTP（`POST /mcp`）+ stdio 入口；沿用后端已有的"端口写 `.run\api.json`"约定，
  **不许自己编端口**。
- **Tools**（4 条，只读）：`search_prompts`、`search_character_cards`、`gallery_list`、`list_skills`。
- **Resources**（5 条，`comfyhub://` 形式）：`prompts/{id}`、`character-cards`、`recipes`、`memory`、`media/recent`。
- **实现纪律**：
  - `inputSchema` **从 `server-rs/src/tools/tools.rs` 的 `catalog` 派生**，不许手写第二份（R12）；
  - 工具 handler **只从 `ToolContext` 拿策略 / Skills / 记忆**，不读环境变量或全局单例；
  - 直接调用工具 handler，**不走 HTTP 回环**（少一跳、少一处鉴权面）。

**S2（写操作）前置检查**：先逐条核 `ToolPolicy.resolveWrite()` 与 `ToolApprovalGate`
对现有写工具的覆盖范围，**再**决定对外开哪几个；每个开放的写工具都要有
"外部调用 → Flutter 弹审批 → 拒绝则库里无新行"的用例（R8 语义一个字不改）。

---

## 5. 路线图与判据

| 阶段 | 做什么 | 判据（可测） |
| --- | --- | --- |
| **S1** | 只读 MCP Server（§4） | DSH 挂上后 `tools/list` 列得出、`gallery_list` 返回真实 `mediaId`；**0 写、0 上游请求** |
| **S2** | 前置检查 + 带闸门的写工具 | 每个开放的写工具一条审批用例；拒绝 → 库里无新行 |
| **S3** | 产物回流：`source=comfy-agent` 标记 + `capture_runs.raw` 补齐 | 官方出图 1 分钟内进画廊，且工作流可复现 |
| **S4** | 技能导出器（剥 `always`/`whenToUse`/`version`，`description` 压成单行） | 导出文本粘进官方对话能保存、下一条消息生效 |
| — | ~~`agent.json` 探测~~、~~技能导入（官方 → 我们）~~ | 降级/暂缓：前者收益小于一条状态条成本；后者官方**没有分发通道**，做了也只是个便利按钮 |

---

## 6. 未核实 / 不要当已知用

| # | 未知项 | 影响 |
| --- | --- | --- |
| 1 | **local agent 二进制现在能不能拿到** —— comfy-cli 只管权限不装 agent；Desktop 是运行时下载（`node_fetch_*` / `package_missing` / `install_hint`），来源与版本不在任何公开文件里 | 决定第 4 条探索的起点 |
| 2 | **`/api/agent/*` 有没有长期契约** —— 类型公开，但无 API 文档、无版本策略、无兼容承诺 | 决定"不接"这条要不要重新评估 |
| 3 | **`auto_limited` / 5 并发之外有没有全局限流** | 只影响成本估算 |
| 4 | **单次跑的真实总成本**（token + 它触发的 GPU 时间）—— 现有数字来自第三方对前端 fixture 的测算（约 181 credits≈$0.86 一次 build-and-run），不是账单 | 只影响成本估算 |
| 5 | **loopback 到底算不算 egress 例外** —— never-grantable 清单里点名了 loopback，但白名单里又有 ComfyUI；`AGENT_EGRESS_ALLOW_HOSTS` 是否预置 loopback 未核实 | 决定第 4 条还有没有戏 |

---

## 7. 附：一处需要连带更新的旧文档

[`comfy-mcp-report.md`](archive/comfy-mcp-report.md) 卷首的"最新决定（2026-09-21）"结论仍然成立
（接官方 comfy-mcp、借它的能力），但**它的前提是"MCP 能覆盖 Agent 能做的事"** —— 现在已经不成立：
Comfy Agent 与 MCP 是两条独立产品线。本文与该报告不冲突，但**引用前先看 §2.1**。
