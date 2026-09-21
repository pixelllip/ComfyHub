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
