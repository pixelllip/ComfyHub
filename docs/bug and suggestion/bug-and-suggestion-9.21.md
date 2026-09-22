bug：

- ~~当前的 ask_user_question 功能不完善，首先它应该向用户抛出选项，点击对应选项做出选择，
  最末尾选项固定让用户决定（输入你的答案）；然后在调用 ask_user_question 时，
  将原来聊天详情下部的模块替换（输入框等），具体可以参考 deepseek harness 的做法。~~
  → **已修**（2026-09-21）
  （两件事分开做，都在**界面侧**：
  ① **末尾固定"自己输入答案"** —— 气泡里的提问卡不再自带输入框，改成"问题 + 一排选项行"，
  最后一行**永远**是「自己输入答案」（序号接着模型的选项往下数，点它把焦点交给底部那个框）；
  选项行点一下即答（走 `optionIndex`），要自己写就写在那唯一的输入框里。
  后端选项上限从 4 提到 5（`take(5)`），工具描述与系统提示都写明"界面会固定补这一条，
  你不用留'其他'这种选项"。
  ② **提问时接管输入区**（`_AskComposer`，参考 DSH 的 composer takeover）——
  `store.pendingQuestion` 非空时，聊天下部整块由普通输入框换成答题卡：
  问题 + 选项 + 最后一行自己输入（`Key('ask-answer-field')`，Enter 提交 / Shift+Enter 换行）
  + 「跳过这个问题」+「停止生成」；答完 / 跳过后 `pendingQuestion` 变 null，
  普通输入框**自动回来**。两条退路都留着，避免用户被"接管"卡住没有任何出口。
  防线：`test/ai_ask_question_test.dart` 4 例（接管与末尾固定项、点选项只带 optionIndex、
  自己输入带文本、跳过打到 `/skip`），另加 `store.pendingQuestion` 的取/清断言。
  之前的 `_AskCard` 是"卡片里也放一个输入框"，一屏两个框用户不知道该往哪儿写 —— 现在
  卡片只管**记录**（问过什么、答了什么），输入区只管**作答**。）

【2026-09-21 复现与二次修复（真机现场）】

第一轮改完重启后**仍然不接管**，用户给了控制台输出，据此查出**另外两个真根因**（都在真事件里）：

1. `user.question` 事件**不带 `callId`**（只有后端生成的 `questionId`），而提问卡的位置是按
   模型那条调用的 `callId` 排的（`tool.requested` 先到）。旧代码按 `questionId` 另开了一条实时调用，
   于是 `pendingQuestion` 在 `callId` 那条上找不到问题 → 输入区永不接管、工具卡一直停在"运行中"。
   修法：贴到已存在的那条 `ask_user_question` 调用上，并把调用号记进 `AiQuestion.callId`。
2. 后端把 `PendingQuestion` **平铺**在事件 data 顶层，`data['question']` 于是是**问题文字**
   （字符串），客户端读的是对象 → 解出 null。修法：后端改成嵌套形状，前端两种形状都认。

另外用户还撞到一个**更难查的**：那次提问被「停止」打断，库里留下一条"有 `tool_call`、
没有 `tool_result`"的助手消息，之后**整条会话怎么发都是 HTTP 400**
（"An assistant message with 'tool_calls' must be followed by tool messages..."），
界面显示成"上游网关不接受工具参数，本次已退回纯文本模式"。修法：`ToolCallHistory.repairOrphans`
在拼历史时给没有结果的调用补一条如实说明的占位结果。

界面体验（用户当场提的）：提问卡与底部答题区把整组选项画了两遍 → 气泡里那张卡**默认折叠**
成一条记录（与"已回答"卡片同高，答完不跳），点头部可展开；底部答题区的选项区内部滚动，
「自己输入答案」那行始终可见。

判据：`test/ai_ask_question_test.dart` 9 例（含"按真事件逐字重放"的假 SSE）、
`ToolCallHistoryTest` 4 例。教训记一笔：**假 SSE 的形状与顺序必须照着真事件抄** ——
第一版用例少了 `tool.requested`、payload 也是错的形状，所以两个根因一个都没抓住。

建议：

- ~~加入 ##模拟用户行为## 模式，面对一些仅面向前端的工作流节点，让 AI 有能力在前端读取、
  编辑、在 comfy 前端提交。~~ → **已做（走后端 + ComfyUI HTTP API，不做浏览器自动化；
  2026-09-21 与用户当面确认过这个范围）**
  （要解决的问题：ComfyUI 的官方前端在排队前会调用各扩展的 JS 钩子改写节点图
  （`graphToPrompt()` 之后、`POST /prompt` 之前），那份 JS 在**浏览器**里，服务端拿不到，
  所以过去只能对 `Anything Everywhere` 这类节点如实报 `UNSUPPORTED_NODES`。
  "模拟用户行为"就是**把前端排队前会做的那几件事，在后端等价地做掉**，
  判据一律保守、做的事全部如实回报：
  ① **广播型节点**（`Anything Everywhere` 一族，含 `?` / `!` / `Prompts` / `Seed` / 数字后缀变体）
  —— 按它自己 `properties` 里声明的 `Input Type` / `Input Name` 过滤，把上游那一路灌进全图
  **同类型且空着**的连线输入（已经接好的、说不清类型的一律不动）；
  ② **纯显示节点**（`Preview*` / `ShowText` / `Display Any`…）丢掉（官方前端也不把它们写进提示词）；
  ③ **一个输出节点都没有**时补一个 `SaveImage`（接在真正产出 `IMAGE` 的节点上，前缀 `ComfyHub`）。
  开关是 `comfy_load_workflow.emulateFrontend`（**默认开着**；`false` = 一个字都不改、
  只如实报缺口的保守档）。改了什么逐条回给模型与用户：
  `frontendEmulated` / `frontendBroadcast` / `frontendInjected` + 一句 `frontendHint`，
  系统提示第 15 条（`v22`）要求**必须把这些改动转述给用户**，别默默替他把图改了。
  说不清类型的广播**不猜**，仍然落进 `unsupportedNodes`（出口还是老那套：
  `tolerateUnsupported=true` + `connections=` 补线，或让用户在 ComfyUI 里导出 API 格式）。
  防线：`FrontendNodesTest` 5 例（不开开关时行为与过去**逐字一致**、只灌同类型空输入且不碰已接好的、
  灌完重算 `openInputs`、说不清类型要报错、显示节点丢掉 + 补 `SaveImage`、已有输出节点时不补）。
  **明确不做**：浏览器自动化（不做 Playwright 之类驱动 ComfyUI 网页界面）；也不实现
  rgthree 那类"只有界面逻辑"的特殊节点（说不清语义就不猜）。）
