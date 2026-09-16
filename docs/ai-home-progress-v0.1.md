# AI 工作台实施进度（v0.1 实施记录）

> 日期：2026-09-15 起，2026-09-16 追加（思考强度 + token 统计 / 用户清单收尾），
> **2026-09-16 第二轮追加（工具循环 M4 + Skills M5 + 退出收尾 + 内置模型目录 + 长列表卡顿）**，
> **2026-09-16 第三轮追加（用户新清单 5 个 bug + 2 条建议：会话/草稿、滚动条、卡顿、Markdown 表格、
> Skills 投放口、长期记忆 M6）**
> 依据：`docs/ai-home-requirements-v0.1.xlsx`（需求清单 / 待确认决策 / 风险清单 / 里程碑）
> 与 `docs/ai-home-implementation-plan-v0.1.md`（实施方案）
> 已覆盖范围：**M0 安全前置 + M1（Provider / 模型 / 凭据）+ M2（Run / 统一 SSE / 三协议）
> + 思考强度与 token 统计（AIH-056 / AIH-057）+ **M4 工具循环（AIH-033~036 / 046 / 049）
> + M5 Skills（AIH-037~045）** + **M6 长期记忆** + 用户清单 `docs/bug-and-suggestion-9.16.md` 全部条目**

## 0. 一句话现状

**已经能用真实 Base URL + API Key 配对并流式对话**（OpenAI 兼容 / Anthropic / OpenAI Responses），
可以在聊天框里直接切模型**和思考强度**、看到每轮消耗的 token；
**助手现在会真的调工具**（查 ComfyUI / 在 ComfyUI 目录内读写文件 / 注册与加载 Skills / 记长期记忆，
需要批准的工具会弹批准卡），
**Skills 落盘、即时生效、右侧栏可删，装 skill 就是把文件夹拷进"投放口"**，
**长期记忆跨对话生效、用户随时能看能改**；附件（图片等）仍然是"正确阻断"而不是能发（M3 未做）。

## 1. 本轮完成的提交

| 提交 | 内容 |
| --- | --- |
| `需求基线…` | 需求 xlsx 与实施方案入库；修复 `Content-Disposition` 转义 |
| `AI 工作台 M1 地基…` | 安全收紧（回环监听 + CORS 白名单）、Provider/模型/凭据领域、附件准入规则、`/api/ai/*` |
| `AI 工作台成为默认首页…` | 三栏工作台、会话/消息持久化、附件阻断提示、默认导航切换 |
| `文档同步…` | AGENTS 第 3 节、README 功能/目录/接口表 |
| `AI 模型设置页…` | Provider 配置界面、模型目录编辑、只写密钥界面 |
| `思考强度…` | 模型声明可选档位；三协议按方言各自落到正确字段；token 统计归一化（AIH-056 / AIH-057） |
| `markdown 渲染…` | 助手回复不再吐 `**加粗**` 原文；自研流式安全解析器 + 8 项单测 |
| `设置页：AI 模型与凭据挪到最前面…` | 它是最常去的入口，原先压在自动捕获 / 库统计后面 |
| `openai-responses 协议真正实现…` | AIH-004：请求体 / 流事件 / 端点（原来拼成 `/chat/completions`）+ 5 项契约单测 |
| `修「获取完可用模型并加入后，退出重进模型全消失」…` | 真机复现并修复：详情面板只在 `initState` 拷目录，父级异步补上时不更新（补 `didUpdateWidget` + `_dirty` 保护） |
| `视频播放：长边铺满 + 全屏 + 封面预览图` | 预览区不再固定 16:9/420 高；全屏页接着进度播；新增 `/api/media/{id}/poster`（Windows 缩略图管线抽帧，不依赖 ffmpeg） |
| `内置模型目录：模态 + 思考档位一起预填…` | 内置目录新增 `thinkingEfforts` / `thinkingFormat`，"获取可用模型"时随候选下发 |
| `修两个 bug + 长列表卡顿：视频全屏、画廊删除、懒构建` | 见第 4.1 节 ①②③ |
| `AI 工作台：冷启动新会话、空会话自清、草稿保留、记住上次模型、状态栏去「上下文」` | 见第 4.1 节 ④⑤⑥⑦ |
| `未关联提示词标记 + 一键清除；新建 Provider 的凭据引用名默认全大写` | 见第 4.1 节 ⑧ 与 4.2 节 |
| `连接测试 / 获取模型：/models 地址自动回退，404 不再被当成「Key 报错」` | 见 4.3 节（用户报的"填 API Key 报错"最可能的原因） |
| `AI 工具循环（M4）+ Skills（M5）+ 退出收尾 + 内置模型目录 + 卡顿修复` | 见 4.6 节 |
| `修两个会话 bug：开 App 冒出多条「新对话」+ 打了一半的字切走就没了` | 见 4.8 节 ①② |
| `Markdown 渲染：表格支持 + 粗体里嵌行内代码不再吐出星号` | 见 4.8 节 ⑤ |
| `AI 模型与凭据：69 个模型不再卡顿，滚动条滑块也不再越滚越短` | 见 4.8 节 ③④ |
| `Skills 改成「投放口」+ 引入长期记忆（用户建议第 6、7 条）` | 见 4.8 节 ⑥⑦ |

## 2. 逐条对照需求

