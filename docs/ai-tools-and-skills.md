# AI 工具与 Skills（M4 / M5）设计与实现说明

> 2026-09-16 起实施。对应需求：AIH-021/022/024（事件协议与取消）、AIH-033~036（Comfy 工具与轮数限制）、
> AIH-037~045（Skills 与安全）、AIH-046/047（提示词与注入防护）、AIH-049（工具调用状态卡）、AIK-002（效仿 DSH）。
>
> 配套调查记录：[`dsh-tool-layer-report.md`](dsh-tool-layer-report.md)（对 DeepSeek Harness 工具层的逐项实测）。

## 0. 一句话

**AI 工作台现在真的会调工具了**：模型流里出现工具调用 → 后端按权限策略执行 → 结果作为 `tool` 轮喂回去 →
直到模型给出正文或到达轮数上限；Skills 落在磁盘上，AI 说一句「记住这个流程」就能注册，用户在右侧栏能删，
**不需要重启应用**。

## 1. DSH 的工具里，我们抄了哪些、不抄哪些

DSH 暴露给模型的东西分五类。对照结论（`dsh-tool-layer-report.md` §1）：

| DSH 工具 | 我们是否采纳 | 说明 |
| --- | --- | --- |
| `read` / `write` / `edit` / `glob` / `grep` | **部分采纳** | 采纳"文件工具"这一类，但**收敛成 4 个**：`read_file` / `list_dir` / `write_file`（+ skills 专用写工具）。没有 `edit`（字面量替换）与 `glob`/`grep`（需要 ripgrep 二进制）——首期用不上，等真的有"改 ComfyUI 配置"这类需求再加 |
| `read_image` | 不采纳 | 我们的多模态入口是附件事务（M3），不是工具；工具里塞图片会把 token 账算乱 |
| `pwsh` / `bash` | **明确不采纳** | AIH-045 与实施方案 §2.3 都排除了 shell；"能跑命令"等于把权限模型绕过去了 |
| `subagent` / `subagent_fork` / `send_message` / `interrupt_agent` / `list_agents` | 不采纳 | 我们是单模型对话产品，不是多智能体编排器 |
| `workflow` / `ralph` | 不采纳 | 同上 |
| `job_output` / `job_list` / `job_kill` | 不采纳 | 没有长任务后台作业的概念（ComfyUI 那边是轮询捕获，不是 job） |
| `todo_write` / `ask_user_question` | **部分采纳**（2026-09-20 起） | `todo_write` 仍然不做（没有多步任务面板）；`ask_user_question` **已采纳**（用户建议 ⑥）：见下面「提问与审批是两件事」一节 —— 它**不结束**当前回答，答完同一个 Run 接着做 |
| `web_search` / `web_fetch` | **采纳**（2026-09-20 起推翻"首期不采纳"） | 用户定的规则：**只用通用方案**——不碰各家原生内置搜索（OpenAI `web_search` / Anthropic `web_search_20250305` 那套：能力没有统一的查询接口、按模型逐个变，取舍记录见 §2.0.1），接 **Keenable**（给 AI Agent 用的独立搜索索引，**默认免密钥**，按来源约 1000 次/小时；可选 key 只用来提额）。原"不采纳"的理由是"需要外部搜索服务与密钥"——免密钥默认把它消掉了；"与本地创作工具定位不符"由用户拍板推翻：查提示词用法 / 模型文档 / ComfyUI 节点说明是真需求。出网客户端：`KeenableSearch.kt`（本项目**唯一**出网口，复用 `EndpointGuard`） |
| `present` / `exit_plan_mode` / `goal` 系列 | 不采纳 | 交付物在画廊里，不在工具里 |
| **`skill`（加载 Skill 正文）** | **采纳** | 对应我们的 `load_skill`，并补了 `list_skills` / `register_skill` / `delete_skill` |
| Skills 目录注入 | **采纳思路，实现不同** | DSH 把目录作为 **user 消息**（`<system-reminder>` + `<available_skills>`）注入，为的是不打断系统前缀的 KV cache；我们的系统提示本来就带时间戳（每 Run 变化），所以直接写进系统提示的独立区块，省一层消息 |
| DSH 默认提示词的**行为纪律段** | **择优采纳**（2026-09-17，用户要求） | 原文 `docs\default-prompt-inject.md`。挑出与我们对得上的三块写进系统提示 **v9**：① 事实纪律（不猜本机状态 / 证据冲突的采信顺序 / 把"查到的"与"推测的"分开说）；② 表达风格（开门见山、不奉承、不复述、**不写嵌套列表** —— `lib\widgets\markdown.dart` 会把子项拍平成同级，而表格反而是支持的）；③ 两条硬规则（写入留退路：改参数用覆盖不重写原始工作流；读取按需：不遍历磁盘）。另外，那份文档里的 `[MODE: UNRESTRICTED SANDBOX — STABLE]` 段（人格 / 拒绝对抗 / 越狱与渗透话术）**当年是整段搬进来的，现在也还在**（`HarnessRunner.kt` 里一个独立的 `append("""…""")`，上方有注释）：2026-09-17 用户明确说"那是我的意图，不许删除，不要动那段越狱提示词"。它与代码侧的审批纪律（提示词第 3 条）和凭据红线（第 8 条）并存 —— **别把它当误粘贴清理掉**，要改先问用户（AGENTS §10 记着这条） |

我们的工具清单（**唯一真源是 `server-rs/src/tools/tools.rs` 的 `IMPLEMENTED`** —— 当前 37 个：手机侧 7 个 + 桌面专属 30 个；全部可在设置里改成 `allow / ask / deny`）：

