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

* ~~"**我需要在本项目进程存活的时候，产生新工作流运行的记录才会入库**，这不对吧"~~ → **已修，你说得对**（2026-09-18）
  （现场：你说"基于 `krea2SFWNSFWUncensoredImageTo_v10` 生成"，AI 回"库里搜不到这条工作流，
  要么把 `.json` 路径发我、要么你在 ComfyUI 里点一次 Queue 让它被自动捕获" —— 而那份文件
  **一直躺在磁盘上**：`…\ComfyUI-Installs\ComfyUI\ComfyUI\user\default\workflows\krea2…v10.json`。
  根因是库的来源：`prompts` 只有两个入口 —— 自动捕获（只收"本进程活着的时候跑过的"）与
  手动按路径读，所以**首次使用时库里必然是空的**。三处一起改：
  ① 新增 `comfy_list_workflows`（只读）列出本机 ComfyUI 已保存的工作流文件，
  `comfy_find_workflow` 在库里搜不到时**自动带上**它们（`localWorkflows`，带 path）；
  ② `NOT_FOUND` 兜底文案与系统提示 **v13 第 16 条**给出正确顺序
  （`comfy_find_workflow` → `comfy_list_workflows` → `comfy_load_workflow(path=…)` → `comfy_submit`），
  并**明确禁止**再说"得先在 ComfyUI 里跑一次才会被捕获"；
  ③ 设置页 →「ComfyUI 自动捕获」加一颗**「导入本机工作流」**按钮
  （`POST /api/capture/import-workflows`）把目录里的工作流批量读进库，幂等
  （`run_key = file:<sha256>`，与 `comfy_load_workflow` 同一把钥匙）；转换不了的那份也照样入库、
  如实标 `runnable=false`。另有只读的 `GET /api/capture/workflows?q=`。
  防线：`ComfyWorkflowFilesTest` 7 例 + `ToolRegistryTest` 3 例（回落 / `NOT_FOUND` 指路 / 只读 allow）
  + `e2e-submit-test.ps1` 第 6 幕 6 项断言。**本机实测**：11 份工作流全部列得出来，
  `krea2SFWNSFWUncensoredImageTo_v10.json` 就在里面）
* ~~代码块里的内容超过宽度只能横向拖~~ → **已改**（2026-09-18）
  （`lib/widgets/markdown_view.dart`：**不含换行符**的代码块改成软换行 —— 超过容器宽度就折行显示，
  必要时连超长单词也拆开；**多行**代码块**保持横向滚动**：缩进 / 对齐 / 表格状输出是有意义的排版，
  自动折行反而更难读。回归用例 `test/markdown_test.dart` 两条：单行折行 + 多行仍可横滚）

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

- ~~krea2 那条工作流本身仍然不能自动转换 —— 这种问题，能放权给AI自动处理吗？~~ → **能，而且已经做成两条腿**（2026-09-18）
  （**先查了那份文件**：113 节点里真正卡住的是 2 个 `Anything Everywhere` + 一堆 rgthree 纯前端节点
  + 6 个 **UUID 组节点**；同时发现一个好消息 —— 这份文件里有 `widgets_values_named`（参数名写全了），
  而旧转换器只读位置式的 `widgets_values`。
  于是按你选的"双管齐下"：
  **① 确定性增强**（不用猜的部分自己扛下来）：
  `widgets_values_named` 优先（只认 `/object_info` 声明过的键，避免版本不一致塞进未知参数）；
  **组节点按 `definitions.subgraphs` 展开** —— 内部节点重新编号（内部 id 会与主图撞号）、
  内部连线整表重编、外层控件值直接填给内部输入、实例输出穿透到内部真正的产出节点；
  旁路实例（krea2 里 34 / 110）不展开，交给原有的旁路逻辑按类型接过去。
  **② 放权通道**（剩下的交回给 AI，但只交"接线"这件事，不交"猜语义"）：
  `comfy_load_workflow(tolerateUnsupported=true)` 把转换不了的节点摘掉，并给出**缺口清单** ——
  `openInputs`（哪些连线型输入空着、缺什么类型）/ `unresolvedInputs`（哪个输入还悬着）/
  `unsupportedNodes`（摘掉了哪些类），结果标 `runnable=false`；
  AI 照着清单用 `comfy_submit(promptId=…, connections={"8.vae":["1",2]})` 补线再提交。
  线接错了 ComfyUI 自己的校验会当场拒（`node_errors`，不入队）—— 比服务端瞎猜一个语义安全得多。
  用本机那份真实文件实测过：6 个组节点里 4 个被展开（另 2 个是旁路）、
  18 条悬空输入全部点名、111 个节点转出来结构自洽。
  防线：`WorkflowConvertTest` 20 例 + `WorkflowEditTest` 13 例 + `e2e-submit-test.ps1` **第 5 幕**
  （真读一份带 `Anything Everywhere` 的工作流 → 断言 `runnable=false` 与 `openInputs` →
  按清单补线 → 断言提交出去的图里 `8.vae=[1,2]` 且被摘掉的节点没混进去）
  顺带修掉一个真 bug：**提示词被删掉之后，同一份工作流文件再也导不进来**
  （`run_key = file:<sha256>` 是内容寻址的，删提示词只把 `prompt_id` 置空、`status` 还是 `success`，
  而 `success` 是"永不抢占"的终态 → 永远回 `LOAD_IN_PROGRESS`）。）