| 需求 | 状态 | 说明 |
| --- | --- | --- |
| AIH-001 默认落地页 | ✅ | `HomeShell` 默认 `AI 工作台`，顺序 AI 工作台→画廊→提示词→标签→设置；`home_nav_test` / `localization_test` 已同步 |
| AIH-002 宽屏三栏 / 窄屏 | ✅ | ≥900 三栏，≥1200 才显示右侧栏；窄屏会话进抽屉、状态进底部 Sheet，Composer 常驻 |
| AIH-003 openai-completions | ✅ | 文本流 + 多轮历史真的能聊；`SseAccumulator` + `OpenAiCompletionsAdapter`，适配器有契约单测 |
| AIH-004 openai-responses | ✅（文本流，待真实 API 实测） | 已实现：顶层 `instructions`、`input[{role,content:[{type:input_text/output_text,text}]}]`、`store:false`、思考落 `reasoning{effort,summary}`；事件 `response.output_text.delta` / `reasoning_summary_text.delta` / `completed`（取 usage+id）/ `failed`。**端点也修了**：原先拼成 `/chat/completions`，现在走 `/responses`。契约单测 5 项 |
| AIH-005 anthropic-messages | ✅（文本流） | system 顶层 + `max_tokens`、`content_block_delta`/`message_start`/`error` 事件，有契约单测 |
| AIH-006/007 Provider 自定义 + revision | ✅ | ID 校验（kebab-case、创建后不可改）、URL 校验与规范化、乐观锁冲突返回明确错误 |
| AIH-008 连接测试 | ✅ | `POST /api/ai/providers/{id}/test`：请求前 SSRF 复核、不跟随重定向、10 秒超时、状态码映射到稳定错误码；日志只记 Provider/端点/状态码，**不含密钥与 Header**（有单测） |
| AIH-009 模型发现 | ✅ | `POST …/discover-models`：兼容 `data[]` 与 `models[]`；**能力自动预填**并按可信度分级：接口声明(discovered) → 内置目录(builtin，标注可能过期) → 未识别(unknown，仅文本)；不落库，用户确认后才进目录 |
| AIH-010/011 手工能力声明、不猜能力 | ✅ | 模型目录是能力真源；UI 与预检都以目录为准，未知模态直接拒绝。发现阶段会预填能力，但**来源全程可见**（接口声明/内置目录/未识别），内置目录命中不算"猜"，未识别的模型一律只给文本（有守护断言） |
| AIH-012/013 凭据只写 + set/describe/unset | ✅ | 所有 DTO 只含 `{configured, source, writable}`；`resolve` 仅后端内部 |
| AIH-014 改密钥下次请求生效 | ✅ | Run 开始时才 `resolve`，改密钥不影响已开始的 Run，下一次请求立即生效 |
| AIH-015 Windows 凭据方案 | ✅ | DPAPI(CurrentUser) 加密落盘；DPAPI 不可用时**写入直接失败**，绝不退化为明文 |
| AIH-016 监听/CORS 收紧 | ✅ | 默认 `127.0.0.1`；`COMFYHUB_ALLOW_REMOTE=1` 才对外；CORS 由 `anyHost()` 改为本机 + 白名单 |
| AIH-017 SSRF 保护 | ✅（保存时 + 每次请求复核） | 地址范围判定、信任级别匹配、云元数据地址任何级别都拒绝、DNS 解析后复核 |
| AIH-018 会话 CRUD | ✅ | 新建/改名/归档/删除；删除走外键级联（已验证） |
| AIH-019 有序消息块 | ✅ | `ai_message_parts`（text/attachment/tool_call/tool_result），按 `seq`+`ordinal` 无损恢复 |
| AIH-020 Run 独立实体 | ✅ | `POST /conversations/{id}/runs` → `202 + runId`；后台协程执行；启动时把遗留 `running` 标成失败 |
| AIH-021 统一 SSE + seq 续传 | ✅ | `GET /runs/{id}/events?after=seq`；事件全部落库（`ai_run_events`），进程重启后仍可回放；带 15 秒心跳 |
| AIH-022 取消 | ✅ | `POST /runs/{id}/cancel` → 取消协程 → `runInterruptible` 打断阻塞读 → 上游连接关闭，Run 记 `cancelled` |
| AIH-023 快照 | ✅ | Provider/模型快照 + `promptVersion` 随 Run 保存，**快照不含密钥**（只有引用名） |
| AIH-024 稳定错误码 + 有限重试 | ✅ | `MISSING_CREDENTIAL / UNKNOWN_MODEL / RATE_LIMIT / QUOTA_EXCEEDED / CONFIG_ERROR / PROTOCOL_ERROR / ABORTED / PROVIDER_UNREACHABLE / UNSUPPORTED_CONTENT`；失败/被取消的回复上直接给「重试」：**新建 Run** 并用 `retryOfRunId` 关联回原 Run，重放原来的提问与思考强度 |
| AIH-027 严格文件识别 | ✅（分类器 + 预检接入） | `FileKindDetector` 签名优先、未知即 UNKNOWN；`StrictIntake` 让**服务端判定压过前端声明**（前端谎报 image 骗不过准入，有单测）。上传落盘链路要等 M3 |
| AIH-028 能力矩阵 | ✅ | 模型声明 ∩ 适配器实现 ∩ MIME ∩ 大小/数量，任一不满足即阻断（有单测）；适配器 `transports` 是唯一事实来源 |
| AIH-029/030 前后端阻断、零上游请求 | ◐ | 前端以后端 preflight 为准并禁用发送，`preflight` 是纯计算；Run 准入已在创建 Run 前做能力校验，但"事务内快照再验一次"要等附件真正可发（M3） |
| AIH-048 能力徽标 | ✅ | 模型选择器与侧栏都按目录声明显示，未声明的一律标不支持 |
| AIH-053 首页 Widget 测试 | ✅ | `test/ai_home_test.dart`（含流式发送）、`test/ai_provider_settings_test.dart` |
| AIH-055 文档同步 | ✅ | AGENTS 第 3 节 + README 功能表/目录树/API 表/协议现状表/测试表 |
| AIH-056 思考强度 | ✅ | 模型目录声明可选档位（`thinkingEfforts`）+ 网关方言（`thinkingFormat`）；聊天框选择器只列声明过的档位；Run 记录**生效值**；三协议方言分别适配（见第 6 节） |
| AIH-057 token 统计 | ✅ | 后端把各家 `usage` 归一化成 input/output/cached/reasoning；助手消息显示单轮用量，输入区显示本对话汇总；历史老数据（供应商原始 usage）也能回算 |
| AIH-033 `comfy_get_status` | ✅ | 复用 `ComfyCapture.status()`（连通性 / 队列 / 最近捕获）；**不接受任意 URL**；只读、免审批 |
| AIH-034 `comfy_get_run` | ✅ | 按 `runKey` 查捕获记录（给 `CaptureRepo` 加了 `findRun`）；查不到如实报 `NOT_FOUND` |
| AIH-035 `comfy_sync_history` + 审批 | ✅ | 默认 `ask`：工具卡上点「批准 / 拒绝」才执行（`POST /api/ai/tool-calls/{callId}/approve|deny`）；超时 5 分钟或 Run 取消 = 拒绝；复用 `pollOnce` 的并发锁 |
| AIH-036 限制主动轮询 | ◐ | 轮数上限 8 / 单 Run 调用上限 16 / 取消即停都做了；"一次回复最多主动查询 3 次"这个**按工具类别**的细分没做（现在是统一预算 + 提示词纪律） |
| AIH-037 扫描 Skills 根目录 | ✅ | `<项目根>\skills\builtin` + `<storage>\ai\skills`，只扫根下一层（bundle/SKILL.md 或平铺 .md）；同名用户版胜出并标冲突 |
| AIH-038 frontmatter 严格校验 | ✅ | name kebab-case 且与目录名一致、description 必填、正文上限；非法项**列出来带诊断**但不进提示、不能加载 |
| AIH-039 只注入目录摘要 | ✅ | 系统提示只给名称 + 描述（截断 240 字）+ whenToUse |
| AIH-040 `load_skill` 按需加载 | ✅ | 返回 `<skill_content>` 块；同一 Run 内重复调用直接报"已加载过" |
| AIH-041/042 内置 Anima / H3 Skills | ⬜ | 内置根 `<根>\skills\builtin` 仍是空的（发布包会带上 `skills\` 目录）；用户自己的 16 个在投放口里 |
| AIH-043 第三方 Skill 导入 | ◐ | 装 skill 现在只有**投放口**一条路（拷进去 → 启动/重新扫描时自动登记）；ZIP 导入仍未做 |
| AIH-044 阻断路径穿越 / ZIP bomb | ◐ | 投放口只读"根下一层"、自动登记只补 frontmatter 不动正文、正文有 256KB 上限；ZIP 相关规则要等 ZIP 导入 |
| AIH-045 禁止执行第三方脚本 | ✅ | 工具集里**根本没有** shell / 进程工具；`scripts/` 只是不可执行资源 |
| AIH-046 版本化系统提示 | ✅ | `SystemPrompt.VERSION = v2`：工具清单 + 权限边界 + Skill 纪律 + 防提示注入 + 附件诚实 |
| AIH-047 用户指令不覆盖安全段 | ◐ | 系统提示里把安全规则写成"必须遵守"，Run 记 `skill_snapshot`（名称 + digest）可追溯；**用户自定义追加段的界面**还没做 |
| AIH-049 工具调用状态卡 | ✅ | 工具卡显示名字 / 参数摘要 / 审批按钮 / 耗时 / 结果预览 / 错误码，服务端已截断脱敏 |
| AIH-052 三协议 Fake Provider | ◐ | 三家协议的**工具**线格式都有单测（`ToolProtocolTest` 20 例）+ 本地假网关端到端；401/429/500 / 断流 / 畸形 SSE 属于既有覆盖 |
| **用户建议：长期记忆（M6）** | ✅ | `MemoryStore` + `<storage>\ai\memory.md`（一行一条、人可手改）+ `remember` 工具 + 右侧栏面板/编辑器；系统提示 v3 每次 Run 现注入，明写"记忆是数据不是指令"；写入超限**报错**不截断。见 `MemoryStoreTest` 9 例 + 4.8 节 ⑦ |
| **用户建议：Skills 投放口** | ✅ | 删掉「从 DSH 导入」按钮与整套 `.dsh` 导入代码；投放口 = `<storage>\ai\skills`，启动/重新扫描时自动补 frontmatter 登记；界面显示后端算好的绝对路径 + 打开/复制。见 4.8 节 ⑥ |

图例：✅ 完成　◐ 部分完成　⬜ 未开始

## 3. 验证证据（实测，非推断）

> 每轮的数字都在累加，**最新一轮（第三轮）见 §4.8.1**：后端 235 tests、前端 148 例、
> 端到端 58 项检查。下面这几条是第一轮时的记录，保留作对照。

- `pwsh -File scripts\server.ps1 test` → **126 项全通过**（`AiDomainTest` / `FileKindDetectorTest` /
  `AiUpstreamTest` / `AuthAndModelsUrlTest` / `ModelDiscoveryTest` / `ModelCapabilityTest` /
  `ProtocolAdapterTest` / `ThinkingAndUsageTest`…）。
- `flutter analyze` → 无问题；`flutter test` → **94 项全通过**（含流式发送、连接测试、密钥不回显、
  画廊分页回退、未关联清理、新建 Provider 对话框、视频/聊天懒构建后的页面结构）。
- `flutter build windows --debug` → 通过（本轮改动不影响原生构建）。
- **端到端对话（用本地假 OpenAI 服务，真跑 HTTP + SSE，不联网、不花钱）**：
  - 建 Provider（loopback）→ 写密钥（DPAPI）→ 连接测试 `ok=true, modelCount=2`；
  - 模型发现返回 2 个候选，**目录里仍是 1 个**（证明发现不落库）；
  - 发起 Run：**48 个事件**，序列 `run.started → message.started → reasoning.delta → text.delta×42 → usage.updated → message.completed → run.completed`；
  - 助手回复完整落库，`ai_run_events` 48 行，Run 状态 `completed`、`promptVersion=v1`；
  - 第二轮追问后会话里 4 条消息（历史被带进上下文）。
- 之前几轮实测过：凭据状态只返回 `{configured, source}` 无值、revision 冲突、会话级联删除后三表 0 行、分类器对谎报扩展名的文件阻断、`e2e-capture-test.ps1` 全通过。
- 顺带修掉一个真实缺陷：删除接口原先返回 `mapOf("deleted" to true, "id" to id)`，kotlinx 不支持元素类型不同的集合，运行时会 500。

## 4. 未完成 / 下一步（按建议顺序）

1. **附件真正可发（M3）**：实现图片内联（openai-completions 的 `image_url` data URI）后，
   把适配器 `transports` 从空集改成实际实现，并在 Run 准入处用事务内快照再验一次（AIH-030 的"上游请求数为 0"用例）。
   现在的行为是**正确阻断**，不是静默丢弃。
2. **`openai-responses` 真实 API 实测**（AIH-004）：协议已按官方结构实现、契约单测 + 本地假网关端到端
   都过了，但**还没拿真实 API Key 跑通过一条完整流**（用户暂时没有可用 Key）。
   要确认的点与已修的"填 Key 报错"见第 4.3 节。
3. **工具循环的"真实网关实测"**（M4 已完成，缺真实环境验证）：三家协议的**假网关**端到端
   （`scripts\e2e-ai-tools-test.ps1`）已覆盖工具调用、审批、越界拒绝、注册/加载 Skill；
   但仍没用真实网关跑过一轮工具调用（不同网关对 `tools` / `tool_calls` 的方言差异最大）。
4. **Skills 的第三方 ZIP 导入 + 预览确认**（AIH-043/044）：现在装 skill 只有"拷进投放口"这一条路
   （ZIP 要用户自己解压后拷进去）。
5. **内置 Anima / H3 Skills 正文**（AIH-041/042）：内置根 `<项目根>\skills\builtin` 目前是空的，
   用户自己的 16 个在投放口里（发布包会带上 `skills\` 目录，但正文能否随项目分发要单独确认许可）。
6. **工具审批的"本次会话都允许"**：现在每次 `ask` 都要点一次批准。
7. **长期记忆的进阶**：按类别分组 / 命中检索（现在是全量注入 + 截断）、"这条是谁写的"审计。
8. 事件表保留策略：`AiRunRepo.pruneEvents()` 已写好但还没接到定时任务。

## 4.1 用户提的 bug / 建议清单（`docs/bug-and-suggestion-9.16.md`）对照

**本轮已全部做完**（清单里 8 条 + 1 条追加建议）：

| # | 条目 | 状态 / 做法 |
| --- | --- | --- |
| ① | bug：视频页两个全屏按钮、点全屏从头播、Esc 退不出 | ✅ 右上角那个悬浮全屏按钮删掉（只留控制条一个）；全屏前**暂停内嵌那一路**并记下位置，全屏页退出时回传最后进度续播（原来两路同时出声、回来还停在旧位置）；全屏页补 `Focus` 焦点锚点 + `DismissIntent`，Esc 真的能退出；控制条在全屏页改白字 |
| ② | bug：删除数量大于单页承载时，画廊提示为空 | ✅ 成因是删除后**当前页码越界**，后端对越界页码返回空数组。`LibraryStore.refreshMedia` 发现"items 空但 total>0 且页码超页数"就回到最后一页重取一次；提示词列表同样处理。回归用例 `test/gallery_paging_test.dart` |
| ③ | 建议：滑动长列表卡顿（AI 模型与凭据） | ✅ 模型卡片原来用 `children: [... for i in models]` **一次性建出全部卡片**（每张 6 个 FilterChip + 一个下拉框），改成 `ListView.builder` 懒构建 + `RepaintBoundary`；聊天页每条消息拆成独立组件加 `RepaintBoundary`，流式刷新不再重绘所有 Markdown 气泡 |
| ④ | 建议：冷启动后 AI 工作台应是新建聊天记录 | ✅ `AiWorkspaceStore.load()` 每次都新建一条会话（历史仍在左侧列表），不再自动打开上一条 |
| ⑤ | 建议：内容为空的聊天记录，切换时可以删掉 | ✅ 切换 / 新建会话前把"没有消息且输入区没有待发送内容"的会话删掉；**有草稿或有消息的一律保留**（草稿按会话存在本地，见 ⑥） |
| ⑥ | 建议：记住上一次使用的模型 | ✅ Provider / 模型 / 思考强度记进 SharedPreferences（**不碰密钥**），下次开 App 自动选回 |
| ⑦ | 建议：去掉「上下文」字样，刷新按钮放 ComfyUI 右边 | ✅ 右侧栏不再有含混的「上下文」标题；刷新按钮移到「ComfyUI」字样右侧（它刷的本来就只有 ComfyUI 状态） |
| ⑧ | 要求：产物删除后提示词显著标「未关联」+ 批量管理一键清除未关联产物 | ✅ 提示词卡片上 `mediaCount == 0` 时显示橙色「未关联」标记（带 tooltip 说明是"产物被删掉或还没关联"）；工具栏新增「未关联产物」筛选（走 `hasMedia=0`）与「一键清除未关联」；画廊侧另有「清除未关联产物」清掉孤儿产物。两者都**先报条数 + 举例再确认**，删除按批循环（超过单页 200 条不会漏） |
| ⑨ | 追加建议：新建 Provider 的凭据引用名实际必填 → 默认值取 Provider ID 全大写 | ✅ 改为**程序自动处理**：默认值 = Provider ID 全大写、`-` 换成 `_`、末尾加 `_API_KEY`，跟着 ID 实时变；只有用户**主动改过**才停；界面上标「可选」，留空提交也会自动补默认值（只有 ID 以数字开头时才需要手填）。 |
| ⑩ | 追加要求：调查 `%USERPROFILE%\.dsh\settings.yaml`，把常用模型的模态与思考支持预填进去 | ✅ 见 4.4 节 |

### 4.2 别人的两条"其他建议"怎么处理的

| 条目 | 处理 |
| --- | --- |
| 调试用 flutter debug + 热重载 | 已经是仓库约定（`AGENTS.md` 第 1 节 + `scripts/dev-app.ps1`）；本轮改动的验证走 `flutter analyze` / `flutter test` / `server.ps1 test`，没有每次 Release 全量构建 |
| 减少截屏 / 读图次数 | 采用：本轮 UI 改动全部用 widget 测试断言（结构 + 发出的请求），没有靠截图确认 |
| 先读 `%USERPROFILE%\.dsh\settings.yaml` 预填模型模态与思考强度 | ✅ **本轮真做了**，见 4.4 节 |

### 4.4 `%USERPROFILE%\.dsh\settings.yaml` 调查结论（本轮新增）

那份文件是本机在用的**权威声明**：一个 provider（`command-code-goat`，
`https://api.commandcode.ai/provider/v1/`，`openai-completions`）下面挂了几十个模型，
每个条目带 `input`（输入模态）与 `reasoningEfforts`（思考档位）。调查到三件事：