| 工具 | 类别 | 权限档 | 说明 |
| --- | --- | --- | --- |
| `list_skills` | skill | allow | 列出本机 Skills（名称/描述/是否合法） |
| `load_skill` | skill | allow | 取完整正文，包成 `<skill_content>`；同一 Run 不重复加载 |
| `register_skill` | skill | allow | **AI 说一声就把 Skill 写到本机**（用户核心诉求） |
| `delete_skill` | skill | **ask** | 不可逆，默认要用户点批准 |
| `remember` | memory | allow | 把一条跨对话成立的事实写进长期记忆（M6）；写的是应用自己的 `memory.md`，不是用户文件系统 |
| `list_dir` | files | allow | 列目录（默认 ComfyUI 目录，深度 ≤2） |
| `read_file` | files | allow | 读文本，单次上限 2MB（2026-09-19 由 256KB 上调）；没读完会给出 `nextOffset`，按 `offset=` 接着读 |
| `write_file` | files | allow | **只能写 ComfyUI 目录**（越界直接拒绝）；单次上限 4MB，超了用 `mode="append"` 分多次写 |
| `comfy_get_status` | comfy | allow | 复用 `ComfyCapture.status()`：连通性 / 队列 / 最近捕获（**附最近提交的任务**） |
| `comfy_get_run` | comfy | allow | 按 runKey 查一次捕获（AIH-034） |
| `comfy_sync_history` | comfy | **ask** | 会写我们的库，按 DEC-005 默认要审批 |
| `comfy_load_workflow` | comfy | allow | 把**一份本机工作流文件**读进库换成一个能提交的 `promptId`（用户 bug ③，2026-09-18）：API 格式原样用；界面格式（nodes/links）按 ComfyUI 的 `/object_info` 转成 API 节点图 —— 普通节点、**组节点（subgraph）**、Reroute / Set-Get、旁路都会等价展开或改写，控件值**首选带名字的 `widgets_values_named`**。**## 模拟用户行为 ##**（用户建议 ②，2026-09-21）：`emulateFrontend` **默认开着**，把纯前端节点照前端排队前的行为等价改写（广播填空 / 丢显示节点 / 补 SaveImage），改了什么都列在 `frontendEmulated` / `frontendBroadcast` / `frontendInjected` 里（见 §2.0.2）；说不清类型的仍然如实报 `UNSUPPORTED_NODES`，带 `tolerateUnsupported=true` 再来一次则**摘掉它们并给出缺口清单**（`openInputs` / `unresolvedInputs`），交给模型补线。只读用户机器上的文件，不改它 |
| `comfy_find_workflow` | comfy | allow | 找一个**能直接跑**的工作流：先搜库里**能直接跑**的工作流（标注 `runnable` = 有没有 API 节点图）；**库里没有时自动回头列本机 ComfyUI 已保存的工作流文件**（结果在 `localWorkflows` 里，用户 bug ⑤，2026-09-18） |
| `comfy_list_workflows` | comfy | allow | 列出**本机 ComfyUI 里已经保存的工作流文件**（`user\<用户>\workflows\*.json`，用户 bug ⑤，2026-09-18）：带路径 / 格式（ui/api）/ 是否已入库（`inLibrary` / `promptId`），可按文件名过滤（如 `krea2`）。**只读**，不改用户的文件。首次使用、或 `comfy_find_workflow` 什么都搜不到时先看它 —— 这些工作流**不需要在 ComfyUI 里跑过一次**，拿到 path 直接 `comfy_load_workflow` 就能提交 |
| `comfy_submit` | comfy | **ask** | **真的把工作流提交给 ComfyUI 跑**（用户建议 ①，2026-09-17）：支持 `节点id.输入名` 覆盖参数、`connections` 补连线（值写 `["<上游节点id>", 槽位]`），跑完直接入库；会消耗显卡时间，默认要审批 |
| `comfy_use_attachment` | comfy | **ask** | 把用户发来的**图片附件**投放进 ComfyUI 的 `input/` 目录并返回真实文件名（用户 bug，2026-09-17）：图生图 / 参考图的唯一正路，默认要审批（往用户机器上写文件） |
| `comfy_check_inputs` | comfy | allow | **体检一份工作流：哪些输入还等着外部给**（用户建议 ⑤，2026-09-20）：把 `comfy_get_run` 的 `raw` / `comfy_load_workflow` 的 `apiGraph` 传进来，逐条列出 `节点id.输入名` / 节点类 / 缺什么类型 / 现在是什么值 / 下一步该怎么办，分成 `image` / `video` / `connection` / `parameter` 四类。**只读**。用户现场是"用户没提供参考图，但工作流的节点用到参考图"——没有它，模型会带着缺口硬提交，ComfyUI 只回一句缺输入。判据刻意保守：只有"连线型空着 / 值缺失"才算缺口，已经填好值的不报；`Save*` / `Preview*` 这类输出节点不算缺口。本次带了图附件时，图片类位置进 `suppliedByAttachment`（投放就能解决），不混进"要用户补" |
| `comfy_fit_workflow` | comfy | allow | **把一份工作流要的模型与本机已有的对一遍**（用户建议 ①，2026-09-22）：输入 API 节点图，输出①图里点名的每个模型（`CheckpointLoaderSimple` / `LoraLoader` / `VAELoader` / `CLIPLoader` / ControlNet…，自定义加载器靠 `/object_info` 的枚举反推）、②本机最接近的等价物（**词根相似度** + 体积）、③要改哪几个 `overrides` 键。**只报不改**：`suggestedOverrides` 要真生效得由模型写进 `comfy_submit(overrides=…)`。三档结论：`exact`（同名，不用改）/ `substitute`（有更像的，**先问用户**——相似度高**不代表架构兼容**）/ `missing`（本机没有，**不许编文件名**，去 `civitai_search` 查 + `ask_user_question` 让用户选装 / 换工作流 / 放弃）。清单来源优先 `/object_info` 的枚举（"ComfyUI 能加载什么"的权威答案），拿不到才扫盘，`inventorySource` 会说明是哪一档。**只读**：不装模型、不改图、不写文件 |
| `web_search` | web | **ask** | 联网搜索（Keenable）：用自然语言描述"你想要的那一页"（语义索引，别堆关键词），返回**带编号与链接**的网页正文（`[n] 标题 (URL)` + 抽取好的正文），引用时给编号与 URL。**出厂免密钥**；查询词会离开本机，所以默认要批准；一次回复最多 8 次（独立预算 `maxWebQueriesPerRun`）。一条没搜到报 `NO_RESULTS` 并要求模型如实说明 |
| `web_fetch` | web | **ask** | 把一个**公开**网页取回来读全文（服务端去样板转 markdown，默认 20000 字符上限）。目标 URL 出网前先过 `EndpointGuard` 的 PUBLIC 判定：内网 / 本机 / 云元数据一律 `FETCH_TARGET_DENIED`。正文套**不可信数据**横幅后才给模型。**它不是模型库检索**：要模型 / LoRA 的字段用 `civitai_search`（见 §2.0.3） |
| `civitai_search` | web | **ask** | 搜 **Civitai 模型库**（用户建议 ④⑦，2026-09-20）：每条结果都给"**拿来做什么**"——类型 / 底模 / 简介 / 触发词 / 下载量 / 链接，并放进结构化 `meta.models`（界面与"登记进长期记忆"直接可用）。普通搜索走 `civitai.com`（免令牌）；**NSFW 必须显式 `allowNsfw=true`**，走 `civitai.red`：配了令牌（凭据名 `civitai`，或环境变量 `CIVITAI_API_KEY`）就带上登录态，**没配也照搜**，但结果里会写明"本次是匿名通道"（2026-09-20 线上核对后由用户决定放宽，见第 9 节第 11 条）。与 `web_search` / `web_fetch` **共用同一个联网额度** |
| `comfy_prune_workflow` | comfy | allow | **给一份工作流做减法 / 加法**（用户建议 ②，2026-09-20）：`removeFrom` 点名要摘的输入（`"89.image"`）或节点（`"89"`），后端做**确定性图运算**——摘掉目标、把指着它的线接回它自己的上游、把"从输出再也走不到又不是孤立节点"的那些级联收掉；`injections` 往一条线上插节点（典型是显存不足时的 `SageAttention`，接哪些输入由 `/object_info` 算）。返回**改好的完整图** + `removed` / `rewired` / `injected` / `unresolvedInputs` / `openInputs` / `runnable`。**只读**（不改用户的文件）；`runnable=false` 时工具会明确说"别硬提交" |
| `comfy_start` | comfy | **ask** | **把本机 ComfyUI 实例起起来**（2026-09-28，用户 bug「通过 CLI 启动 comfyui 总是失败」）：ComfyHub **自己起** —— 用工作区自己的 `.venv\Scripts\python.exe` 跑 `main.py`，带上 ComfyUI Desktop 的三个路径参数（共享模型根 / input / output，复用 `launch_comfyui` 那一份实现）、`PYTHONUTF8=1` / `PYTHONIOENCODING=utf-8`，日志落项目内 `.run\comfyui-<端口>.log`，**等 `/system_stats` 真的答话**才回报成功（进程刚退出 / 等超时分开如实报，带日志尾巴）。已经在跑时**不再起第二个**。起之前有一道**写权限闸门**：当前会话把进程关在项目里时（子进程继承这个限制，WMI / schtasks 都被系统挡掉）**当场拒绝**并让用户自己双击 ComfyUI Desktop —— 硬起只会得到一个卡死的实例。默认要审批（起一个占显存的长跑进程）。**不要再用 comfy-mcp 的 `launch_comfyui`**：comfy-cli 启动要写两个项目外的文件，受限会话里必然 `Errno 13` |
| `comfy_stop` | comfy | **ask** | **停掉 ComfyHub 自己起的那个实例**（2026-09-28）：只认 `.run\comfyui-<端口>.pid` 里那条记录，杀之前还核一眼那个 pid 现在是 python（pid 会被系统回收）——**用户自己用 ComfyUI Desktop 起的实例一个进程都不碰**（那种情况如实拒绝，让他自己在那个窗口里关）。杀完等端口真的不再答话才回报成功 |
| `ask_user_question` | ask | allow | **当场提问、不中断当前回答**（用户建议 ⑥，2026-09-20）：把问题（+ 2~5 个可点选项）递到界面，**挂在原地等**用户答，答完把答案当工具结果喂回模型，**同一个 Run 接着往下做**。提问期间**输入区被答题卡接管**，末尾固定补一条「自己输入答案」（用户 bug 2026-09-21）。等不到（超时 / 用户跳过 / Run 结束）时如实返回"用户还没有回答"，模型据此基于已有信息继续。同一句话问两遍会被 `QUESTION_ALREADY_ASKED` 挡掉。既不改盘也不出网，所以默认 allow |

