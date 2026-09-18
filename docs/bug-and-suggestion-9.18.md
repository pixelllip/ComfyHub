bug:

* ~~appbar没有及时更新当前对话的消息数量~~ → **已修**（2026-09-18）
  （根因：标题栏读的 `conversation.messageCount` 只有拉会话列表时后端才会给；
  发出去的消息与流式回复都是本机乐观插入的，所以那个数字一直停在"打开这条会话时"的值。
  现在每次消息列表变化就把当前会话的计数对齐到本机消息数 —— `AiWorkspaceStore._syncMessageCount()`；
  回归用例 `ai_tools_ui_test.dart` "AppBar 的消息数跟着对话实时变"）
* ~~仅能选择3行文本~~ → **已修**（2026-09-18）
  （根因：`MarkdownText` 把一段回复解析成很多块，每块一个 `SelectableText` —— 每块都是独立的选择域，
  鼠标拖到当前段末尾就拉不过去了，短段落正好三行。现在正文一律用 `Text`/`Text.rich`，
  整条气泡（含思考段与工具卡）包在**一个** `SelectionArea` 里，可以跨段连续拖选；
  回归用例 `ai_tools_ui_test.dart` "一条回复只有一个选择域"）
* ~~“krea2 卡在一个硬事实上：那条工作流在本机库里没有任何捕获记录，而 comfy\_submit 只认库里的 promptId——我不能凭一个文件路径提交。”~~ → **已修**（2026-09-18）
  （新增 `comfy_load_workflow`：读一份本机工作流 .json → API 格式原样用；**界面格式（nodes/links）
  按 ComfyUI 的 `/object_info` 转成 API 节点图** → 入库拿 `promptId` → 正常提交；
  `comfy_submit` 也接受 `workflowPath` 一步到位。转换不出来的（`Anything Everywhere` 这类纯前端节点）
  **如实报 `UNSUPPORTED_NODES`** 并给出两个出口：在 ComfyUI 里「导出（API）」一次，或点一次 Queue 让它被捕获。
  防线：`WorkflowConvertTest` 13 例 + `e2e-submit-test.ps1` 第 4 幕 11 项断言）
* ~~“read file / PATH DENIED / 失败”——应该自动发现comfy目录，给comfy的目录默认白名单，而不是现在的写死在代码里的本机目录（是吗？）~~ → **已修，而且你的判断是对的**（2026-09-18）
  （诊断确认：读白名单出厂只有 `<项目根>\comfyui` + `<项目根>\storage`，而**用户的 ComfyUI 根本不在项目里**。
  新增 `ComfyRoots`：每次现探本机 ComfyUI 目录 —— 环境变量 → 用户配的产物目录的父目录 → 项目内 `comfyui`，
  再在它们的兄弟目录里**有界**搜索（ComfyUI Desktop 把程序与共享数据分家：本机实例的工作流就在
  `D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI\user\default\workflows`，只按配置的输出目录是推不到的）。
  探测结果作为**只读**白名单交给 `ToolPolicy`，**写仍然只允许 `<根>\comfyui`**；不落库、每次现算，
  所以换机器 / 换安装位置都不用改配置。设置页「AI 工具权限」新增一张「自动放行（只读）」卡，只展示不编辑。
  防线：`ComfyRootsTest` 5 例 + `ToolPolicyTest`"自动发现的目录只放宽读、不放宽写"）
* 更多bug可以调研目前最新的软件对话记录 → **调研了**（2026-09-18，库里的最新会话
  `双模型参考图生图提示词` + `ai_tool_calls` 全量），找出一条真 bug 并修掉：
  **`comfy_get_run` 查不到刚提交完的那次运行** —— `comfy_submit` 的结果里同时有
  `promptId`（库里的提示词）/ `comfyPromptId`（ComfyUI 的 UUID）/ `capturedPromptId`（捕获记录编号），
  模型（正确地）拿数字那个去查，而查询入口当时只认 runKey（UUID）→ 直接 `NOT_FOUND`，
  表现就是助手说"图我这边看不到"。现在结果里显式给出 `runKey` 与 `idHint`，查询入口**数字与 UUID 都认**。
  * 另外两条不算 bug、但值得记一笔：① 这次对话里工具调用 16 次撞上 `maxCallsPerRun` 上限
    （最后那个 `load_skill` 被拒）——上限本身是设计，但"投递附件 + 查节点 + 查工作流"确实很容易撞满，
    设置页可以按需调大；② `krea2SFWNSFWUncensoredImageTo_v10.json` 是一份 **113 节点、含
    `Anything Everywhere` / `SetNode` / `GetNode` 的界面格式工作流**，正是本轮转换功能刻意
    "宁可失败也不猜"的那一类（见上一条的两个出口）。
* ~~**特定情况下，一条消息会复制一遍再发送**~~ → **已修**（上一轮，2026-09-18；本轮复核仍全过）
  （两处根因：① 输入框是"先清空、再由 `send()` 拒绝"，"正在生成中按回车"会把用户打的字静默吃掉；
  ② 会话草稿在 `startRun` 之后才清，而"发送中"那次通知在它之前，页面的"取回草稿"正好把刚发出去的原话
  灌回输入框。现在：准入判断同步化（`sendBlockReason()`，被拒时不动输入框）、草稿在第一次通知前作废、
  用户打的字一律认领到当前会话；回归用例 4 条，见 [`ai-home-progress-v0.1.md`](ai-home-progress-v0.1.md) §4.13）

建议：

* ~~“生成的产物”包括生成的工作流~~ → **已做**（2026-09-18）
  （回复末尾那张「生成的产物」卡原来只有缩略图。现在工具结果里的 `capturedPromptId`
  ——**本次真正入库**的那份提示词，含改过的参数——也带进界面，卡上多一个「查看工作流」按钮，
  直接打开这次真跑过的工作流；标题也如实写成"N 个 · 含 M 份工作流"。
  防线：`ai_tools_ui_test.dart` "「生成的产物」里也含生成它的那份工作流"）
* ~~查看工作流功能，可以效仿comfy内部那样展开成蓝图显示出来（难度比较大，先调研）~~ → **调研完成**（2026-09-18）
  （见 [`workflow-blueprint-research.md`](workflow-blueprint-research.md)，本轮**只调研、未动代码**。
  结论摘要：**分两阶段**——A 先做结构化大纲/树视图（1~1.5 人日）；B 再做 `CustomPainter` +
  `InteractiveViewer` 的**只读**蓝图画布（3.5~5 人日，UI 格式有 `pos`/`size`/`links`，可以忠实还原）。
  **不建议**内嵌 litegraph.js / ComfyUI 前端（Flutter Windows 没有 DOM；那套前端依赖服务端 API +
  WebSocket，不是可复用库）。硬约束：本机库里 13 份工作流中**只有 3 份是 UI 格式、10 份是 API 格式**，
  而 **API 格式没有坐标** —— 那段布局只能是猜测，界面上必须标注。）

- krea2 那条工作流本身仍然不能自动转换 —— 这种问题，能放权给AI自动处理吗？
- AIH-002（900~1199px 宽度下 Comfy 状态整块不可达）——这是什么意思，另外本机开了1.5倍缩放，会不会对宽度判定有点影响？
- 能让AI记录长期记忆吗？（但是AI的自动记录应当由规则限制，不然条数太多，到时候想查找、改动、删除会比较困难）