1. **思考等级其实有 7 档**，我们原来只有 5 档：
   pi-ai / DSH 的顺序是 `off → minimal → low → medium → high → xhigh → max`。
   我们缺 `minimal` 与 `xhigh`，后果很实际 —— **GPT-5.5 / 5.4 / 5.3-codex / Grok 4.6 /
   Muse Spark / Fugu Ultra / Qwen3.8 这些只声明到 `xhigh`**，原来只能被压回 `high`，
   用户以为选了"极高"、实际发出去的是"高"（比报错更糟）。
   → `ReasoningEffort` 补 `MINIMAL` / `XHIGH`（后端 + 前端 `AiReasoningEffort` 同步），
   `ThinkingLevels.DEFAULT` 补 `minimal→minimal`、`xhigh→xhigh`，
   Anthropic 预算表补 1024 / 24576，Responses 适配器改成**显式映射、不再降级**。
2. **用户对"别人替我决定"的容忍度**：DSH 的做法是——模型条目没声明 `reasoningEfforts` 就
   **继承安装目录（catalog）里的能力**；声明了就必须逐档写清楚（没写的档位=不支持）。
   我们的 `ModelCapabilityCatalog` 就是扮演那个 catalog，所以本轮把它按 settings.yaml 补齐。
3. **同系列里存在视觉差异**：`deepseek-v4-flash-vision-exp` 有图、`deepseek-v4-pro` 没有；
   `glm-5.3` 有图、`glm-5.2` 没有；`mimo-v2.5` 有图、`mimo-v2.5-pro` 没有。
   → 内置目录按**具体变体**登记，不再只用宽前缀。