### 2.0.2 提问与审批是两件事（2026-09-20，用户建议 ⑥）

用户的原话是「让 AI 像 deepseek harness 一样支持提问功能，**回答结束时不中断当前对话**」。
所以 `ask_user_question` 与"等用户点批准"（`ToolAccess.ASK`）**必须分开**：

| | 审批（`ToolApprovalGate`） | 提问（`AskGate` / `AskRegistry`） |
| --- | --- | --- |
| 用户没反应时 | 当作**拒绝**，工具不执行 | 当作**没回答**，如实告诉模型"用户还没有回答" |
| 结果 | 一次允许 / 不允许 | **一段文字**，模型拿着它接着做 |
| Run | 停在这里等 | **不终止**；答完继续同一个 Run |
| 界面 | 工具卡上的「批准 / 拒绝」 | 一张**提问卡**（选项按钮 + 「跳过」）；提问期间**接管聊天下部输入区**（选项 + 自己输入 + 跳过 / 停止） |

**提问时的两块 UI 分工**（用户 bug 2026-09-21："在调用 `ask_user_question` 时，将原来聊天详情
下部的模块替换（输入框等），具体可以参考 deepseek harness 的做法"）：

| | 气泡里的提问卡 | 被接管的输入区（`_AskComposer`） |
| --- | --- | --- |
| 是什么 | **记录**：问过什么、答了什么，跟着这条回复走 | **作答**：现在就要做的决定 |
| 有什么 | 问题 + 选项行（点一下即答）+ 固定末行「自己输入答案」 | 问题 + 选项行 + 最后一行**自己输入答案**（输入框）+ 「跳过这个问题」/「停止生成」 |
| 为什么不放输入框 | 一屏两个输入框，用户不知道该往哪儿写 —— 打字的地方只有底下一个 | 答完 / 跳过后 `store.pendingQuestion` 变 null，普通输入框自动回来 |

- 选项的**最后一行固定**是"自己输入答案"（用户原话："最末尾选项固定让用户决定（输入你的答案）"）：
  序号接着模型的选项往下数，点它就把焦点交给底部那个唯一的答案框。
- 所以模型**不用再留"其他"这种选项**了：系统提示与工具描述都写明了界面会固定补一条。
- 后端最多收 5 个选项（`take(5)`），多出来的丢掉；`Enter` 提交、`Shift+Enter` 换行。
- 回归用例 `test/ai_ask_question_test.dart` 4 例（接管 / 点选项 / 自己输入 / 跳过）。

实现要点（改这块之前先读 `AskGate.kt` 的注释）：

- **回答先落"一次性邮箱"再唤醒等待方**：界面把回答 POST 回来的那一刻，模型那边的协程
  可能还没走到 `await`（事件发出 → 用户秒点）。所以 `AskGate` 有一条 `answers` 表，
  `resolve` 与 `await` 都先看它 —— 否则用户的话会被静默丢掉，而工具侧一直等到超时。
- **Run 收尾必须放行**：`HarnessRunner` 的 `finally` 调 `AskRegistry.discardRun(runId)`，
  不然协程要等 30 分钟超时才回收，界面还挂着一张"等你回答"的卡片。
- **提问卡要能重开**：`GET /api/ai/runs/{id}/question` 回"这个 Run 还在等什么"，
  界面重开 / 断线重连时把卡片补回来（SSE 里那条 `user.question` 可能是在界面没挂着时发的）。
- **答了什么要落库**：工具结果的 `asked` 字段进 `parts.payload.question`，
  历史里的提问卡靠它还原（问了什么、选项是什么、用户答了哪个）。
- 出厂 `allow`：它不改盘、不出网，只是把一句话递到用户面前。
- 提示词第 9.1 条（`NsfwPrompt`）写着纪律：**能自己查的不要问**、一次只问一个决定、
  同一个问题不要问第二遍、没等到回答时**不许假装他答过了**。

### 2.0.2 「## 模拟用户行为 ##」——把纯前端节点也跑起来（2026-09-21，用户建议 ②）

用户的原话：「加入 ##模拟用户行为## 模式，面对一些**仅面向前端的工作流节点**，
让 AI 有能力在前端读取、编辑、在 comfy 前端提交」。
范围与用户当面确认过：**走"后端 + ComfyUI HTTP API"这条线，不做浏览器自动化**。

问题在哪：ComfyUI 的官方前端在 `graphToPrompt()` 之后、`POST /prompt` 之前，会调用各扩展
注册的 JS 钩子改写节点图。那份 JS 在**浏览器**里，服务端永远拿不到 —— 所以
`Anything Everywhere` 这类"只用前端 JS 实现"的节点过去只能如实报 `UNSUPPORTED_NODES`。

「模拟用户行为」= **把前端排队前会做的那几件事，在后端等价地做掉**
（Kotlin `FrontendNodes.kt`；Rust 侧是 `server-rs/src/workflow/workflow_frontend.rs`，
**P4 第 68 步搬完** —— 之前 Rust 只如实报 `unsupportedNodes`，判据/表复用
`workflow_audit.rs` 的 `is_broadcast` / `is_display`，没有第二份表。判据一律保守、做了什么都如实回报）：

| 前端节点 | 前端自己干了什么 | 这里怎么等价改写 |
| --- | --- | --- |
| `Anything Everywhere` 一族（含 `?` / `!` / `Prompts` / `Seed` / 数字后缀变体） | 把上游那一路**广播**到全图所有同类型、且没接线的输入上 | 按它 `properties` 里声明的 `Input Type` / `Input Name` 过滤**填空**，并逐条报告灌了哪几条线 |
| `Preview*` / `ShowText` / `Display Any`… | 只是"看一眼"的显示节点 | 丢掉（官方前端也不把它们写进提示词） |
| （没有任何输出节点） | 前端会**自动挂一个 `SaveImage`** | 补一个接在真正产出 `IMAGE` 的节点上，`filename_prefix = ComfyHub` |

几条纪律（改这块之前先读 `FrontendNodes.kt` 的文件头注释）：

- **开关**：`comfy_load_workflow.emulateFrontend`，**默认开着**；
  `false` = 一个字都不改、只如实报缺口的保守档（用户说"别动我的工作流"时用它）。
- **改动必须可见**：`frontendEmulated` / `frontendBroadcast` / `frontendInjected` 逐条列出，
  外加一句 `frontendHint`。系统提示第 15 条（`v22`）要求模型把这些改动**转述给用户**——
  默默替用户改图是不允许的。
- **说不清就不猜**：广播节点没声明类型、或者全图没有对得上的空输入，照旧落进
  `unsupportedNodes`（出口还是老那套：`tolerateUnsupported=true` 摘掉 + `connections=` 补线，
  或者让用户在 ComfyUI 里「导出（API）」）。**已经接好的线一个字都不动**。
- **不做**：浏览器自动化（不驱动 ComfyUI 网页界面）；rgthree 那类"只有界面逻辑"的特殊节点
  （说不清语义就不猜）。这两件是刻意的边界，不是没做完。
- 判据：`FrontendNodesTest` 5 例 + `WorkflowConvertTest` 原有那批
  （**不开开关时行为与过去逐字一致**那一条就是防回归的锚）；
  Rust 侧对应 `workflow_frontend.rs` 的单测 + `workflow_convert.rs` 里那两条
  "`emulateFrontend=true` vs `false` 逐条比差异"的用例（P4 第 68 步）。

