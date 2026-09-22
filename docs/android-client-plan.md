# 安卓客户端（ComfyHub Mobile）：架构预留与落地顺序

> 2026-09-21 用户提出"近期开工 ComfyHub 安卓客户端，需要做出一些预留"，并定了两条取向：
> 1. **已经设计好的工具可以留在移动端**（生成提示词、查询网页这类**不涉及本地工作流**的）；
> 2. **移动端要和桌面端通信，由桌面 App 再去调 comfy mcp** ——
>    即"本地工作流能力永远在桌面那一侧"。
>
> 这份文档是**设计 + 现状核对 + 分步计划**，不是已完成的东西。每条"现状"都标了代码出处。

---

## 1. 一句话架构

```
┌──────────────────┐        局域网 / USB 端口转发        ┌───────────────────────────────┐
│  Android 客户端   │  ───────────────────────────────▶  │  桌面 App + Kotlin 后端 :8080  │
│  （Flutter）      │        REST + SSE（既有协议）        │  （真正的能力宿主）             │
└──────────────────┘                                     └───────────────┬───────────────┘
                                                                         │ stdio (JSON-RPC)
                                                                         ▼
                                                              ┌─────────────────────┐
                                                              │ 官方 comfy-mcp 子进程 │
                                                              │  + 用户的 ComfyUI    │
                                                              └─────────────────────┘
```

**核心判断：移动端不直连 ComfyUI，也不装 Python。** 理由不是"做不到"，而是：
- ComfyUI、`comfy-mcp`、工作流文件、模型都在**桌面那台机器**上（那是唯一有显卡的机器）；
- 产物入库（画廊 / NSFW 标记 / `promptId` 绑定）是我们自己的链路，必须走桌面后端；
- 移动端一旦自己起 MCP，就会变成"第二份需要装配的运行时"，与发布包便携性冲突（见 `comfy-mcp-report.md` §4.1）。

---

## 2. 现状核对（哪些已经就绪、哪些是硬缺口）

| 需要的东西 | 现状 | 出处 |
| --- | --- | --- |
| 移动端要调的 API | ✅ **已经有了**：AI 工作台那套 REST + SSE 与桌面 App 用的是同一套（会话、Run、工具审批、画廊、Skills、设置） | `server/src/main/kotlin/com/comfyhub/ai/AiRoutes.kt`、`Application.kt` 的路由注册 |
| 局域网可达 | ⚠️ **默认不可达，但开关已有**：默认只绑 `127.0.0.1`；`COMFYHUB_ALLOW_REMOTE=1` 时绑 `0.0.0.0` | `Config.kt:29/53-57`（注释里就写着"远程/移动端场景"） |
| CORS | ✅ 可配：`COMFYHUB_CORS_ORIGINS`（逗号分隔） | `Config.kt` 的 `corsOrigins` |
| **鉴权** | ❌ **完全没有** —— 整条 API 无 token / 无会话 / 无来源校验 | 全仓 grep `Authorization` 只命中**出网**（Civitai / 上游模型），入站没有 |
| 移动端构建 | ⚠️ `android/` 是**同一份 Flutter 工程的安卓 target**（AGP 9.4 + 内建 Kotlin） | `AGENTS.md` §4、`docs/android-agp9-builtin-kotlin-migration.md` |
| 工具分层（谁能出现在移动端） | ❌ 现在**没有**"这个工具属于哪个端"的概念：`ToolCategory` 只分 skill/files/comfy/memory/web/ask | `ai/tools/ToolModel.kt` |

### ⚠ 硬缺口：**开放局域网之前必须先有鉴权**

这不是"以后再补"的洁癖，是**现在就有风险**：
- 后端持有用户的 **AI Provider API Key**，能代替用户**花钱**（`CredentialService` 存 DPAPI 加密值，但 API 本身能触发调用）；
- 后端能**读写文件**、能**提交 ComfyUI 任务**、能**调用 MCP 工具**；
- 一旦 `COMFYHUB_ALLOW_REMOTE=1`，同一个 Wi-Fi 下任何设备（包括别人的手机）都能直接调这些接口。

所以落地顺序里**鉴权排在"打开远程"前面**（见 §4 第 1 步）。

---

## 3. 工具怎么分端（这是预留的核心）

按"这个工具**碰的是哪台机器的资源**"来分，而不是按功能分：

| 工具 | 放移动端？ | 为什么 |
| --- | --- | --- |
| `web_search` / `web_fetch` | ✅ 移动端 | 我们的 `web_search` 打的是固定的 Keenable 服务，**与桌面资源无关**；移动端自己做还能省一次局域网往返 |
| `civitai_search` | ✅ 移动端 | 同上（打 civitai.com / civitai.red） |
| `remember` / `list_skills` / `load_skill`（**读**） | ⚠️ 看数据在哪 | 长期记忆与 Skills 现在都落在**桌面的 `storage`** 里。要让移动端用，得先决定"是否同步到移动端本地" |
| `register_skill` / `delete_skill` / `write_file` / `list_dir` / `read_file` | ❌ 桌面 | 写的是桌面磁盘（且 `write_file` 只允许写 `<根>\comfyui`） |
| 全部 `comfy_*` | ❌ 桌面 | 产物要进桌面画廊；ComfyUI 在桌面 |
| 全部 **comfy-mcp** 工具（39 个） | ❌ 桌面 | 它们操作的是桌面的 ComfyUI 与磁盘 |
| `ask_user_question` | ⚠️ 两端都可能发起 | 提问是"人机通道"，哪一端跑 Run 就在哪一端问 —— 但需要桌面把 pendingQuestion 同步过去（见 §4 第 4 步） |