补齐后的表覆盖：Claude 4.6/5、GPT-5.x、DeepSeek V4.x、Kimi K2.5~K3、GLM-5.x、
MiniMax M2/M3、Qwen3.6~3.8、Gemini 3.x、Grok 4.5/4.6、Muse Spark、MiMo、Step、Hy、
Inkling、LongCat、Nemotron、Ling、Laguna、gpt-oss 等（`VERSION = 2026-09b`）。
回归用例 `ModelCapabilityTest` 里有一张**逐模型对照表**（27 个条目），
以后 settings.yaml 变了、或者表写错了，跑一遍后端测试就会报是哪一条对不上。

> 仍然是"预填建议"，不是真相：接口声明 > 内置目录 > 兜底（**逐维度**合并，界面标来源、可手改）。

### 4.5 模型发现：逐维度合并 + 工具默认给上（2026-09-16 补）

用户实测发现："自动获取模型，接口没有声明模态和思考强度，怎么还不改成内置目录的呢"。
查出来是 `parseDeclaredCapabilities()` 的判定太宽 —— 只要 JSON 里出现任何一个已知字段名
（`capabilities`、`reasoning`、`vision`……）就把整个模型标成"接口声明"，于是内置目录
**永远不会被查到**。真实网关的 `/models` 多数只回 `{id, object, owned_by}`，
少数会带上 `capabilities: {}` 这种空对象 —— 恰好都触发这条错路。

改法：

| 维度 | 接口明确声明 | 接口没说 | 两边都没有 |
| --- | --- | --- | --- |
| 输入模态 | 用接口的 | 用内置目录 | 仅文本 |
| 思考支持 + 档位/方言 | 用接口的（档位仍由内置目录预填） | 用内置目录 | 不支持 |
| 工具 | 用接口的 | 用内置目录 | **默认给上**（用户要求） |

- "接口说了"改成**三态**：`null` = 没说这一项；只有真的读到数组/布尔值才算说了。
  `capabilities: {}`、`reasoning: false` 这种只影响它自己那一项，不再一票否决内置目录。