### 2.0.3 摘节点 / 拼节点（2026-09-20，用户建议 ②）

用户的原话：「让 AI 能够基于基本工作流，**摘除无关节点或者拼装必要节点**」，并补了两个现场：
"我没给参考图，但工作流的节点用到参考图 → 要能识别出骨干、把无关内容临时摘掉"、
"显存不足但是硬要跑 → 可能要考虑向工作流加入 Sage Attention"。

`comfy_prune_workflow` 做的是**确定性图运算**，不是让模型重写 JSON：

- **摘除**：点名 `节点id.输入名` 会摘掉那个节点、清空它的连线输入，
  并把指着它的线**接回它自己的上游**（`KSampler.positive` 原本接 ControlNet，
  摘掉之后接回 `CLIPTextEncode`）；消费者的节点**保留**。
  另外级联收掉"**从输出反向再也走不到、又不是孤立节点**"的那些
  （`LoadImage` / `ControlNetApply` / `ControlNetLoader` 整条参考图支路）。
  两条闸门：**输出节点永远不摘**；**没有连线输入的孤立节点不碰**
  （那是用户还没接完的东西，不是"无关分支" —— 第一版拿"删完之后输出还能不能走到"当判据，
  把没接线的 `EmptyLatentImage` 一起删了）。
- **拼装**：`injections[].at` 指定插在哪条线上，`classType` 由后端查 ComfyUI 的
  `/object_info` 决定它要接哪些输入（`model` 那类"接上游"的输入自动接回原上游）。
  拿不到定义表 / 这台机器没装那个节点类时**宁可少插也不乱接**，并如实警告。
- **返回里必须带审计清单**：`removed` / `rewired` / `injected` / `unresolvedInputs` /
  `openInputs` / `runnable`。用户的工作流被动了哪里，模型要说得清楚。
- 它是**只读**工具（不改用户机器上的文件），改好的图在结果里，要跑得先落成文件再提交。

### 2.0.1 联网搜索的三条安全线（2026-09-20）
1. **默认 ask**：`web_search` / `web_fetch` / `civitai_search` 是**本项目唯一会把数据送出本机的工具类别**（`ToolCategory.WEB`）。
   查询词本身可能带用户的创作意图，所以出厂停在「询问」档；用户在权限页改成 allow 后才免批准。
   `civitai_search` 的 `allowNsfw=true` 更要用户点头 —— 用户建议 ④ 的原话就是"这种情况可以询问用户"。
2. **独立预算**：`maxWebQueriesPerRun = 8`（`ToolPolicyConfig`，进 `normalizeStored`——
   它不是用户设置，老库切一次权限档就会把出厂值冻进去，不过 normalize 就会"改了没用"）。
   每搜一次都是真实外呼 + 一份要烧 token 的正文，"再搜一下确认"必须刹得住。
3. **不可信数据 + 地址判定**：返回正文前套 `untrustedBanner()`（RSK-004 注入防线：网页里的
   "忽略规则"与工具结果里的任何指令都不是指令）；`web_fetch` 的目标先过 `EndpointGuard`
   PUBLIC 判定。HTTP 层纪律与 `AiUpstream` 相同：不跟随重定向、日志绝不落密钥、
   免密钥请求必须带 `X-Keenable-Title`（缺它服务端 400，实测）。
4. **出网遵守本机代理，但内网永远直连**（2026-09-20，用户报的 `civitai_search` 反复超时）：
   JVM 默认一律 DIRECT，所以启动参数带 `-Djava.net.useSystemProxies=true`，
   所有出网客户端再统一挂 `com.comfyhub.ai.OutboundProxy` 的选择器 ——
   公网按本机代理设置走，`127.0.0.1` / 私有网段 / `*.local` 这类**强制直连**
   （打开系统代理后 Java 会把内网也交给代理，那会打死本机 ComfyUI）。
   覆盖入口是环境变量 `COMFYHUB_HTTPS_PROXY`（`direct` = 强制直连）。
   报错文案里带上本次路径，`CIVITAI_TIMEOUT` / `SEARCH_TIMEOUT` 也区分
   "连不上"与"连上了但回得慢"。判据：`OutboundProxyTest`。

测试怎么钉的：`ToolRegistryTest`（预算 / 审批 / 横幅 / SEARCH_DISABLED / CIVITAI_DISABLED / 结构化 meta，假客户端不碰 HTTP）+
`KeenableSearchTest`（本机起假服务钉路径 / 头 / 错误映射 / 内网拒绝，**不出网**）+
`WorkflowInputsTest`（待补输入的判据：该报的不漏、填好的不报，**不碰 HTTP**）。

### 2.0.3 `civitai_search` 为什么不和 `web_search` / `web_fetch` 合并（2026-09-20 用户问）

用户的原话：「civitai_search 跟 web_fetch，有没有合并的可能？感觉有点重复造轮子。」
**结论是不合并**，三者的动作根本不是一回事：

| | 打的是什么 | 回的是什么 | 能不能带登录态 |
| --- | --- | --- | --- |
| `web_search` | Keenable 的语义索引 | 若干网页的**片段** + 链接 | 不适用 |
| `web_fetch` | 某个 URL 的**那一页** | 去样板后的 markdown 正文 | 不能（不接受自定义头） |
| `civitai_search` | Civitai 的**模型检索接口** | 结构化字段：类型 / 底模 / 触发词 / 下载量 / 是否 NSFW + 链接 | 能（`Authorization: Bearer`） |

真要"合并"，模型就只能拿 `web_fetch` 去拼 `civitai.com/api/v1/models?query=…`：
拿到的是转成 markdown 的 JSON（字段全糊在一起）、带不上令牌、
还得自己猜 `types` 的大小写与 `limit` 上限（上游对这些是 400 报错的）。
所以保留成两个工具，**把边界写进三处**：工具描述（`web_fetch` / `civitai_search` /
`web_search` 各自点名该走哪条路）、系统提示第 17 条、以及本表。
判据：`ToolRegistryTest` 的「web_fetch 与 civitai_search 的分工写在工具描述里」
（顺带钉住"工具描述被系统提示截断到 300 字时，数据边界那句不能被挤掉"）。

> 曾考虑过的替代方案 open-webSearch（Node MCP 爬虫聚合）记录在案：中文引擎是真优势，
> 但它需要一个常驻 Node 进程（实施方案 §5.2 明确排除 sidecar）且爬虫会随引擎改版静默坏掉。
> 将来若要中文引擎，正确姿势是**用户自跑 `open-websearch serve`，我们加"自建搜索网关"设置**打它的
> `POST /search`（纯 HTTP，不带 Node，与 ComfyUI 接入同款模式）。

### 2.1.1 图生图为什么要单独一个工具（2026-09-17）

用户报的原话：「工作流的 LoadImage（节点 89）读的是它自己被捕获时绑定的那张 jpg，
我只能改文本，改不了这个文件名」。根因是 **LoadImage 的 `image` 输入只能是 ComfyUI
`input/` 目录里真实存在的文件名**，而那个名字是工作流被捕获时留下的，跟我们库里的附件毫无关系。

所以正路只有一条，写进了系统提示 v7 与内置 skill `img2img-reference`：

1. `comfy_use_attachment(attachmentId)` → 拿到投放后的 `filename`；
2. `comfy_find_workflow(includeGraph=true)` → 找到 `LoadImage` 节点的 id 与输入名；
3. `comfy_submit(overrides={"<节点id>.image": "<filename>"})`。

几条纪律：

- **文件名由后端算**：`<原文件名>-<附件 id 前 8 位>.<扩展名>`。同一个附件重复投放得到同一个名字
  （重跑工作流不会指向一个已经消失的文件），不同附件即使原文件名相同也不会互相覆盖。