**已经设计好的提示词类能力**（`anima-*` 那批 Skill）属于"纯文本生成"，用户可以在移动端直接跑 ——
前提是**那一次 Run 在移动端发起、并由移动端的模型完成**，只调用上表里 ✅ 的工具。
这就是"把不涉及本地工作流的工具留在移动端"的确切含义。

---

## 4. 分步落地（每步一个提交，从"不破坏桌面"开始）

### 第 1 步：入站鉴权（**必须先做**，否则不许开远程）

> **完整设计见 [`inbound-auth-design.md`](inbound-auth-design.md)**（威胁模型、回环豁免、媒体签名 URL、
> 接口形状、三阶段落地、防线与风险登记）。下面只留要点，且已按该设计**修正两处**：

- 后端加一个**配对令牌**：首次在设置里"允许移动端连接"时生成一个随机 token（存库、界面可见、可轮换），
  所有**非回环来源**的请求必须带 `X-ComfyHub-Token: <token>`；不带或不匹配一律 `401`
  （绑了 `0.0.0.0` 却没启用令牌时，非回环请求一律 `403` —— **默认拒绝**，不许裸奔）。
- **默认关闭**：没有 token 时行为与现在**逐字一致**（回环 + 无鉴权）—— 保证桌面 App 不受影响。
- **回环默认豁免**（`COMFYHUB_AUTH_LOOPBACK_EXEMPT=0` 可以关掉）：桌面 App 的媒体加载
  （`NetworkImage` / 视频 / 交给系统程序打开）**根本带不上请求头**，本机脚本与 ComfyUI 节点也不带任何头；
  豁免它们，这些调用方一行都不用改。
- **修正 ①**：SSE 用 header 就能覆盖 —— 我们的客户端是 `http.Request` + 手工分帧
  （`lib/core/ai_api_client.dart:434-471`），**不是 `EventSource`**，所以"必须走 `?token=`"这个前提不成立；
  查询串方案仍然保留（给浏览器端 / `curl` 用，以及下面的媒体签名）。
- **修正 ②**：媒体（`/api/media/*/file|thumb|poster`、`/api/ai/attachments/*/file|thumb`）**不能**靠请求头 ——
  这些 URL 是被 `Image.network` / `NetworkImage` / 播放器 / 外部程序直接取的。
  做法是**短期签名查询串**（`?t=<HMAC>&e=<exp>`，TTL 5 分钟，长期令牌永不进 URL），放在 P1。
- 防线：纯函数单测（决策抽成 `decide(...)`，项目现在没有 HTTP 层测试）+ 一条 e2e
  （"不带 token 的非回环请求 401、带上就通过、回环完全不校验"；非回环在本机用**网卡 IP** 造）。

### 第 2 步：把"端"这个概念写进工具层

- `ToolCategory` 只解决分组，不解决"谁能用"；新增一个**工具归属**概念（例如 `ToolScope.DESKTOP / MOBILE / BOTH`），
  由 `ToolRegistry` 在**生成下发给模型的清单**时按"本次 Run 由哪个端发起"过滤。
- 这不是安全边界（安全边界仍是 `ToolPolicy`），而是"别让移动端的模型看见它调不到的工具"。
- 防线：`ToolRegistryTest` 补一例"移动端 Run 拿不到 comfy/mcp 工具"。

### 第 3 步：移动端接入既有 API

- Flutter 侧复用现有的 API 客户端（现在桌面 App 用的那套 REST/SSE 封装），
  差别只有 baseURL 与鉴权头；**不要**为移动端另写一份协议（AGENTS §5 的同一条纪律）。
- 移动端首期只做：会话列表 / 对话 / 提示词类 Skill / 联网工具 / 画廊**只读浏览**。
- 需要用户确认的点：移动端要不要本地缓存会话（离线看历史）？——这会影响数据模型。

### 第 4 步：跨端接力（真正"和桌面通信"的地方）

- **Run 归属**：一次 Run 在哪一端跑，就由哪一端持有流；另一端能看到"桌面上正在生成"并接收结果通知。
- **审批与提问**：用户在手机上也能点"批准/拒绝"、也能回答 `ask_user_question`
  （现在 `pendingQuestion` 是前端本地状态，跨端要落库 + 事件广播）。
- **产物**：桌面生成的图，移动端按 `mediaIds` 取缩略图（后端已经有 `thumbUrlFor`）。

### 第 5 步：局域网发现（体验项，可以最后做）

- 桌面后端在 `_comfyhub._tcp`（或自定义服务名）上广播自己的地址与端口，移动端不用手输 IP。
- 二维码配对（把 baseURL + 一次性 token 编成二维码给手机扫）能省掉"手输 IP + 抄 token"这一步。

---

## 5. 明确不做的事（免得以后反复讨论）

- **不做浏览器自动化**（与本项目一贯立场一致，见 AGENTS §10 与 `##模拟用户行为##` 那一节）。
- **不让移动端装 Python / MCP**（§1 的三条理由）。
- **不为移动端另写一套后端**：`storage`、数据库、画廊、Skills 的真源永远在桌面那一侧。
- **不把 `COMFYHUB_ALLOW_REMOTE=1` 当成默认**：它是显式开关，且必须与配对 token 同时生效才允许非回环访问。

---

## 6. 与 MCP 接入的关系

移动端**不需要**知道 comfy-mcp 的存在 —— 它调的是"桌面的 `/api/ai/...`"，
桌面后端在需要时自己把工具调用转给 MCP 子进程（已实现，见 `docs/comfy-mcp-report.md` §6 与
`server/src/main/kotlin/com/comfyhub/ai/mcp/`）。这条隔离是有意的：

- 移动端不必关心 MCP 的 Python 运行时、工具清单版本、stdio 生命周期；
- MCP 换版本 / 换实现（甚至以后换成 COM 直接调 ComfyUI）时，移动端一行都不用改。