- **所有模型默认提供工具支持**：`ModelCandidate.tools` 默认值、`ModelCapabilityCatalog.UNKNOWN.tools`、
  设置页"手动添加"对话框的初始勾选全部改成 true。这么做有两个前提：
  ① 很多网关不声明工具能力但实际支持；② **当前请求体根本不发 `tools`**
  （工具循环属于 M4，还没实现），所以这个声明只影响界面显示与预检，不会让请求失败。
- 来源标记也跟着变：接口说过能力 → 「接口声明」；只有内置目录命中 → 「内置目录」；
  两者都有时说明里会写"模态/思考按接口声明，其余维度参考内置目录规则「xxx」"。
- 新增/更新用例：`ModelDiscoveryTest`（接口只给 id → 回退内置目录；只声明一部分维度 →
  其余由内置目录补；空 `capabilities` 不算声明；未知模型"仅文本 + 工具"）、
  `ModelCapabilityTest`（接口说"没有"时优先于内置目录）。
> 表命中的模型如果档位不对，用户在模型卡片上改一下即可（改完来源会变成"手工声明"）。

### 4.6 第二轮：工具循环 + Skills + 退出收尾 + 内置模型目录 + 卡顿（2026-09-16）

这一轮对应 `docs/bug-and-suggestion-9.16.md` 的新内容（用户当天更新的那份）：
三条 bug + 两条建议。设计说明单独成文：[`docs/ai-tools-and-skills.md`](ai-tools-and-skills.md)，
DSH 工具层的逐项实测记录在 [`docs/dsh-tool-layer-report.md`](dsh-tool-layer-report.md)。

| 用户报的 | 做法 | 证据 |
| --- | --- | --- |
| **① 关掉前端时没杀掉冷启动同步启动的后端与 MySQL** | 根因：`ensureRunning()` 探到 `/api/health` 健康就早返回，**根本没跑 `up`**，于是没有任何守护进程被挂上；而服务是用 WMI 脱离进程树起的（父进程是 `WmiPrvSE.exe`），App 退出后天然存活；Dart 侧也**没有任何退出钩子**。三处修：`comfyhub.ps1` 新增 `release`（只停服务、不杀 App）与 `watch`（给"已经在用本地服务"的 App 补挂守护）+ `unwatch`（撤销守护）；`backend_launcher` 加 `releaseOnExit()` / `armOwnerWatch()` / `disarmOwnerWatch()` 与"认领"门；`app.dart` 注册 `AppLifecycleListener(onExitRequested:)` | 单测 11 例；scratch 应用实测 `onExitRequested` 在 WM_CLOSE 时确实送达且会等异步收尾；真机脚本演示：`release` 后 java=0 / mysqld=0 / 端口全关而 viewer 仍在；`watch` 后杀掉 owner，11 秒内服务全停（`.run\watch-owner.log` 可查） |
| **② settings.yaml 的模型信息还是没登记进项目** | 三段式：`scripts\gen-builtin-catalog.ps1`（开发期）把 YAML 抄成 `server/src/main/resources/ai/builtin-catalog.json`（1 provider + **69 模型**，生成结果与手抄版 **逐字节一致**，sha256 相同）；`AiSeedCatalog` 只读 classpath 副本（**运行时不读 YAML**）；`AiSeeder` 幂等落库（启动只"补缺失"，`REFRESH_CAPABILITIES` 是显式动作，不覆盖用户改过的行） | `AiSeedCatalogTest` 11 例 + `AiSeederPlanTest` 13 例；真库探针验证"只补不覆盖 / 对齐只改能力 8 列"；本机旧导入的 69 个模型能力字段全是错的 → 已用 `REFRESH_CAPABILITIES` 对齐（见 4.7） |
| **③ 大量列表快速滑动仍然卡顿** | 缩略图 `Image.network` 没有 `cacheWidth`：`/thumb` 是 512px，但**生成失败时会回退原图**，于是每格可能解码 4096²；且 `filterQuality.medium` 每张纹理都要 mipmap。改成按格子尺寸 × DPR 反推解码宽度（上限 512）、`low`、静态骨架（原来每格一个无限 ticker）、图片单独 `RepaintBoundary`；`AdaptiveColumnList` 只对**多列行**保留 `IntrinsicHeight`（单条目的行不再白跑一次固有尺寸查询）；网格关掉 `addAutomaticKeepAlives` | 新增 `test/scroll_perf_test.dart` 8 例（解码宽度随格子/DPR 变化且 ≤512、500 条只建 <60 项、单列 0 次固有高度查询、多列仍等高…）。典型 230px 格子解码从 1MB 降到 211KB，120 格不再触发 ImageCache 淘汰 |
| **建议：DSH 的工具哪些能复用 + 权限设计** | 见 `docs/ai-tools-and-skills.md` 第 1 节的对照表：采纳文件类（收敛成 4 个）与 `skill` 加载，**明确不采纳** shell / subagent / workflow / web / present；权限不照搬 DSH 的 preset，改成"写白名单（默认只有 `comfyui`）+ 逐工具 allow/ask/deny + 真实路径判定 + 四个永久禁写段" | `ToolPolicyTest` 9 例（含符号链接逃逸、白名单放宽到项目根仍拒 `.git`/`.mysql`）、`ToolRegistryTest` 16 例 |
| **其他建议：让 AI 直接注册 skill，右侧栏能删，即时生效** | `register_skill` / `load_skill` / `list_skills` / `delete_skill` 四个工具 + `GET/POST/DELETE /api/ai/skills` + `import-dsh`；正文真源在磁盘、**无缓存**（下一次回复即生效）；系统提示每次 Run 现渲染，所以"新对话生效"是自然结果 | 单测覆盖"注册后立刻可见"；端到端见 4.7 |

顺带修的：
- AI 领域异常以前会落到 `StatusPages` 的兜底分支变成 **500 + "AiException"**，现在统一成
  400 + 稳定 `code`（AIH-024 才有意义）；工具层的拒绝理由（`PATH_DENIED` 等）也照样透出。
- 网关不认 `tools` 直接 400 时，本次 Run 会自动**退回纯文本重试一次**并如实说明，
  而不是让用户以为模型坏了。
- 多轮工具循环的 token 改成**累加**（只算最后一轮会严重少报）。

#### 4.6.1 验证证据（实测）

- 后端：`pwsh -File scripts\server.ps1 test` → **216 tests / 18 classes，0 failures / 0 errors**。
  本轮新增的 90 例分布在 `ToolProtocolTest`(20) / `ToolPolicyTest`(9) / `SkillStoreTest`(18) /
  `ToolRegistryTest`(16) / `AiSeedCatalogTest`(11) / `AiSeederPlanTest`(16)。
- 前端：`flutter analyze` → **No issues found**；`flutter test` → **126 个用例全通过**
  （本轮新增 `scroll_perf_test.dart` 8 例、`backend_launcher_test.dart` 11 例、`ai_tools_ui_test.dart` 13 例）。