- **只投图片**：非图片附件直接报 `NOT_AN_IMAGE`；找不到 ComfyUI 报 `COMFY_NOT_FOUND`，
  **不猜路径、不在随机位置建目录**。
- **附件 id 要交给模型**：用户轮正文末尾会附「这条消息里的图片附件：id=…」。
  **没随本次请求内联的图也算** —— 文件在库里，纯文本模型照样能拿它当参考图（但不能假装看过图内容）。
- 提示词里明确禁止"改不了文件名所以做不了"这种半途而废的答复。

### 2.1.2 "库里没有这条工作流"为什么不等于"用不了"（用户 bug ⑤，2026-09-18）

现场是一条真实对话：用户说"基于 `krea2SFWNSFWUncensoredImageTo_v10` 工作流生成"，
AI 回"**库里搜不到**这条工作流，要么把 `.json` 的完整路径发我，要么你在 ComfyUI 里点一次 Queue
让它被自动捕获"。而那份文件**一直躺在磁盘上**：

```
D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI\user\default\workflows\krea2SFWNSFWUncensoredImageTo_v10.json
```

根因不在模型，在库的来源：`prompts` 表过去只有两个入口 —— **自动捕获**（轮询 ComfyUI `/history`，
只收"本进程活着的时候跑过的"）与 **`comfy_load_workflow`**（要有人先给出路径）。
于是用户机器上早就存着的工作流，在 AI 眼里等于不存在。用户的原话：
**"我需要在本项目进程存活的时候，产生新工作流运行的记录才会入库，这不对吧"** —— 对。

三处一起改（`ComfyWorkflowFiles`）：

1. **看得见**：`comfy_list_workflows`（只读）列出本机 ComfyUI 已保存的工作流文件；
   `comfy_find_workflow` 在库里搜不到时**自动带上**它们（`localWorkflows`）。
2. **说得对**：`NOT_FOUND` 兜底文案改成"…本机 workflows 目录里也没有文件名匹配它的"，
   并指向 `comfy_list_workflows`；系统提示 **v13 第 16 条**给出正确顺序
   （`comfy_find_workflow` → `comfy_list_workflows` → `comfy_load_workflow(path=…)` → `comfy_submit`），
   并**明确禁止**再说"得先在 ComfyUI 里跑一次才会被捕获"。
3. **进得去库**：设置页 →「ComfyUI 自动捕获」多一颗**「导入本机工作流」**按钮
   （`POST /api/capture/import-workflows`），把目录里的工作流批量读进 `prompts`
   （`source=ComfyUI-File`，与 `comfy_load_workflow` 同一把钥匙 `run_key = file:<sha256>`，所以**幂等**）；
   转换不了的那几份也照样入库、如实标 `runnable=false`（缺口照旧交给 AI 按清单补线）。
   另有只读的 `GET /api/capture/workflows?q=`（界面 / 脚本 / 排错都用它）。

目录怎么找（与读白名单同一套判据，绝不"看到个叫 workflows 的目录就认"）：
配置里已生效的输出目录的**父目录**（ComfyUI 的真实形状就是 `output` 与 `user` 并列）→
`ComfyLocator` 探测到的 home → `ComfyRoots.autoReadRoots`（Desktop 把程序与共享数据分家时，
工作流在**安装目录**下，只按输出目录推是推不到的）。扫描**有界**：只看
`user\<用户>\workflows` 一层，非 `.json` / 空文件 / 坏 JSON / 既不是界面格式也不是 API 格式的
一律不列（列出来只会让 AI 白跑一趟 `comfy_load_workflow`）。

防线：`ComfyWorkflowFilesTest` 7 例（懒加载发现 / 多用户目录 / 坏文件过滤 / query / 入库标记 /
空目录如实说）+ `ToolRegistryTest` 3 例（回落、`NOT_FOUND` 指路、只读 allow）+
`e2e-submit-test.ps1` 第 6 幕 6 项断言（只读列出 → AI 拿到路径 → 一键导入 → 幂等 → 状态回填）。

### 2.1 提交任务（`comfy_submit`）的几条纪律（2026-09-17）

- **只认 API 格式节点图**：`prompts.workflow_json` 是界面格式（`widgets_values` 只有位置、没有参数名），
  提交要用的是 `capture_runs.raw` 里那份"当时真正跑的东西"（`ComfyCapture.apiGraphOf`）。
  老数据没有它就报 `NO_API_GRAPH` 并说明怎么办 —— **不做"看起来差不多"的转换**。
- **参数覆盖按原类型转换**：原来存整数就不能塞字符串；字段不存在、或值是连线数组（`[节点, 序号]`）
  一律报错（`WorkflowEditTest` 13 例）。静默忽略的后果是模型以为改了、用户以为改了，实际没改。
- **补连线 (`connections`) 与覆盖参数分开**（用户建议 ②，2026-09-18）：前面那套是"改值"，
  这套是"接线"。键同样是 `节点id.输入名`，值必须是 `["<上游节点id>", 槽位]`，而且
  **上游节点必须真的在这张图里**（写错一个节点号，ComfyUI 只会回一句"节点不存在"，最难查）。
  与覆盖参数不同的是：它**允许写一个当前不存在的输入名** —— 界面格式里那些被摘掉的前端节点
  留下的空位本来就不在 `inputs` 里。
- **"转换不了"不再是死路**（用户建议 ②，2026-09-18）：`comfy_load_workflow` 带
  `tolerateUnsupported=true` 时会把纯前端节点摘掉并给出缺口清单（`runnable=false` +
  `openInputs` + `unresolvedInputs`），模型照着清单补线再提交。**猜的那部分没有变多** ——
  线接错了 ComfyUI 自己的校验会当场拒绝（`node_errors`，不入队），比服务端瞎猜一个语义安全得多。
- **等待有上限**：默认 240 秒、最多 900 秒；超时报 `timeout` 并提示"跑完会自动入库"，
  **不假装完成也不假装失败**。
- **产物入库与手动出图完全同路**：`capture.captureRun()`（幂等靠 `prompt_id` + 文件 SHA-256），
  所以"AI 提交的产出"和"用户自己点的产出"在库里是同一种东西，画廊里都能看到；
  产物 id 会随工具结果回到界面，在回复末尾贴成「画廊入口卡」（用户建议 ⑤）。

## 2. 权限模型：为什么不是照搬 DSH 的 preset

DSH 用 `read-only / workspace-write / danger-full-access` 三档 preset + `workspaceRoot` 表达权限
（`dsh-tool-layer-report.md` §4：**没有** `autoApprove` / `allowedDirectories` 这类白名单）。
那是给"AI 帮我在这个仓库里干活"设计的：整个工作区都该可写。

我们面对的是**内容创作者**，不是开发者；他们机器上这个目录里有 `.mysql`（数据库）、`.run`（进程状态）、
`storage`（作品）、`server`（后端）。写坏一个的代价是不对称的：AI 多写一个文件没什么价值，
但把数据库目录或版本库弄坏，用户就得重装。所以我们的默认是：

```
写：只允许 <项目根>\comfyui            ← 用户要求的"默认不能修改 comfy 目录以外的内容"
读：<项目根>\comfyui、<项目根>\storage  ← 产物目录只读
永远不可写：.git / .mysql / .run / node_modules（即使用户把白名单放宽到项目根，也仍然拒绝）
```

三条补充规则：

1. **路径判定走真实路径**：`..`、绝对路径、符号链接、短路径名都不能绕过（`ToolPolicy.realPath()`：
   存在就用 `toRealPath()`，不存在就取最近的存在祖先再拼回剩余部分）。
2. **逐工具三态**：`allow` 直接执行、`ask` 弹工具卡等用户点批准、`deny` **根本不下发给模型**
   （与其让模型看见再被骗着调用，不如不让它知道）。用户覆盖记在 `app_settings` 的 `ai.tools.policy`。