- ~~AIH-002（900~1199px 宽度下 Comfy 状态整块不可达）——这是什么意思，另外本机开了1.5倍缩放，会不会对宽度判定有点影响？~~ → **已修 + 已解释**（2026-09-18）
  （含义：`ai_home_page.dart` 里 `wide = width >= 900` 决定"要不要三栏布局"，
  而第三栏（Comfy 状态）的门槛是另一个数 `width >= 1200`；右下角那个「ComfyUI 状态」FAB
  又只挂在 `< 900` 的分支上。于是 **900~1199px 这一段两边都不占**：既没有第三栏、也没有 FAB，
  Comfy 状态整块内容在界面上一个入口都没有。你选的处理是**保留两栏**（900px 宽时再塞一栏，
  正文区只剩约 400px），改成在**标题栏补一个图标入口**，只在 900~1199px 出现。
  关于 1.5 倍缩放：**Flutter 的逻辑像素已经除过 DPI**，所以缩放确实会吃掉可用宽度 ——
  150% 缩放下 1920px 的屏幕只有 1280 逻辑像素、1600px 的只有 ~1067（正好掉进那个死角）。
  但这**不是判定的 bug**：界面本来就该按"还剩多少逻辑像素"排版，真问题是那段没入口，已修。
  回归用例 `ai_home_test.dart`「900~1199px：第三栏铺不开，但 Comfy 状态仍然可达」）
- ~~能让AI记录长期记忆吗？（但是AI的自动记录应当由规则限制，不然条数太多，到时候想查找、改动、删除会比较困难）~~
  → **本来就能，这轮把"规则限制"补齐了**（2026-09-18）
  （长期记忆 M6 早就做了：`remember` 工具 + 右侧栏「长期记忆」面板，真源是
  `<storage>\ai\memory.md`（一行一条、人能手改），每次 Run 现读并注入系统提示。
  这轮按你说的加了三道闸：① **总量硬上限 100 条**，满了 `remember` 报 `MEMORY_FULL`
  并**请用户去界面清理** —— 不静默丢、不挤掉最旧的一条，也不许模型自己删改你的条目；
  ② **一次回复最多新增 4 条**（判重命中的"又说了一遍"不算，不占额度）；
  ③ AI 写的每条自动带 `[yyyy-MM-dd]` 前缀（你手敲的不带），判重会忽略这个前缀。
  界面也从"一个整篇文本框"改成**列表视图**：搜索框过滤 + 每行一个删除 + 勾选后批量删除
  （删除前确认），「整篇编辑」仍然保留。面板上显示 `N / 100 条`。
  防线：`MemoryStoreTest` 14 例 + `ToolRegistryTest`「一轮最多新增 N 条」+
  `ai_tools_ui_test.dart`「能按关键词搜、单条删与批量删」；系统提示升到 **v12**）
- ~~AI 工作台左侧的历史对话列表套一个 padding，并标注最近一次对话的时间戳~~ → **已做**（2026-09-18）
  （列表整体留出 8px 内边距 + 每行圆角（以前 ListTile 贴着窗口边缘画，选中底色从边铺到边，
  看着像一整条横幅）；副标题从"N 条消息"改成"**N 条消息 · 最近一次对话时间**"（`relativeTime`）。
  顺带修了两处会让这个时间戳不准的东西：① 后端给的是 `Instant.toString()`（UTC），
  前端没 `toLocal()` → 时间整体偏一个时差；② 本机乐观插入的新消息不会更新 `updatedAt`，
  刚聊完还写着"3 天前" —— 现在消息数一变就把时间戳改成当前时间并**把这条顶到列表最前**
  （后端也是按 `updated_at DESC` 排的）。回归用例 `ai_home_test.dart`
  「会话列表：显示最近一次对话时间，聊完立刻变成「刚刚」」）