- 工具循环的**端到端**：`scripts\e2e-ai-tools-test.ps1` + `scripts\e2e\fake_openai.py`
  （假 OpenAI 网关，离线、不花钱）→ **49 项检查全过、exit 0**，覆盖注册 Skill / 按需加载 /
  越界写被拒 / 目录内写成功 / 审批闸门（批准前不发 `tool.started`）/ 只读工具 / 落库 parts 有序。
- 退出收尾：`release` / `watch` 真机演示 + scratch 应用验证 `onExitRequested`（见上表 ①）。
- 顺带被端到端测试抓出来的三个真 bug（都已修，见 4.6.2）。

#### 4.6.2 端到端测试抓出来的三个真 bug（都已修）

这三个都是"单测测不到、只有真跑一遍才暴露"的类型，值得记下来：

| # | 症状 | 根因 | 修法 |
| --- | --- | --- | --- |
| ① | **任何"不支持推理"的模型都发不出消息**：`POST /runs` 不带 `reasoningEffort` → `400 未知的思考强度：null` | `AiRoutes` 里 `ReasoningEffort.parse(null) ?: throw`，而 DTO 与 `AiRunRepo` 都写着"缺省按 off"；前端在模型不支持推理时**本来就不传这个字段**（本机 69 个模型里有 26 个属于这类） | 缺省/空串 → 直接落 `OFF`；非法值仍然报错（不静默降级）。E2E 脚本改成**故意不带**这个字段，当回归防线 |
| ② | 审批类的工具：用户手快在工具卡出现的一瞬间点批准 → `accepted=false`，然后一直等到 5 分钟超时被当拒绝 | `HarnessRunner` 先发 `tool.requested` 事件、**后**才 `ToolApprovalGate.open()`，中间那几毫秒里的点击打空 | 先 `open` 再 `emit`；AGENTS §10 早就写了这条规矩，代码没照做 —— 现在照做了 |
| ③ | 审批通过后，工具卡一直显示"待批准"，直到工具跑完才跳成"已完成" | `tool.started` 是在 `invoke` **返回之后**补发的 | `invoke` 加 `onApproved` 回调，批准的那一刻就发 `tool.started`（同步工具可能跑好几秒，界面要有"运行中"） |

顺带修的还有一个**真实数据**问题：从 DSH 导入的 `anima-nsfw-prompt` 描述有 762 字，
而扫描时按"超过 600 字算非法"处理 → 这个 skill 直接被禁用、进不了系统提示。
现在**只有 `save()`（AI/界面写入）才管长度上限**，扫描已有 skill 不因为描述长就判非法
（注入目录时本来就截断到 240 字）—— 见 `SkillStoreTest` 的"描述很长仍然可用"用例。

#### 4.7 本机数据的处理（重要）
本机数据库里那份 `command-code-goat` 是**前一天手工导入的旧数据**：69 个模型的能力字段
（模态全开、`reasoning=false`、`thinkingFormat=null`、`capabilitySource=manual`）与内置目录**全部不一致**，
所以"启动时只补缺失模型"在本机上不会带来任何变化 —— 必须显式跑一次
`REFRESH_CAPABILITIES`（界面上是「用内置目录对齐模型能力」按钮，先预览再确认）。
**本轮已经在这台机器上跑过一次对齐**，所以现在库里的模型声明与 `settings.yaml` 的冻结副本一致。

> 这是刻意的取舍：**启动时绝不覆盖用户改过的行**（只补缺失），对齐必须由用户点一下。
> 判断依据是 `AiSeederPlanTest` 里的 `planSeed`/`capabilityDiffers`：只有"目录里有、库里也有、
> 但能力字段不同"才算分歧，展示名、启用状态、附件上限这些属于用户地盘，永远不碰。

### 4.8 第三轮：用户新清单（5 个 bug + 2 条建议，2026-09-16）

用户当天又更新了 `docs/bug-and-suggestion-9.16.md`：5 个 bug + 2 条建议，**本轮全部做完**。

| # | 用户报的 | 真正的根因 | 做法 / 证据 |
| --- | --- | --- | --- |
| ① | 打开 ComfyHub 会产生大于 1 条新对话 | `HomeShell` 每次切页都会**重建** `AiHomePage`（不是 IndexedStack），页面 `didChangeDependencies` 又无条件 `store.load()`，而 `load()` 里含"新建一条会话"；每次开 App 新增的那条空壳在关窗口时也没人清 | `AiWorkspaceStore.load()` 改成幂等（`_loaded` + `_loading` 去重，失败允许重试）；冷启动不再无条件新建：已经有"没有消息、也没有草稿"的空会话就直接用，历史遗留的多条空壳只留最新一条（`_startFreshConversation`） |
| ② | 输入框内容切换对话后无法保存 | 切会话时先改 `_draftConversationId` 再 `_input.clear()`，`_onInputChanged` 于是用**空串**把新会话刚存好的草稿覆盖掉；另外"有草稿的空会话"会被当成空壳顺手指掉 | 页面加 `_applyingDraft` 屏蔽位 + `_setInputText()`（程序改字不触发草稿回调），取草稿用 epoch 防串台；`_cleanupEmptyConversation` 判定空壳时**连本地草稿一起看**；附件托盘也按会话归属（`_stashAttachments` / `_applyDraftAttachments`） |
| ③ | 实用大列表快速滑动依旧卡顿 | 「AI 模型与凭据」每行是一张 6 个 FilterChip + 工具 chip + 思考强度编辑器的卡片，69 行全是一屏装不下的重组件 | 行改成**固定 64px**：只留显示名 + id + 能力图标 + 编辑/移除，能力编辑搬进 `_ModelEditorDialog`；列表换 `SliverFixedExtentList`，一趟只建视口附近的行（用例断言 < 30，实测 4） |
| ④ | 右侧滚动条不能反映当前位置（从页顶往下滚会变短） | 懒构建的 `SliverList` 只能用"**已布局**子项的平均高度"估算 `maxScrollExtent`。探针实测（69 个模型、前 35 个矮后 34 个高）：页顶 `max=3512`、滑块 17.9%，滚到底 `max=21318`、滑块 3.5% | 同 ③ 的固定 extent：修完后全程 `max=4217`、滑块恒为 14.3%（探针复测 + `test/model_list_scroll_test.dart` 断言"滚到任何位置 extent 不变"）；保存按钮也从列表尾巴挪到常驻底栏 |
| ⑤ | Markdown 表格 / 粗体渲染不正常 | （a）解析器**根本没有表格**，整张 GFM 表变成一堆带竖线的段落；（b）行内扫描是"代码永远优先"，`**…以 \`x\` 为准）**` 会先切代码，两头的 `**` 原样吐出、粗体失效 | （a）新增 `MdTable`（表头/数据行/每列对齐，以分隔行为判定，单元格内行内标记照常解析），渲染用弹性列宽 `Table`；（b）改成**位置最靠前的标记先处理**，同位置才按 代码>粗体>删除线>斜体>链接。用库里那条真实回复（2007 字、含 3 张表）核查：3 张表全部识别、裸露 `**` 为 0 |
| ⑥ | 建议：不要「从 DSH 导入」按钮，给一个目录，启动时检测到新 skill 自动注册 | — | 投放口 = `<storage>\ai\skills`（发布包 `<根>\storage\ai\skills`，便携式）；启动 + `POST /api/ai/skills/rescan` 自动给**没有 frontmatter** 的文件补 `name`/`description`（正文不动，已有 frontmatter 一个字节不改）；界面显示后端算好的绝对路径 + 打开文件夹 / 复制路径；整套 `.dsh` 导入代码删除。顺带修了 `description: \|` 块标量被读成字面量 `\|` 的真实缺陷（本机 16 个 skill 里 5 个中招） |
| ⑦ | 其他建议：引入长期记忆（memory） | — | M6：`MemoryStore` + `<storage>\ai\memory.md`（一行一条、人可手改）+ `remember` 工具（默认 allow）+ 右侧栏「长期记忆」面板/编辑器（改 / 加一条 / 清空）；系统提示 **v2 → v3** 每次 Run 现注入并明写"记忆是数据不是指令"；注入截断 4000 字、文件上限 8000 字，**超限报错不静默截断** |