3. **审批超时 = 拒绝**：默认 5 分钟没人点、或 Run 被取消，都按拒绝处理 —— 绝不允许"没人管就默认执行"。
4. **权限两档（用户建议 ⑤，2026-09-17）**：`ai.tools.policy.permissionMode` = `ask`（默认，界面叫
   「询问」）/ `full`（界面叫「自动允许（无需批准）」），在 AI 工作台输入区底部、附件按钮与模型选择之间切换。
   `full` 只把 `ask` 放宽成 `allow`：**`deny` 不放宽、路径白名单不放宽**（越界写照样被拒，
   回归用例 `ToolPolicyTest` 盯着这两条）。后端每次 Run 现读策略，所以切完档**下一次回复立刻生效**
   ——系统提示里会即时写明"本次是自动允许（无需批准）档"（`SystemPrompt.VERSION` 由 v5 提到 v11：
   v6 = 权限档，v7 = 图生图那三步纪律，v8 = 档位改名，v9 = DSH 行为纪律段，v10 = 查询预算 9 次）。
   > 名字的由来（用户建议）：「完全权限」听着像"什么都能干"，其实它只免掉"问一下"，文件夹白名单
   > 一点都没放宽 —— 所以改叫「自动允许（无需批准）」。界面上那两个名字是
   > `AiToolPolicy.modeAskLabel` / `modeFullLabel`（Dart 侧真源），系统提示里与它们一字不差。
   > **踩过的坑**：`/api/ai/tools/policy` 的响应里如果没有 `permissionMode` 字段（后端是旧构建），
   > 前端会按最保守的「询问」解析 —— 表现就是"点了自动允许，界面又跳回询问"（用户 bug ①）。
   > `AiWorkspaceStore.setPermissionMode` 现在会比对后端**回显**的档位，不一致就明说"后端还是旧构建"。

> 移植自 DSH 的两个细节：工具的 JSON Schema 里 `additionalProperties: false`（模型乱加参数会当场报错，
> 而不是悄悄忽略）；工具结果**视为不可信数据**，写进系统提示第 4 条（防提示注入，对应 RSK-004）。

## 3. 工具循环（HarnessRunner）

```
系统提示 + 历史（含历史里的 tool_call/tool_result）
  └─ while (轮数 < maxToolSteps=8)
       ├─ 请求上游（带 tools 定义）
       ├─ 收流：text / reasoning / tool_call 分片 → ToolCallAccumulator 拼完整
       ├─ 没有工具调用 → 结束（这就是最终回复）
       └─ 有工具调用 → 逐个：策略 →（必要时审批）→ 执行 → 落 ai_tool_calls
                      → 事件 tool.requested/started/completed/failed
                      → 结果作为 role=tool 轮喂回去
  └─ 轮数用尽（step == maxSteps）那一轮**照样带 tools，但不再执行**：
     不给清单的话，模型（DeepSeek 系）会把工具调用语法当正文吐出来，答案尾巴上就是一段
     原始标记（用户 2026-09-19 报的"回答像被截断"）；给清单只是不执行，还要调就如实说明
```

要点：

- **一条助手消息装下整轮**：正文 / 思考 / 工具调用 / 工具结果都按 `ordinal` 写进
  `ai_message_parts`，所以重开 App 能无损恢复，界面能按顺序渲染工具卡（AIH-019/049）。
- **token 累加**：多轮循环把每轮 usage 累加成一个 OpenAI 形状的对象再交给 `TokenUsage.from()`
  （只算最后一轮会严重少报，AIH-057）。
- **取消**：走协程取消 → `runInterruptible` 打断阻塞读 → Run 记 `cancelled`，
  并且**不再继续工具循环**（AIH-022）。
- **工具失败不打断 Run**：任何异常都变成本次调用的失败结果（`tool.failed` + 稳定 code），
  模型可以自己纠正，或者如实告诉用户（AIH-036：工具失败要如实报告，不许伪造进度）。
- **三层预算**（防"AI 拿 ComfyUI 当轮询器"）：
  ① 一次回复最多 `maxToolSteps = 8` 轮（到顶那轮不带 `tools`，逼它收尾）；
  ② 单 Run 工具调用总数 `maxCallsPerRun = 16`；
  ③ **查 ComfyUI 的次数 `maxComfyQueriesPerRun = 9`**（AIH-036 原话是 3 次；2026-09-17 用户实测
  "3 次太少"——投递附件 + 查节点 + 查工作流很容易就撞上限——要求放宽到 9）。
  这个数字的**唯一真源**是 `ToolPolicyConfig.DEFAULT_MAX_COMFY_QUERIES_PER_RUN`：系统提示词里插值它，
  读库时也用 `ToolPolicyConfig.normalizeStored` 覆盖掉库里冻着的历史值（它是内置预算，不是用户设置，
  但 `ToolPolicy.save` 会把整份配置写进 `app_settings`，只改默认值老库不生效）。
  超预算的工具调用会以 `TOOL_BUDGET_EXCEEDED` / `QUERY_BUDGET_EXCEEDED` 失败返回，模型能看懂并改用已有信息。
- **提示词版本**：v1 里那句「当前版本尚未注册任何工具」删掉了，v2 改成工具清单 + 权限边界 +
  Skills 使用纪律 + 防注入规则（AIH-046）；v3 长期记忆、v4 附件诚实、v5 会话标题、
  v6/v8 权限档与档位改名、v7 图生图三步；**v11 = 工作流文件可以直接提交**（`comfy_load_workflow`，用户 bug ③）；
  **v12 = 长期记忆的条数闸门 + 工作流转换不了时按缺口清单补线**（`tolerateUnsupported` / `connections`，用户建议 ②）；
  **v13 = "库里搜不到工作流"不等于"这条工作流用不了"**（用户 bug ⑤，2026-09-18）：先
  `comfy_find_workflow`（库里没有会自动带上本机文件 `localWorkflows`）→ `comfy_list_workflows` →
  `comfy_load_workflow(path=…)` → `comfy_submit`，并**明确禁止**再说"得先在 ComfyUI 里跑一次才会被捕获"；
  **v9 = 移植 DSH 默认提示词的行为纪律段**
  （事实纪律 / 表达风格 / 写入留退路 + 读取按需，见 §1 表格最后一行）；**v10 = 查询预算 3 → 9**
  （提示词里那个数字改成从常量插值）。改动只影响新 Run。
  **v34 → v35（2026-09-28）= 第 21 条「环节标记」**（用户建议：关键步骤之间 AI 的操作再折叠）：
  模型在**每个环节开始时**写一行 `[步骤]环节名[/步骤]`，名字**只许**从那六个里挑
  （正在分析请求 / 正在查资料 / 正在读文件 / 正在生成图片 / 正在写文件 / 正在收尾）；
  后端 `server-rs/src/ai/step_mark.rs` 流式摘掉它（用户看不到标记），落成 `step` 有序块 +
  发 `step.started` 事件，界面画成**默认折叠的环节组**（`lib/pages/ai_home_page.dart` 的
  `_StepGroup` / `_StepCollector`；README 功能表里那条「关键步骤（环节）折叠」有用户可见的三条纪律）。
  ⚠ 正文真源是 `server-rs/src/ai/nsfw_prompt.rs`（`gen_prompts.py` 已停用）：改提示词**必须** bump
  `PROMPT_VERSION`，而 `run_kit.rs` 的签名用例 + `api_test.rs` 的版本断言会一起红 —— 那是**故意的**。
- **网关不认 `tools` 时的兜底**：很多网关收到 `tools` 直接 400。这种情况本次 Run 会自动
  **退回纯文本模式重试一次**，并在回复开头如实写明"上游网关不接受工具参数"——
  不能让用户在"模型到底行不行"上猜（也绝不假装是模型自己不想用工具）。

## 4. Skills（M5）

### 4.1 磁盘布局与来源

```
<根>\skills\builtin\<name>\SKILL.md        内置，只读（优先级 100）
<storage>\ai\skills\<name>\SKILL.md        用户 / AI 注册，可改可删（优先级 200）
<storage>\ai\skills\<name>.md              平铺写法也认
```

