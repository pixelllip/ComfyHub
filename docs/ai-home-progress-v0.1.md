# AI 工作台实施进度（v0.1 实施记录）

> 日期：2026-09-15 起，2026-09-16 追加（思考强度 + token 统计 / 用户清单收尾）
> 依据：`docs/ai-home-requirements-v0.1.xlsx`（需求清单 / 待确认决策 / 风险清单 / 里程碑）
> 与 `docs/ai-home-implementation-plan-v0.1.md`（实施方案）
> 已覆盖范围：**M0 安全前置 + M1（Provider / 模型 / 凭据）+ M2（Run / 统一 SSE / 三协议）
> + 思考强度与 token 统计（AIH-056 / AIH-057）+ 用户清单 `docs/bug-and-suggestion-9.16.md` 全部条目**

## 0. 一句话现状

**已经能用真实 Base URL + API Key 配对并流式对话**（OpenAI 兼容 / Anthropic / OpenAI Responses），
可以在聊天框里直接切模型**和思考强度**、看到每轮消耗的 token；
附件（图片等）目前是"正确阻断"而不是能发；ComfyUI 工具与 Skills 还没接。

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

图例：✅ 完成　◐ 部分完成　⬜ 未开始

## 3. 验证证据（实测，非推断）

- `pwsh -File scripts\server.ps1 test` → **122 项全通过**（`AiDomainTest` / `FileKindDetectorTest` /
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
3. **工具循环与 Comfy 查询（M4）**：`ai_tool_calls` 表、`comfy_get_status` / `comfy_get_run` / `comfy_sync_history`（含审批）、
   单次回复最多 3 次主动查询；系统提示词里现在**明确写了"尚未注册任何工具"**，加了工具要同步改提示词版本。
4. **Skills（M5）**：目录扫描、`load_skill`、第三方安全导入；界面上的 `/` 菜单目前只是目录展示。
5. 事件表保留策略：`AiRunRepo.pruneEvents()` 已写好但还没接到定时任务。
6. **ComfyUI 查询工具（M4）尚未注册给 AI**：AI 工作台右侧已经能看到 ComfyUI 状态，
   但工具循环（`ai_tool_calls` 表、`comfy_get_status` / `comfy_get_run` /
   `comfy_sync_history` + 审批 + 单次回复最多 3 次主动查询）还没做，
   系统提示词里也仍写着"尚未注册任何工具"。

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

> 仍然是"预填建议"，不是真相：接口声明 > 内置目录 > 仅文本，界面标来源、可手改。
> 表命中的模型如果档位不对，用户在模型卡片上改一下即可（改完来源会变成"手工声明"）。

### 4.3 用户报的「OpenAI Responses 填 API Key 报错」

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
- 模型能力的判断顺序**不要动**：接口声明 > 内置目录 > 仅文本。
  往 `ModelCapabilityCatalog` 加规则等于"替用户预勾选"，宁可少勾（漏了用户能补，多勾会直接请求失败）。
- 设置页里**任何失败都要能看见**（SnackBar / 顶部横幅）：之前"新建 Provider 失败只写进详情面板、
  而详情面板要先选中 Provider"导致用户看到的是"点了没反应"，已修，别再引入同类回退。
- 默认监听已改为回环；如果要用 Android 客户端连本机后端，需要显式 `COMFYHUB_ALLOW_REMOTE=1` 并自行加认证（AIH-016、AIK-003）。
- 思考强度**不要加"模型 ID 猜档位"的回退**：目录没声明就不给选（AIH-011 同一条原则）。
  设置页模型卡片的 `_copy` 是唯一复制入口，加字段时务必带上，否则切换某个徽标会把别的声明悄悄抹掉。
- **空会话清理与输入草稿是一对**：`AiWorkspaceStore._cleanupEmptyConversation()` 只删"没有消息
  **且** `attachments` 为空"的会话；输入框文字另外按会话存在本地（`saveDraft` / `loadDraft`），
  所以"打了一半切走"不会丢字。改这几处时三个条件要一起看，别只顾删会话。
- **别把"列表接口 404"当成鉴权失败**：`AiUpstream` 里 `/models` 的 404 是
  `MODEL_LIST_UNAVAILABLE`（端点不提供列表，连接本身是好的），401/403 才是 Key 的问题，
  而且此时**不再回退第二个地址** —— 否则会把"Key 不对"掩盖成"地址不对"。
- **列表页的页码要能收敛**：批量删除后当前页可能已经不存在，后端对越界页码返回空 items。
  `LibraryStore.refreshMedia/refreshPrompts` 已经内置"items 空但 total>0 → 回最后一页重取一次"，
  新增列表/筛选逻辑时要沿用，否则会出现"删了几个却显示整个库空了"。
- **长列表一律懒构建**：模型卡片、聊天气泡这类"一屏装不下"的列表用 `ListView.builder`，
  别用 `children: [for (...) ...]`（首帧会建出全部条目）；流式刷新时给每项加 `RepaintBoundary`。