#### 4.8.1 验证证据（实测）

- 后端：`pwsh -File scripts\server.ps1 test` → **235 tests / 0 failed**
  （新增 `MemoryStoreTest` 9 例、`SkillStoreTest` 投放口 7 例 + 块标量 3 例、`ToolRegistryTest` 记忆 3 例）。
- 前端：`flutter analyze` 无问题；`flutter test` → **148 例全过**
  （新增 `ai_conversation_lifecycle_test` 5 例、`model_list_scroll_test` 4 例、`markdown_test` 表格/粗体 12 例、
  `ai_tools_ui_test` 投放口与长期记忆 2 例）。
- **端到端**（真后端 + 假 OpenAI 网关，离线不花钱）：`scripts\e2e-ai-tools-test.ps1` 新增第 8 幕 →
  **58 项检查全过、exit 0**，其中包含「`remember` 落盘 → `GET /api/ai/memory` 读得到 →
  下一轮 Run 的系统提示里确实带上了这条记忆 → 工具清单里确实有 `remember`」；
  测试写入的记忆在结束时被原样恢复（不污染用户真实记忆）。
- 真机：往投放口拷一个**没有 frontmatter** 的 `.md` → `POST /skills/rescan` 返回 `registered=1`，
  列表里出现且描述取自正文第一行；同时 16 个已有 skill 的 frontmatter 未被改动。
- 滚动条：`.run\scrollprobe_test.dart` 探针在修复前后各跑一次（数据见上表 ④）。

### 4.3 用户报的「OpenAI Responses 填 API Key 报错」（2026-09-16 第一轮）

**无法实测**（用户暂时没有可用 Key），做了两件能确定的事：

1. **代码走查 + 修掉最可能的一条**：Base URL 填 `https://api.openai.com` 与
   `https://api.openai.com/v1` 会拼出两个不同的 `/models` 地址，其中必然有一个 404，
   而原来的实现把 404 显示成"连接失败 / 端点或协议不匹配"——看着就像密钥不对。
   现在两个候选地址**依次试**并回报实际可用的那个；`/models` 404 时连接测试仍算**连通**
   （鉴权已经过了，只是这份端点不提供模型列表）；只有 401/403 才报 `MISSING_CREDENTIAL`
   且不再试第二个地址。
2. **把协议行为钉进测试**：`AuthAndModelsUrlTest` 真起一个本地假网关，
   覆盖"只认 `/v1/models`"、"只认 `/models`"、"根本没有 `/models`"、"401"四种情况，
   以及 `openai-responses` 的请求确实落在 `/v1/responses` 并带 `Bearer`。

**仍待用户实测**：拿到真实 Key 后按 ①②③④ 确认——
① 端点是否 `{base}/responses`；② `instructions` 是否被接受；
③ `reasoning.summary` 是否下发思考摘要；④ `response.completed.response.usage` 的字段名。
失败时界面会带上上游原文（已抹密钥），把那段话发回来就能定位。

> ⚠️ **需求 xlsx 的"状态"列不可信**：里面把没实现的需求（AIH-025~032 附件可发、
> AIH-033~045 工具与 Skills）都标成了"通过"。**以代码与本文档为准**，别照抄那一列。

## 5. 思考强度与 token 统计（AIH-056 / AIH-057，2026-09-16 新增）

### 5.1 为什么不能"直接发 reasoning_effort"

同一个"高"，不同网关落到**完全不同的字段**（`server/.../ai/protocol/Reasoning.kt`）：

| 协议 / 方言 | 开启 | 关闭 |
| --- | --- | --- |
| `anthropic-messages` | `thinking{type:enabled,budget_tokens}`，且 `max_tokens` 必须**严格大于**预算 | 不发 `thinking`，`max_tokens` 回默认 |
| `openai` + `openai`（默认） | `reasoning_effort: "high"` | **什么都不发**（很多网关不认 `"none"`） |
| `openai` + `deepseek` | `thinking{type:enabled}` + `reasoning_effort` | `thinking{type:disabled}` |
| `openai` + `qwen` | `enable_thinking: true` + `reasoning_effort` | 不发 |
| `openai` + `zai` | `thinking{type:enabled,clear_thinking:false}` + `reasoning_effort` | `thinking{type:disabled}` |
| `openai` + `openrouter` | `reasoning{effort: "high"}` | `reasoning{effort: "none"}` |

等级：`off / low / medium / high / max`。模型可以用 `thinkingEfforts` 把等级**改名**
（`max: ultra`，给自有词汇的网关）或直接给 Anthropic 的**预算数字**（`medium: "4096"`）。

### 5.2 四条不能动的规矩

1. **真源是模型目录**：模型没勾"支持推理"→ 适配器一个思考字段都不发（不是发个空值），
   因为往不支持推理参数的模型上塞 `reasoning_effort` 会被网关 400。
2. **声明了档位就只允许声明过的档位**：请求 `max` 而模型只列了 `low/high` → 直接
   `CONFIG_ERROR`，不静默降级成 `high`（用户以为选了 max，其实没有，比报错更糟）。
3. **Run 快照记的是"生效值"不是"请求值"**：`ai_runs.reasoning_effort` 存的是过滤后的结果，
   事后审计能看出这次到底思考了没有。
4. **读快照、不读目录**：Run 开始后用户改模型目录，不影响已经在飞的请求。

### 5.3 加新协议/新方言时怎么做

在 `ThinkingFormat` 里加一个枚举值 + 在 `OpenAiCompletionsAdapter.applyReasoning` 里加一个
`when` 分支，然后在 `ReasoningEffortTest` 里补一个"开启发什么、关闭发什么"的用例。
**别在别处再维护一份支持矩阵** —— 预检、Run 准入、UI 都读同一处声明。

### 5.4 token 统计

后端 `TokenUsage.from(usage)` 把各家方言归一化：