同名冲突**不静默覆盖**：用户版本胜出并保持可用，胜出者带 `conflict` 提示，界面显示冲突说明（AIH-037）。

**装 skill 的唯一方式是投放口**（用户建议第 6 条，取代了原来的「从 DSH 导入」按钮）：
把 skill 文件夹（或一个 `.md`）拷进 `<storage>\ai\skills`，然后

- **后端启动时自动登记**：没有 frontmatter 的文件会被补上 `--- name / description ---`
  （`name` 取文件名 kebab-case，`description` 取正文第一行），**正文一字不改**；
  已经有 frontmatter 的文件一律不碰（哪怕它不合法，仍然按 §4.2 的规矩"列出来 + 带诊断"）；
  中文文件名转不出 kebab-case 时会在 `errors` 里如实说明，不静默忽略。
- 应用开着的时候点右侧栏的刷新（`POST /api/ai/skills/rescan`）即可，不用重启。
- 界面显示**后端算好的绝对路径**（源码树 `<项目根>\storage\ai\skills`，
  发布包 `<根>\storage\ai\skills`，便携式）并提供「打开文件夹 / 复制路径」。

`%USERPROFILE%\.dsh` 现在**任何代码都不读**（连导入按钮也删了）：没装 DSH 的机器拿到的是同样的功能。

### 4.1.1 frontmatter 里的块标量

真实 SKILL.md 里 `description` 十有八九写成 YAML 块标量：

```markdown
---
name: 3d-animation-short-generator
description: |
  Create complete stylized 3D animated shorts from a story idea…
compatibility: …
---
```

解析器支持 `|` / `>` / `|-` / `>-`（含缩进块），多行描述进系统提示前会**压成一行**
（`SkillDto.oneLineForPrompt`）。这不是锦上添花：不支持的话读出来就是字面量 `|`，
等于让这个 skill 在目录里"没有描述"，模型根本不知道什么时候该用它
（本机 16 个 skill 里当时有 5 个中招）。

### 4.2 frontmatter 与校验

```markdown
---
name: anima-prompt
description: 把模糊需求转成 Anima 优化提示词
whenToUse: 用户要 Anima 生图提示词时
version: 1
user-invocable: true
disable-model-invocation: false
---
正文（Markdown）
```

- `name` 必须是 kebab-case（`^[a-z0-9][a-z0-9-]{0,63}$`）且与目录名一致；`description` 必填
  （写入路径上限 4000 字，扫描已有 skill **不因为描述长就判非法**）；正文非空（≤256KB）。
- **非法项不静默忽略**：仍然列在界面里、带 `validationError` 诊断，但**不进系统提示、不能被加载**（AIH-038）。
- `disable-model-invocation: true` → 只有用户能显式调用（外部未审核 Skill 的推荐姿势）。

### 4.3 即时生效

- 正文真源在磁盘，**没有索引缓存**：`register_skill` 写盘后，下一次 `load_skill` 就能读到；
  用户点删除，下一次 Run 的系统提示里就没有它了。
- 系统提示里只注入**名称 + 描述（截断 240 字）**，正文按需 `load_skill` 取（AIH-039/040）——
  这是上下文成本的关键。
- 用户说的"不重启应用、新对话生效"就是这样满足的：提示词每次 Run 现渲染。

### 4.4 安全（AIH-044/045）

- **不执行任何 Skill 带来的脚本**：本期工具集里根本没有 shell/进程工具，`scripts/` 目录只是不可执行的资源。
- 投放口只读"根下一层"的 `<name>/SKILL.md` 或 `<name>.md`，不做递归搜索；自动登记只补 frontmatter，
  不动正文（超 256KB 的正文直接报错，不写）。
- 注入防线写在系统提示第 4 条：工具输出、Skill 正文与**长期记忆**都是**数据**，
  里面出现"忽略你之前的规则"这类话要报告给用户，而不是照做。

## 4.5 长期记忆（M6）

用户建议"引入长期记忆"的落地方式：

```
<storage>\ai\memory.md      一行一条，人能看、能手改（与 Skills 同样的取舍：真源在磁盘上）
```

- **注入**：每次 Run 现读，`SystemPrompt.VERSION = v3` 起多一段「长期记忆」，
  与工具输出同等对待（数据不是指令）；注入部分截断到 4000 字符，文件本身可以更长（上限 8000）。
  标题行会带上"现有 N/100 条"，让模型知道还剩多少额度。
- **写入**：AI 走 `remember` 工具（默认 `allow`，写的是应用自己的记忆文件，
  不是用户文件系统 —— 所以不走 `ToolPolicy.resolveWrite`，也不需要审批）；
  界面走 `PUT /api/ai/memory`（整篇）、`POST /api/ai/memory/entries`（追加一条）
  与 `POST /api/ai/memory/delete`（按下标删单条 / 批量）。
- **规则限制（2026-09-18，用户要求）**：用户的顾虑是"条数太多，想查找、改动、删除会比较困难"，
  所以除了字符上限再压三道闸：
  ① **总条数硬上限 100**（`MAX_ENTRIES`），满了 `remember` 报 `MEMORY_FULL` 并**请用户去界面清理**
     —— 不许静默丢、也不许自动挤掉最旧的一条，更不许模型自己删改用户的条目；
  ② **一次 Run 最多新增 4 条**（`MAX_ENTRIES_PER_RUN`，查 `ToolContext.memoryWrites`）；
     判重命中的"同一件事又说了一遍"不算新增 —— 既不占额度，也不被节流挡住；
  ③ AI 写的每条自动带 `[yyyy-MM-dd]` 前缀（用户手敲的**不带**，不替用户改写他自己的话）；
     判重忽略这个前缀，所以"隔天又说一遍"不会堆成两行。
  界面上：面板显示 `N / 100 条`，编辑弹窗是**列表视图**（搜索框 + 每行一个删除 + 勾选后批量删除，
  删除前确认），「整篇编辑」仍然保留（真源是文件，用户想手改就手改）。
- **不做静默截断**：单条 > 500 字、总量 > 8000 字都是**报错**，并提示用户去界面里清理；
  重复内容（忽略 `- `/`[日期] ` 前缀与大小写）不会写两遍。
- **为什么是文件而不是表**：用户随时能打开看、能手改、能整篇删掉。
  代价是"按类别检索 / 谁写的审计"这类能力暂时没有（见第 6 节）。