- 把目前已有的skill整理成技能树，后续加skill进来用技能树管理和使用？
- 新增对生成角色的提示词约束skill（占位符，哪天我想起来再实装）
- 接入comfy mcp，我们项目里和mcp重复的功能就可以删掉了
  → **决定：接（2026-09-21 晚，用户拍板）**；查证报告见 [`docs/comfy-mcp-report.md`](../../docs/comfy-mcp-report.md)。
  （先查证后决策：拉了官方本地服务端源码读（`Comfy-Org/comfy-mcp` v0.10.0，HEAD `e5f768d`，40 个 `@mcp.tool()`），
  当时的报告结论是"不整条接"（要 Python+comfy-cli、我们的入库链路删不掉、安全面更大）；
  用户随后给出了依赖问题的解法并拍板**用它的**："官方 mcp 它覆盖的功能比我们更多更全，用它的。
  comfy 没有自动捆绑 comfy mcp 下载，但是我们可以做脚本，在启动时运行脚本检测下载。"
  落地四步（每步一个提交）：
  ① ✅ **MCP 运行时装配**（提交 `8f0ca60`）：`scripts\mcp-runtime.ps1` 把便携版 Python 3.12 embeddable
     + pip + `comfy-cli 1.20.0` + `comfy-mcp 0.10.0` 装进 `<根>\storage\ai\mcp`（首次约 300MB），
     并把 comfy-cli 的工作区 `comfy set-default` 指向用户真实在用的 ComfyUI；
     `comfyhub.ps1 up` 自动补（已就绪时只有几次文件检查）、`doctor` 单独报一段；
     顺带补了 **ComfyUI Desktop 新版嵌套路径**的探测（`D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI`，
     `comfy-path.ps1` 与 `ComfyLocator.kt` 两边同步）。
     → **随后按用户要求改成"检测优先，安装只作兜底"**（用户："环境有 python 你的脚本就不用再装了，
       同理 jdk、mcp 也是"）：已有的（state.json / venv / 便携版）→ 借机器上**已装好 comfy-mcp** 的 Python →
       借机器上的 Python 建 **venv**（不污染用户的全局 site-packages）→ 真的没有才下便携版。
       `state.json` 记 `pythonSource`，`doctor` 打出来；`COMFYHUB_NO_DOWNLOAD=1` 时**不许**下便携版。
       Java 侧同样加了 `JAVA_HOME` 与更宽的 JDK 探测（机器上有 ≥21 就直接用，`Ensure-JavaRuntime` 也认这个开关）。
       同时修掉参数改名留下的**静默失效**：`-Proxy` / `-Force` 在 mcp-runtime.ps1 里被 PowerShell
       丢进 `$args`（简单函数不报错），改成真名 + 给文件级参数加 `Alias('Proxy'/'Force')`。
  ② ⬜ 后端 MCP stdio 客户端（initialize → tools/list → tools/call）
  ③ ⬜ MCP 工具注入 `ToolRegistry` + 逐个定权限档，重复的 `comfy_*` 让位（产物入库/审批/NSFW 链路保留）
  ④ ⬜ 安卓客户端预留：桌面 App 当能力宿主，移动端只留不依赖本地工作流的工具）
- 近期开工 ComfyHub 安卓客户端：需要预留（用户 2026-09-21 晚提出）
  → **待做**。已经设计好的工具可以留在移动端（生成提示词、查询网页这类**不涉及本地工作流**的）；
  与桌面端通信的设计取向是：**移动端 → 桌面 App → 桌面 App 再调 comfy mcp**，
  即"本地工作流能力永远在桌面那一侧"，移动端不直连 ComfyUI 也不装 Python。
- 设计入站配对令牌鉴权（开远程的前置条件）（用户 2026-09-21 晚提出）
  → **设计完成（未实施）**：见 [`docs/inbound-auth-design.md`](../../docs/inbound-auth-design.md)。
  一句话：默认完全关闭（回环 + 无鉴权，行为与今天逐字一致）；打开后**非回环来源**必须带
  `X-ComfyHub-Token`，回环默认豁免（否则桌面 App 的 `NetworkImage` / 视频 / 外部打开那 11 处取字节
  根本带不上头、本机脚本与 ComfyUI 节点也一个头都不带）；绑了 `0.0.0.0` 却没启用令牌 → 非回环一律 `403`。
  媒体走**短期签名查询串**（P1），SSE 走 header 即可（我们的客户端不是 `EventSource`）。
  落地分三步（P0 闸门 / P1 媒体+移动端 / P2 二维码配对），防线是**纯函数单测** + 一条新 e2e
  （项目现在没有任何 HTTP 层路由测试，也没有 `ktor-server-test-host` 依赖）。