- 输入：`prompt_tokens` / `input_tokens`；
- 输出：`completion_tokens` / `output_tokens`；
- 缓存命中：`prompt_tokens_details.cached_tokens` / `cache_read_input_tokens`；
- 思考：`completion_tokens_details.reasoning_tokens`。

只给 `total_tokens` 的网关会退化成"全记在输入上"（`totalTokens` 仍然正确）。
没给 usage 就是 0，**不编数字**：界面上没有就是没有，不显示占位的 0。

## 6. 给下一个协作者的注意事项

- **别把密钥塞进 `app_settings` / SharedPreferences / 日志**：唯一入口是 `CredentialService`；
  Run 的 provider 快照里只有 `credentialRef` 名字。
- 新增 AI 表时，`db/schema.sql`、`db/migrate.sql`、`Migrate.kt` **三处必须同步**（启动自动补齐靠 `Migrate.kt`）。
- 适配器的 `transports` 是"附件到底能不能发"的唯一事实来源；预检与 Run 准入都读它，
  所以**实现新传输方式时先改适配器**，别在别处再维护一份支持矩阵。
- 加新协议时照 `OpenAiCompletionsAdapter` 的样子写，并在 `Adapters.all` 注册；
  没实现完的协议要**明确抛错**（参考 `OpenAiResponsesAdapter`），不要让用户以为能用。
- 模型能力的判断顺序**不要动**，而且必须**逐个维度**判断：接口声明 > 内置目录 > 兜底。
  兜底里只有"工具"默认给 true（网关普遍支持但不声明，且请求体还不发 `tools`），
  模态兜底"仅文本"、思考兜底"不支持"。
  ⚠️ 这里踩过一次：旧写法是"看到任何已知字段就整体采信接口"，于是网关只回
  `{id, object, owned_by}` 时内置目录永远轮不到，用户看到的全是"未识别 / 仅文本"、
  模态和思考档位还得手填。判"接口说了"必须**按维度**、且**真的读到值**才算说
  （`capabilities: {}` 不算）。用例：`ModelDiscoveryTest` 的"接口只给 id 时回退到内置目录"、
  "空的 capabilities 对象不再被当成接口声明"。
  往 `ModelCapabilityCatalog` 加规则等于"替用户预勾选"，宁可少勾（漏了用户能补，多勾会直接请求失败）。
- 设置页里**任何失败都要能看见**（SnackBar / 顶部横幅）：之前"新建 Provider 失败只写进详情面板、
  而详情面板要先选中 Provider"导致用户看到的是"点了没反应"，已修，别再引入同类回退。
- 默认监听已改为回环；如果要用 Android 客户端连本机后端，需要显式 `COMFYHUB_ALLOW_REMOTE=1` 并自行加认证（AIH-016、AIK-003）。
- 思考强度**不要加"模型 ID 猜档位"的回退**：目录没声明就不给选（AIH-011 同一条原则）。
  设置页模型卡片的 `_copy` 是唯一复制入口，加字段时务必带上，否则切换某个徽标会把别的声明悄悄抹掉。
- **空会话清理与输入草稿是一对**：`AiWorkspaceStore._cleanupEmptyConversation()` 只在"没有消息、
  `attachments` 为空、**且本地草稿也是空的**"时才删会话；输入框文字按会话存在本地
  （`saveDraft` / `loadDraft`），附件托盘也按会话存（`_stashAttachments` / `_applyDraftAttachments`）。
  页面侧还有一条不能破的规矩：**程序性地改输入框内容必须屏蔽草稿回调**（`_applyingDraft` +
  `_setInputText`），否则"清空输入框"会被当成用户删光了字，把刚取回来的草稿覆盖成空串
  —— 这就是用户报的"切走再切回字没了"的成因。回归：`test/ai_conversation_lifecycle_test.dart`。
- **`AiWorkspaceStore.load()` 是幂等的**：`HomeShell` 每次切页都会重建页面，重跑加载流程就会
  白送一条新会话（用户报的"打开就冒出好几条新对话"）。冷启动也不再无条件新建会话：
  已有干净空会话就复用。改这段前先看 §4.8 的 ①。
- **别把"列表接口 404"当成鉴权失败**：`AiUpstream` 里 `/models` 的 404 是
  `MODEL_LIST_UNAVAILABLE`（端点不提供列表，连接本身是好的），401/403 才是 Key 的问题，
  而且此时**不再回退第二个地址** —— 否则会把"Key 不对"掩盖成"地址不对"。
- **列表页的页码要能收敛**：批量删除后当前页可能已经不存在，后端对越界页码返回空 items。
  `LibraryStore.refreshMedia/refreshPrompts` 已经内置"items 空但 total>0 → 回最后一页重取一次"，
  新增列表/筛选逻辑时要沿用，否则会出现"删了几个却显示整个库空了"。
- **长列表一律懒构建**：模型卡片、聊天气泡这类"一屏装不下"的列表用 `ListView.builder`，
  别用 `children: [for (...) ...]`（首帧会建出全部条目）；流式刷新时给每项加 `RepaintBoundary`。
  但**懒构建 + 高矮不一的条目 = 滚动条滑块会乱跳**（`maxScrollExtent` 是估算的）：
  「AI 模型与凭据」那类列表要用 `SliverFixedExtentList(itemExtent: 常数)` + 编辑弹窗，
  别让每行自己撑高（详见 AGENTS §6 与 `test/model_list_scroll_test.dart`）。
- **Markdown 解析器是自研的**（`lib/widgets/markdown.dart`）：改行内规则时记住
  **"位置最靠前的标记先处理"**，不能"代码永远优先"；块级规则要**流式安全**
  （未闭合的围栏/标记按字面量显示，不吞内容）。表格以 GFM 的分隔行为判定，没有分隔行就是普通段落。
- **缩略图必须给 `cacheWidth`**：`Image.network` 不带解码上限时会按原图尺寸解码，
  而 `/thumb` 在"生成失败"时会**回退原图**（4096² 就是 64MB）——画廊快速滑动卡顿的主因。
  规则与回归用例见 `lib/widgets/media_thumb.dart` 与 `test/scroll_perf_test.dart`。
  ⚠️ 顺带纠正一个常见误判：`SliverChildBuilderDelegate` 默认 `addRepaintBoundaries: true`，
  **网格里的每格本来就有 RepaintBoundary**，不用手加（真正该关的是 `addAutomaticKeepAlives`）。
- **AI 工具 / Skills / 长期记忆的改动规矩见 `AGENTS.md` 第 10 节**（工具清单唯一真源、写入路径必须过
  `ToolPolicy`、审批必须先 open 再 emit、Skills 不做缓存、投放口自动登记不许改已有 frontmatter、
  记忆超限报错不许截断、改提示词必须 bump `SystemPrompt.VERSION`）。
- **改 AI 相关接口后跑一次 `scripts\e2e-ai-tools-test.ps1`**：它是唯一能覆盖"工具循环 + 审批 +
  权限拒绝 + 落库 parts + 长期记忆注入"的端到端用例（假网关，离线、不花钱，58 项检查）。
  第二轮三个真 bug、第三轮的记忆链路都是它抓出来/钉住的。