## 5. 接口一览（新增）

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/api/ai/skills` | 列出 Skills（含非法诊断、冲突提示） |
| GET | `/api/ai/skills/{name}` | 元数据 + 正文 |
| POST | `/api/ai/skills` | 注册 / 覆盖（界面用） |
| DELETE | `/api/ai/skills/{name}` | 删除（仅用户来源） |
| GET | `/api/ai/skills/roots` | **投放口位置**（`userRoot` / `builtinRoot`），界面显示它 |
| POST | `/api/ai/skills/rescan` | 重新扫描投放口 + 自动登记没有 frontmatter 的 skill |
| GET / PUT / DELETE | `/api/ai/memory` | 长期记忆：读 / 整篇替换 / 清空 |
| POST | `/api/ai/memory/entries` | 追加一条记忆 |
| POST | `/api/ai/memory/delete` | 按下标删除若干条记忆（界面的单条 / 批量删除；用 POST 而不是带 body 的 DELETE） |
| GET | `/api/ai/tools` | 工具清单 + 生效权限 |
| GET / PUT | `/api/ai/tools/policy` | 读 / 改白名单、逐工具权限、轮数与调用上限 |
| POST | `/api/ai/tool-calls/{callId}/approve` \| `/deny` | 工具卡上的批准 / 拒绝 |

SSE 新增事件：`reasoning.delta`、`tool.requested`、`tool.started`、`tool.completed`、`tool.failed`；
`message.completed` 增加 `parts`（有序块，界面按它定稿）与 `steps`。

## 6. 已知未做 / 下一步

1. **附件可发（M3）**：与工具无关，但适配器的 `transports` 仍是空集，图片附件依旧是"正确阻断"。
2. **第三方 Skill 的 ZIP 导入 + 预览确认**（AIH-043/044）：现在装 skill 靠"拷进投放口"这一条路
   （ZIP 要用户自己解压后拷进去）。
3. **内置 Anima / H3 Skills 正文**（AIH-041/042）：内置根 `<根>\skills\builtin` 里现在有五份
   （**随项目分发**，只读、不可删）：`img2img-reference`（图生图三步）、`krea-2`（Krea 2 提示词 +
   ComfyUI 参数）、`comfy-workflow-audit`（工作流审核的判定与修法）、`inpaint-composite`
   （图像编辑：底图 + 遮罩 + 两阶段贴人）、`workflow-recipe`（换新模型：学配方 / 绑模型 / 四档结论）；
   用户自己的 16 个（Anima / H3 / Music3 等）仍在投放口里 ——
   **别人的 skill 正文能不能随项目分发要单独确认许可**，所以它们刻意不进 `builtin`。
   `krea-2` 是唯一一份"官方 skill 落地"的样板：正文按 Krea 官方材料适配本机 ComfyUI 路线，
   官方原文逐字放在 `skills\builtin\krea-2\references\`，出处 / 采集日期 / 许可见那里的 `SOURCES.md`。
4. **"记住这次允许"**：现在每次 `ask` 都要点一次；之后可以在工具卡上给"本次会话都允许"。
5. **审批与工具卡的断线续传**：Run 被后端重启打断时，等待中的审批会随 Run 一起失败（如实报错，不会静默执行）。
6. **长期记忆的进阶**：按类别分组、命中检索（现在是全量注入 + 截断）、"这条是谁写的"审计。
7. **Civitai 搜索只做了"查"**：查到的模型信息现在由模型自己调 `remember` 登记进长期记忆，
   还没有一张**结构化的模型表**（用户建议 ⑦ 里"登记在数据库中"的完整形态）。
   要做的话，落点是一张 `ai_model_notes`（civitai_id / name / type / baseModel / description /
   触发词 / 更新时间），再让 `comfy_find_workflow` 这类工具按类型反查。
8. **自动"摘除无关节点 / 拼装必要节点"**：现在只有**识别**（`comfy_check_inputs` 把待补输入列清楚）
   + **补线**（`comfy_submit(connections=…)`）。真正"按用户给的条件重写一份工作流"还没做 ——
   那是改写用户的图，得先有"改了哪里、怎么回退"的呈现（第 13 条纪律已经在要求"用覆盖、不重写"）。
9. **`ask_user` 式提问**：用户建议 ⑥ 想要"像 DSH 一样支持提问、回答时不中断对话"。
   DSH 那边是 `ask_user_question` 工具 + 一个暂停/恢复的 Run 状态机；本项目现在靠
   "模型在正文里问、用户在下一条消息里答"，没有真正的挂起态。
   这一轮的改动是**纪律层**的（系统提示 v18 第 9.1 条：要问就问在该动手之前，
   以问句收尾并停止调工具）。真要做挂起态，落点是 `ai_runs.status` 加一个 `waiting_user`
   + 前端在气泡里给一组可点的选项。
10. **Civitai 令牌暂时只能走环境变量**：后端凭据接口只有 Keenable 那一条
    （`/api/ai/web-search/credential` 写死了 `KeenableSearch.CREDENTIAL_REF`），
    前端也照着它写的。所以 `civitai_search` 的令牌目前只有 `CIVITAI_API_KEY`
    （或 `COMFYHUB_CRED_civitai`）这一条路，**设置页里填不了**。
    要补的话，把那条接口参数化成一个 `ref`（路由 `/api/ai/credentials/{ref}`）再加一张卡即可。
11. **Civitai 线上实测（2026-09-20，本机实打 civitai.com / civitai.red）**：这是在
    "HTTP 层只拿本机假服务钉过"之后补的一次真机核对，结论按"能照做 / 要改口径"分开：

    | 核对项 | 结果 |
    | --- | --- |
    | `items[].modelVersions[0].baseModel` | ✅ 真的有（`"SD 1.5"`），解析写对了 |
    | `items[].stats.downloadCount` / `thumbsUpCount` | ✅ 真的有，数字正确 |
    | `items[].creator.username`、`nsfw`（bool）、`description`（HTML） | ✅ 与解析一致 |
    | `items[].baseModels`（顶层数组） | 🆕 线上也有（一个模型可跨多个底模）；已在 `baseModel` 缺失时兜底 |
    | `sort=Most Downloaded` + `period=AllTime` | ✅ 200，且按下载量降序（`Most Reactions` 反而 400） |
    | `types=` 的取值 | ✅ `TYPES` 里 8 个值全 200；但**大小写敏感**（`lora`/`Lora` 都 400）→ 已在本地归一成官方拼写 |
    | `limit` 上限 | 上游是 **100**（`101` 就 400）；工具自己的上限仍是 20（那是"别白烧 token"的取舍，记在 `UPSTREAM_MAX_LIMIT`） |
    | 中文查询词 | ✅ 200（`query=水墨风` 照 URL 编码发即可） |
    | NSFW 是否"必须令牌 / 必须 `.red`" | ❌ **不是**：`.com?nsfw=true` **匿名**就返回 `nsfw=true / nsfwLevel=60` 的条目，与 `.red` 同查询逐条一致；`.red` 匿名同样 200，连**假的** Bearer 令牌也不会被拒 |

    最后一行是**口径**问题，不是 bug。2026-09-20 用户当场做了决定：**放宽**——
    令牌不再是硬门槛（`.red` 匿名照搜），但结果正文里会写明"本次是匿名通道"，
    系统提示 v19 与工具描述同步改掉"需要令牌"那句。原来那条
    `CIVITAI_TOKEN_REQUIRED` 硬拒绝已经删掉：它拦下的是**上游本来就会答应**的搜索。


---

## 24. 模型能力目录的自动更新（2026-09-23"全做了"）

模型导入时的模态 / 工具 / 思考档位预填，原来只靠**编进 jar 的内置表**（`ModelCapabilityCatalog`），
新模型要等发版。现在 `lookup()` 按**逐个维度**合并三层（与"接口声明 > 目录"同一哲学）：

| 层 | 位置 | 谁写 | 匹配 |
| --- | --- | --- | --- |
| 手工 custom | `<storage>\ai\capabilities\custom.json` | 用户（界面/手改），最高优先 | 精确 id（去前缀须唯一） |
| 在线 remote | `<storage>\ai\capabilities\remote.json` | `CapabilityRefresh` 拉 **models.dev** | 精确 id（去前缀须唯一） |
| 内置 builtin | 代码里的家族规则表（118 条） | 随版本发布 | 包含匹配（含档位/方言） |

- **自动刷新**：启动读一次覆盖层；后台 daemon 每小时看一眼，remote.json 超 **7 天**才真出网
  拉取；`COMFYHUB_NO_DOWNLOAD=1` 一刀关。拉取失败旧文件原样保留（原子写）。
- **实测校准（TESTED）**：档位阶梯的线上表达没有标准接口，只有真发请求才知道。设置页
  「AI 模型与凭据 → 模型能力目录」打开「允许实测校准」后可触发（默认关）：对每个 Provider
  最多 8 个推理模型，从最高档降档发 `reasoning_effort` 极小请求，把**网关实际接受**的档位写回
  （4xx 与思考参数无关时如实中止；`manual` 行绝不覆盖；只把 builtin 行升为 tested）。
- **可见性**：设置页卡片 + `GET /api/ai/capabilities/status` + `comfyhub.ps1 doctor` 都报新鲜度。
  接口自述（DISCOVERED）永远优先 —— models.dev 只是"网关什么都没说"时的兜底。
