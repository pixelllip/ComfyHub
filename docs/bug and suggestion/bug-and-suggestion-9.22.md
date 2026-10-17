建议：

* 可以使用civitai等网站查找符合要求的工作流，优先根据本机已有的模型修改该工作流
  → **查明现状（2026-09-22，未改代码）**：这条路今天**有工具但缺一步**。
  `civitai_search`（09-20 加的）能搜模型/LoRA 并带触发词，`comfy_load_workflow` 能把本机
  工作流文件读进库，`comfy_submit` 能在提交时覆盖 `节点id.输入名` 的参数 —— 三件都齐了。
  缺的是**"按本机已有的模型改这份工作流"这一步没有专工具**：现在得让模型自己比对
  `search_models` 的结果与图里的模型名，逐个 `overrides` 改。昨天 `b5233fb4` seq6 里
  它就是靠**手读 safetensors 头部**确认"那个 13.49 GB 的 checkpoint 是裸 DiT、不含 CLIP/VAE"
  的 —— 这说明"模型名对不对"这件事现在靠模型自己推理，代价高且不稳定。
  **下一步（半天）**：加 `comfy_fit_workflow(graph)` —— 输入一份 API 图，输出
  "图里要的模型 / 本机最接近的等价物 / 需要 `overrides` 改哪几个键"，
  判据复用现成的 `ComfyModels` 与 `ComfyRoots`（**只报不改**，改由 `comfy_submit` 的 overrides 做）。
  → **已做（2026-09-22）**。落成三块：
  ① `ComfyModels.kt`（新，`com.comfyhub`）——**清单两个来源**：优先 ComfyUI 的 `/object_info`
  枚举（`CheckpointLoaderSimple.ckpt_name` 就是"它能加载的 checkpoint 列表"，**权威、零成本**），
  拿不到才按目录扫 `models\<folder>`（判据复用 `ComfyRoots`，只读元数据、有界）；
  配对判据是**分词后的 Dice 相似度**（`ModelName`），不是编辑距离 ——
  `krea2TurboOfficialComfy_krea2TurboInt8` ↔ `krea2_turbo_int8` 认得出来，
  而 `sd_xl_base_1.0` ↔ `sd_xl_refiner_1.0` 虽然字面很像，**分数高也不许当成同一份**。
  ② `WorkflowFit.kt`（新）—— 三档结论 `exact` / `substitute` / `missing`，
  报告里给的是**事实**（`overrideKey` / 候选 / 分数 / 体积 / 在哪个目录），
  唯一由代码下的结论是 `exact`（归一化后同名才叫"就是它"，也只有它进 `suggestedOverrides`）；
  `missing` 时明确写"**不要编一个文件名**去提交"，并指向 `civitai_search` + `ask_user_question`。
  ③ `comfy_fit_workflow` 工具（出厂第 23 个）+ 系统提示 **v23 第 12.1 条**（"从别处拿来的工作流
  先对模型再提交"）。回归：`ComfyModelsTest` 12 例（纯判据）+ `ToolRegistryTest` 2 例（接线；
  其中一条钉"配对跑完图**一个字节都没变**"）。`gradle test` 471 例全绿。

* 或者将本地已有工作流的简单修改对比网上的成品工作流（先分析如果用网上工作流，需要额外安装多少节点，占用多少硬盘空间），让用户选择
  → **查明现状（未改代码）**：**"体积预估"这件事完全没有**。今天只知道"缺哪个节点类"
  （`comfy_check_inputs` 报缺口、`comfy_prune_workflow` 报 `unsupportedNodes`），
  不知道"补上它要下多少 GB"。而昨天真实的门槛恰恰是体积：
  `krea2TurboOfficialComfy_krea2TurboInt8.safetensors` 一个就 **13.49 GB**（本机实测）。
  两个可用的现成数据源：① comfy-mcp 的 `search_models`（能列本机已有文件）；
  ② ComfyUI registry / Civitai 的模型版本接口（带 `sizeKB`）。
  **下一步（半天~一天）**：把"缺口"升级成"缺口 + 需要装什么 + 大概多大 + 本机已有替代品"，
  作为一次 `ask_user_question` 的选项给用户（他刚要求过"每次完成选择后重置预算"，
  这条正好是"选择"的典型用法）。
  → **已做（2026-09-22）**。体积这条链路现在是通的，三段都补齐了：
  ① **`civitai_search` 报体积**（此前完全没有）：解析线上 `modelVersions[0].files[primary]`
  的 `sizeKB` / `downloadUrl`，正文多一行「体积：13.49 GB」+「直链：…」，
  结构化 `meta.models[].sizeLabel` 也带上（界面与模型都能直接用）。
  **拿不到就一个数字都不写**（正文不写那一行、JSON 不写那个键）—— 作者没给就说没给，
  绝不用猜的数字撑场面（用户会按这个数决定要不要下 14GB）。
  ② **`comfy_fit_workflow` 的缺口带"怎么补"**：每条 `missing` 多一个 `installProposal`
  —— `lookupQuery`（把打包文件名转成能搜的词：`krea2TurboOfficialComfy_krea2TurboInt8.safetensors`
  → `krea2 turbo int8`；分词里 `krea`/`2` 是两个词是为了算相似度，搜索词则粘回 `krea2`）
  / `installInto`（该放进哪个目录）/ 以及**本机"沾边但不够像"的那几个**（`nearMisses`）。
  ③ **系统提示 v23 第 12.1 条**写明：问用户的选项里**必须带体积**、并说清装到哪个目录。
  回归：`CivitaiSearchTest` 2 例（体积进正文与结构化字段；作者没给时不编）+ `ComfyModelsTest`
  2 例（查询词转换；缺模型的 installProposal 与 nearMisses）。`gradle test` 475 例全绿。
  **另做了一次真机核验**（2026-09-22）：起后端后 `GET /api/ai/tools` → **62 个工具**，
  其中 `comfy_fit_workflow` 在列（`category=comfy` / `access=allow`），
  也就是"**23 个出厂 + 39 个来自 comfy-mcp**"这个分界仍然成立。
  （⚠️ 本机 8080 落在 Windows 保留端口段里，起后端要加 `-Port 8280` 之类；
  见 [`pitfalls.md`](../pitfalls.md) 第一行。）

* 每次完成选择后，重置工具预算
  → **已做（2026-09-22，提交 `7a9b59f`）**。三件事一起改的：
  ① 用户**真的回答**了 `ask_user_question` → 这一窗的调用/查 ComfyUI/联网额度归零
  （上限 4 次，到顶如实说明，不再白送）；提问工具自己不占额度；
  ② **失败的调用不计数**（越界被拒 / 参数错 / ComfyUI 不在线 / 用户点拒绝 —— 今天实测这几类
  以前都在扣额度，所以经常"什么都没干就说预算不足"）；
  ③ 绝对天花板 200 次（不受重置影响），默认值 8 轮 / 16 次 → **16 轮 / 48 次**，
  库里冻着的旧出厂值按值识别抬起（用户自己调过的值一律不动）。
  起因就是昨天三条对话都在第 8 轮被掐断（见建议 ⑤ 的调研）。

* 设计工具，当用户提出类似"删除无用节点"时，分析出本地用不上的节点并向用户再次确认删除，然后执行操作
  → **已做（2026-09-22，提交 `7a67ae5`）**。`comfy_prune_workflow` 新增 `analyze` 档：
  只分析、**一个字都不改**（返回里根本没有 graph 字段，钉了用例），
  按数据流方向分成"一条分支 = 一个末端 + 喂给它的所有上游"，每条给**人话标签**
  （"ControlNet（结构控制）"/"放大 / 缩放"…，认不出就照抄类名，不编），
  入口就是那个末端（用户点它整条一起摘）；**孤立节点单列并明说不建议删**
  （那是"还没接完"，判据与真摘时的级联判据逐字一致）。
  hint 明确要求先 `ask_user_question` 确认，再带 `removeFrom` 真摘 —— 也就是
  "分析 → 确认 → 执行"三步。昨天那份 rgthree 工作流里 20 个纯前端节点正是典型场景。

* 调研我昨天和ai的对话记录，分析还有哪些值得优化的
  → **已做（2026-09-22，提交 `0a7da68`）**：见 [`conversation-review-9.21.md`](../archive/conversation-review-9.21.md)。
  口径是库里 09-21 的 5 场对话、45 条消息、36 次 Run、91 次工具调用。三条真痛点：
  ① 工具预算（上面那条，已修）；② **上游连接失败 11/13、8 条是 `SSLHandshakeException`，
  而且失败时回复是空的** —— 用户看到的是"四连空气泡"，且错误信息把 cause 链丢了、
  重试策略只覆盖 CONFIG/PROTOCOL（**这条建议排在最前面修**）；
  ③ 界面格式工作流转 API 图反复失败，最后用户自己说"你自己搓一个工作流" ——
  而 AI 自建最小骨干**一次就成功**，建议把"自建"从兜底提到默认选项之一。

* 开始安卓客户端的施工
  → **今天没动工，原因是排期**：`docs/android-client-plan.md` 把落地顺序定死了 ——
  **第 1 步必须是入站鉴权**（后端持有 Provider Key、能读写文件、能提交 ComfyUI，
  一开 `COMFYHUB_ALLOW_REMOTE=1` 就是裸奔），设计已在
  [`inbound-auth-design.md`](inbound-auth-design.md)，分 P0 闸门 / P1 媒体签名 / P2 二维码配对。
  今天的时间用在了建议 ①~⑤ 上（其中 ③④ 是当天可交付、当天能验证的）。
  下一步就按那份文档的 P0 开工 —— `X-ComfyHub-Token` 闸门 + 回环豁免 + 纯函数单测 + 一条 e2e。

* 有可能在完成配对之后，使用公网远程控制电脑（电脑和手机在异地）吗？安全性如何保证？
  → **已答（2026-09-22）**：见 [`remote-access-security.md`](../remote-access-security.md)。
  三句结论：① **可能**（后端本来就是个 HTTP 服务，把"能连到它"解决掉就是全部，不用改架构）；
  ② **今天不能用** —— 今天开远程（`COMFYHUB_ALLOW_REMOTE=1`）**一个 token 都没有**，
  `Application.kt` 只打一句 warn 不拦请求，所以**必须先做 `inbound-auth-design.md` 的 P0**；
  ③ 安全靠**四层叠加、缺一层都不够**：网络层"外网扫不到端口"（组网 / Cloudflare Tunnel）、
  传输层 TLS（否则令牌在公网明文飞）、应用层配对令牌（常数时间比较 + 失败限速 + 媒体走短期签名）、
  **能力层不因"来自手机"而放宽任何权限**（审批档与写白名单照旧 —— 这条最要紧：
  异地时用户不在电脑前，审批会超时，那是特性不是 bug，要在手机上弹而不是自动放行）。
  推荐顺序：**虚拟组网优先**（Tailscale/ZeroTier，端口对外不存在）→ 要"浏览器直接开"用
  Cloudflare Tunnel + Access（不需要在路由器开任何入站端口）→ **最后**才是端口映射 + 反代 + 令牌
  （那是真的把服务放到公网上，必须只有 TLS、后端只绑回环、反代限速）。
  另附"开启远程前的 8 条自检"（含"8188 不能暴露公网""电脑别睡觉""手机端要能处理审批卡"）。

---

## 2026-09-22 深夜：复盘最新一场对话（`0bedb3f5`「启动 ComfyUI」）

**观察对象**：会话 `0bedb3f5-2510-47df-8aad-61d74b9acdec`（21:07:46 创建）。
复盘时它**仍在跑**（`run b556e8fa` = `running`、`seq11` = `streaming`），所以下面是活现场，不是事后回忆。
口径：7 次 Run（其中 2 次 `PROVIDER_UNREACHABLE` 白跑）、**67 次工具调用 / 7 次明确失败**、
30 分钟里只产出 **1 张图**（mediaId 204）。
取数：`comfy_hub.ai_messages` / `ai_message_parts` / `ai_runs` / `ai_tool_calls`。

### A. 用户这次提的问题（8 条）

* **AI 碰壁次数过多**
  → **已核实，成立**。七次明确失败里，**只有两次是"这个世界真的做不到"**，其余五次是工具/环境自己的毛病：

  | 现场（seq / 调用） | 代价 | 性质 |
  | --- | --- | --- |
  | `list_workflow_slots`（193KB 工作流）跑 61 秒 → `MCP_TOOL_ERROR` | 61 s + 一次白等 | comfy-mcp 工具糙 |
  | `search_models(folder=unet)` / `(folder=clip)` → `MCP_TOOL_ERROR` | 3 次调用 + 推理跑偏 | 目录不存在**应该回空列表**，不该报错 |
  | `comfy_load_workflow` → `UNSUPPORTED_NODES`（20 个 rgthree 前端节点） | 多一轮 + 需自己想到 `tolerateUnsupported` | 缺"哪些节点可直接摘"的知识 |
  | `comfy_submit` → `QUERY_BUDGET_EXCEEDED`（9 次上限） | **Anima 那一跑根本没提交** | 预算被上面的失败调用吃满 |
  | `list_dir D:\Comfy-Desktop` / `...\ComfyUI-Installs` → `PATH_DENIED` | — | 只放行精确目录、**不放行父目录**，恰好挡掉"往上走一层看布局" |
  | `launch_comfyui` 87 s 失败（8188 占用 / 无权限） | 87 s | 环境冲突（Desktop 实例在跑） |
  | `comfy_use_attachment` 投到 `ComfyUI-Shared\input`，实例去 `ComfyUI-Installs\...\input` 找 → 日志 `No such file or directory` | **整条 Image-to-Prompt 工作流报废** | 我们自己的 bug（见 C-1） |

  另外两次撞顶：`seq5` 撞 **9 次 ComfyUI 查询上限**（任务半途而废）、
  `seq9` 撞 **16 轮工具上限**（正文写成"（已达上限，再发一条消息它就能接着做）"）。
  白跑但报成功一次：`Krea2_spine` 78 秒 → `status: success` + 一句小字"产物与已有文件相同，未重复入库"，
  `mediaCount: 0`（用户追问才发现）。白等三次：`ask_user_question` 117 s + 95 s、`server_info` 每次 18 s。

* **没有主动总结长期记忆并记下的能力**
  → **已核实**：这条会话 `remember` 调用数 = **0**；`storage\ai\memory.md` 里 4 条全是 09-19 的 danbooru 笔记。
  但**不是它忘了**：`seq7` 的 reasoning 里它**明确想过**"Also I should update the memory about the user's
  preferences? ... I can remember it." —— 最后没调。
  真因是**触发条件写窄了**：`NsfwPrompt.kt` 第 11 条只覆盖"用户说出的**偏好/约定**"，
  还明确写着"**不要**记录本次任务的临时进度"。而这次真正值钱的是**环境事实**：
  ① 本机真实模型根是 `D:\Comfy-Desktop\ComfyUI-Shared\models`（不是 `ComfyUI-Installs\...\models`）；
  ② `krea2TurboOfficialComfy_krea2TurboInt8` 本身就是 Turbo 档（8 步 / cfg 1），不需要再装蒸馏 LoRA；
  ③ 用户要 Sage Attention、不要任何画风 LoRA。三条一条都没落盘。
  **后果**：它 21:35 才发现的事实，21:38 还在同一个坑里向用户解释。

* **AI 不查找 ComfyUI-Shared 里面放着的模型等文件**
  → **已核实，但根因不是"它没看"**。它到 `seq10` 用户点名才去看，一看全在
  （`anima-base-v1.0` 4.18 GB / `qwen_3_06b_base` 1.19 GB / `anima-turbo-lora-v0.2` 148 MB）。
  真正的机制是**两个模型根不是同一个地方**：
  ① 它自己 `launch_comfyui` 起的实例，workspace = `D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI`，
  而 `user\` 下只有 `extra_model_paths.yaml.example`、**没有生效的 `extra_model_paths.yaml`** → 该实例看不到 Shared；
  ② `ComfyModels.inventory()` **优先取 `/object_info` 并直接 return**（`ComfyModels.kt` L233-236），
  拿到的是这个瞎实例的枚举（59 条，`diffusion_models` 只有 2 个 minimax）→
  `comfy_fit_workflow` 于是报"本机没有 `anima-base-v1.0`"。
  **它不是偷懒，是工具给了一份"权威但错的"清单。**
  讽刺的是 `ComfyModels.scan()` **本来能扫到 Shared**（`scanRoots` 会收下所有含 `models/` 的根，
  Shared 也在只读白名单里）—— 只是永远走不到那一步。

* **约束 AI 提问的问题种类**
  → **待做**。现在只有"能只读查到的不要问"这一条软约束（`NsfwPrompt.kt` 第 9 条末段）。
  建议改成白名单：**只问"用户的意图 / 审美 / 取舍"**；"能只读查到的 / 有明确默认值的 /
  能从已有选项反推的"一律不许问，必须自己查或明确标注默认值往下走。

* **让 AI 支持提问多个问题**
  → **待做**。现在 `ask_user_question` 的 schema 是 `{question, options, header}` **单问题**
  （`ToolRegistry.kt` L1195），并且描述里明确写"② **一次只问一个决定**，别把三个问题塞进一句话"（L1191），
  系统提示第 9 条同样写着"一次只问**一个**决定"（`NsfwPrompt.kt` L88）。
  而这张卡是**同步阻塞**的（`AskGate.await` 挂在工具里，界面答完才继续），
  所以"多问几个"的代价是 **N 轮工具预算 + N 次等待**（这次 117 s + 95 s 就是这么来的）。
  改法：schema 扩成 `questions[]`（每项 `header` / `question` / `options`，推荐项放第一个），
  一次挂多张卡、集中一次等待；`AskGate` 的 `PendingQuestion`（单条）+ `findSameQuestion`（防重复）
  要扩成"一组 questionId"，前端也要能一次画多张。

* **提问的数量尽量少，问题覆盖尽量全面**
  → **待做**。建议把"提问预算"显式化（例如一次回复 ≤1 张卡、≤3 问），
  并要求"每个问题必须互不依赖、且都影响下一步动作"——避免把一次决策拆成三次问询。

* **让 AI 具有审核本地工作流的能力（skill）**
  → **待做**。这次会话就是最好的动机：用户直接问"你有没有发现你用的 krea2 工作流本身有问题呢？"，
  AI 承认"我上一轮确实没审这条骨架就用了"。现有 `skills\builtin` 只有 `img2img-reference` / `krea-2`，
  **没有工作流审核**。建议两条腿：
  ① **确定性检查器给事实**（前端外观节点清单：rgthree Label/Bookmark/Fast Groups Bypasser、Set/Get、
  Anything Everywhere；缺模型；`IS_CHANGED` 版本不兼容节点；**输出前缀 + 固定 seed 撞名**；
  显存估算；参数档位是否匹配该模型）；
  ② **skill 文字给判定与修法**；并且**入库时自动跑一遍**，而不是让 AI 每次现查。

* **加入 modelscope、huggingface 的查询功能，并与 civitai 合并成"定向查询" tool；查到的贴链接让用户手动下载**
  → **待做**。代码里 **0 处** `huggingface` / `modelscope`（全仓 grep 无匹配），今天只有 `civitai_search`。
  落地要注意三点：① civitai 直连不通、**必须走 `127.0.0.1:7890`**（AGENTS 第 4 节），HF 常需镜像，
  所以工具**必须报可达性**（是否需代理 / 是否 gated / 是否需登录），否则又变成"AI 说能下、用户下不动"；
  ② 直链要带 **sha256 / LFS oid** —— 用户手动下完之后我们能校验，也能判断"盘上那份是不是同一个模型"
  （现在 `ModelName.similarity` 只是**字面**相似，正是误判之源）；
  ③ 输出统一 schema + 永远附 `installInto`，顺序固定为**先查本机再查外网**。

### B. 复盘时另外发现的（用户没提到，按严重度排）

1. **冲突证据没人裁决（最要紧的一条）。** `seq9` 里 AI 自己就发现了矛盾——
   "历史 promptId 266 用 `anima-base-v1.0` 跑成功过，但现在枚举里没有"，
   它在 reasoning 里怀疑过 Shared、怀疑过 `extra_model_paths`，**但没验证就下了结论**，
  还照着错的结论写了两个工作流文件（`anima_miaomiao_sage_t2i.json` 等）。
  缺一条硬纪律：**"历史成功捕获"与"当前枚举"冲突时，必须先把目录/配置查清再说话**
  （历史成功是强证据：文件曾经存在过）。
2. **`launch_comfyui` 启的不是用户日常那套环境**，起完就应该**对账**：
  实例的 models / input / output 三个根 vs 我们配置里那套，不一致当场警告
  （而 `comfy_get_status` 报的 `outputDir` 其实是**我们 capture 配置**的值，不是实例的 —— 这点也会误导模型）。
3. **缺一等公民「本机模型清单」工具。** 现在只有 `comfy_fit_workflow`（必须给一份图）和
  comfy-mcp 的 `search_models`（只看实例）。所以它只能手搓十几条 `list_dir`（`seq9` + `seq11`）。
  应有 `comfy_list_models(folder?, query?)`，返回 **`/object_info` ∪ 盘扫的并集**，每条标 `source`。
4. **comfy-mcp 那批工具与自研工具职责重叠、且不受我们的约束。**
  `search_models` / `search_templates` / `get_template` / `fetch_template` / `nodes` / `get_logs` /
  `which` / `generate_image` 和我们的 `comfy_*` 混在一张清单里（62 个工具），
  `seq7` 一轮就横跳了 7 个来源；而它们**不走读白名单、不进审计**，报错也糙（见 A 表）。
  建议：给每个工具补"何时用 / 何时别用"的对偶说明（现在只有 `civitai_search` 与 `web_search` 有），
  并在提示里把 `search_models` 标为"低优先，改用 `comfy_list_models`"。
5. **"能不能跑"判早了。** `comfy_load_workflow` 只答"转换得出来"（`runnable: true`）。
  promptId 304 挂着本机没有的画风 LoRA（`allmm style-step00003000`）、
  还用 `UNETLoader` 读 `diffusion_models/`（文件实际在 `checkpoints/`），照样 `runnable: true`。
  这正该由审核器补上（"转换成功 ≠ 跑得起来"）。
6. **"成功但 0 产物"的提示强度不够。** 78 秒白跑藏在一句 `message` 小字里，模型没接住。
  这一档应独立语义 + 直接给动作（换 seed / 换 filename_prefix）。
7. **预算被"失败调用"吃掉。** 09-22 已经改成"真的执行了才扣"，但**"执行了却报错/超时"照样扣**
   （61 秒那次 `MCP_TOOL_ERROR` 就扣了）。`seq5` 就是这么被吃满、Anima 跑不成的。
8. **父目录读不到。** 写白名单只放行精确目录（`D:\Comfy-Desktop\ComfyUI-Shared`、
  `...\ComfyUI-Installs\ComfyUI\ComfyUI`），**不放行它们的父目录**，
  于是"往上走一层看看旁边还有什么"这个动作必然被拒 —— 而它恰恰是**自己**发现 Shared 的唯一路径。
9. **无 HF/ModelScope**（同 A 最后一条），且三源字段要能对齐才谈得上"合并"：
  名字 / 作者 / 类型 / 底模 / 触发词 / 体积 / 下载量 / 直链 / 需登录 / 需代理 / sha256。

### C. 建议的优先级（我的判断）

**P0：先把"世界状态"修对（B-1 / B-2 / B-3 / B-8）。**
理由：清单是错的，那么加 HF/ModelScope、加审核 skill 只会把错误结论做得更精致 ——
下一次它仍会拿那份 59 条的瞎清单去"审核"。具体四件事：
① `ComfyModels.inventory()` 改成 **object_info ∪ scan 的并集**，并对差集报
"**盘上有、但这次启动的实例看不到**"这句诊断（这句话能一眼定位 `extra_model_paths` 没生效）；
② `comfy_use_attachment` 投到**实例的** input（或对账后在两个 input 都放一份）；
③ 新增 `comfy_list_models`；
④ 读白名单放行"已知 ComfyUI 根的上一级"（只读、有界）。

**P1：审核 skill + "能不能跑"（用户点名的第 4 条 + B-5 / B-6）。**
**P2：提问改造（多问 / 种类白名单 / 少而全 —— A 的三条 + B-7 / B-8）。**
**P3：定向查询 tool（HF / ModelScope / civitai 合并）。**
**P4：长期记忆沉淀（A 第 2 条）。**

### D. 决策记录（2026-09-22 用户答复）

| 问题 | 用户的选择 | 落地含义 |
| --- | --- | --- |
| 环境对账以哪套为准 | **自定义**："在 ComfyUI desktop 里下载模型，它会优先下载到 shared 里；不过**也只限模型**，节点、工作流等还是在 comfyUI-desktop 里。**以防万一还是把 shared 默认挂上**" | 不是"只报告"，而是**默认挂载**；但 Shared 只是**模型根**，工作流/节点仍在安装目录（两边都要认，谁也不能替代谁） |
| 目录可见性 | A | 新专用「环境报告」工具 + 只读白名单放行"已知根的上一级" |
| 审核形态 | A | 确定性检查器（新工具）+ skill 写判定与修法（**不做**入库自动跑） |
| 提问改造 | **B** | 一次调用可多问，但**每个问题一张独立卡**（后端一组 questionId，前端保持单卡渲染） |
| 长期记忆 | A | 扩 `remember` 纪律：环境事实与用户偏好都记 |

### E. 核实"Shared 到底是怎么挂上的"（2026-09-22，实测）

**这不是 extra_model_paths.yaml，是 ComfyUI Desktop 自己的一套配置。** 证据（本机实测）：

- 安装根 `.comfy_environment` = `local-desktop2-standalone` → 确实由 **Comfy Desktop 2** 管理；
- 安装根里**只有** `extra_model_paths.yaml.example`（所以 comfy-cli 直接跑 `main.py` 时**没有任何**额外模型路径）；
- **真正的配置在 `C:\Users\<用户>\AppData\Roaming\Comfy Desktop\`**：
  - `settings.json` → `modelsDirs: ["D:\Comfy-Desktop\ComfyUI-Shared\models", "E:\ComfyUI\models"]`、
    `inputDir` = `…\ComfyUI-Shared\input`、`outputDir` = `…\ComfyUI-Shared\output`、`installDir` = `…\ComfyUI-Installs`；
  - `shared_model_paths.yaml`（Desktop 自动生成，**两个**模型根：`comfy.desktop_0` = Shared\models、
    `comfy.desktop_1` = `E:\ComfyUI\models`，各带全套子目录映射）；
  - `instance-model-paths\inst-<id>.yaml`（同一份内容，按实例生成）；
  - `installations.json` → `launchArgs: "--enable-manager"`、`launchMode: window`、端口冲突策略 `auto`。

**结论（P0 的正确做法）**：Desktop 启动实例时把这三样传给了 `main.py` ——
`--extra-model-paths-config <shared_model_paths.yaml>` + `--input-directory` + `--output-directory`。
而 `launch_comfyui`（走 comfy-cli 直接跑 `main.py`）**一样都没传**，于是：
① 看不到 Shared / `E:\ComfyUI\models`（→ AI 报"缺模型"）；
② input 是安装根自己的 input（→ `comfy_use_attachment` 投到 Shared\input，实例找不到，日志实据）；
③ **output 也是安装根自己的 output**（→ 我方的自动捕获盯的是 Shared\output，
**AI 自己起的实例跑出来的图，捕获根本看不到** —— 这条是新发现，比"缺模型"更隐蔽）。

**另一个新事实**：用户的模型根有**两个** —— `D:\Comfy-Desktop\ComfyUI-Shared\models` 与
`E:\ComfyUI\models`。任何"只认一个根"的实现都是错的。

**做法（待实施）**：新增 `ComfyEnv.kt` 读 Desktop 的 `settings.json` + `shared_model_paths.yaml`，
算出 `modelsDirs / inputDir / outputDir / extraModelPathsConfig`，
由 `McpToolBridge` 在模型调 `launch_comfyui` 时**自动补上这三个参数**（模型不用知道这件事），
并在结果文本里如实说明"已按 ComfyUI Desktop 的配置挂上 Shared"；拿不到配置就原样透传 + 警告。

### F. 实施进度（2026-09-22 深夜）

**已做（提交 `05d9771`，492 → 507 例全绿）** —— P0 的三件（也是"世界状态先修对"的核心）：

1. **`ComfyEnv.kt`（新）**：只读 `%APPDATA%\Comfy Desktop\settings.json` +
   `shared_model_paths.yaml` / `instance-model-paths\*.yaml`，算出模型根（**两个**）、
   input、output、`--extra-model-paths-config` 的值。四条纪律写在类注释里：
   只读不写、拿不到就如实警告、**不覆盖模型自己写的参数**、**目录不存在就不加那个参数**
   （给 ComfyUI 一个不存在的 input/output，启动会直接失败）。
2. **`McpToolBridge` 自动补参数**：模型调 `launch_comfyui` 时补
   `--extra-model-paths-config` + `--input-directory` + `--output-directory`，
   并把"补了什么"如实回给模型（成功与失败两条路都带）；顺手把模型写成 JSON 字符串的
   `extra_args` 归一成 schema 声明的数组（那次现场就是这么写的）。
3. **清单改成并集**（`ComfyModels.inventory`）：`/object_info` ∪ 盘扫，
   `LocalModel` 多一位 `visibleToInstance`，`Inventory` 多 `diskOnly` / `merged`；
   `Application.kt` 把 `ComfyEnv` 的模型根传给扫盘（少了它，盘上事实里就没有 Shared）。
   `WorkflowFit` 新增**第四档 `onDiskOnly`**：硬盘上有同名、但当前实例枚举不到 ——
   不进 `suggestedOverrides`、**不算 `missing`**，正文明确"**先重启 / 修挂载，不要下载**"。
   系统提示 **v24**（第 12.1 条，两份正文都改）。
   回归 15 例：`ComfyEnvTest`（10，含"模型写错形状也能补全"与"拿不到配置要如实警告"）+
   `ComfyInventoryUnionTest`（5，含"不许把它算成 missing"）。

**还没做（同一个 P0 剩下的，下一批）**：

- `comfy_list_models(folder?, query?)`：并集清单的一等公民入口
  （现在 AI 只能手搓十几条 `list_dir`，`seq9`+`seq11` 就是这么烧掉半小时的）；
- **环境报告**（`comfy_env_report`）：实例根 / 配置根 / 磁盘上的模型根 / 各自子目录，
  一眼看出"三个根差在哪"；配套把**已知 ComfyUI 根的上一级**加入只读白名单
  （现在 `list_dir D:\Comfy-Desktop` 被 `PATH_DENIED`，"往上走一层"这条唯一路径是堵死的）；
- `comfy_use_attachment` 按**实例的** input 投放（现在按"配置输出目录的兄弟目录"，
  所以投到 Shared\input 而实例去自己的 input 找）；
- **`launch_comfyui` 起完对账**：实例的 models/input/output 三个根 vs 配置那套，
  不一致当场警告（现在 `comfy_get_status` 报的 `outputDir` 其实是**我们 capture 配置**的值，
  不是实例的 —— 这点也在误导模型）。

**已做（第二批，提交 `1c38f40`，507 → 515 例全绿）** —— P0 收尾，三件都按用户拍板的档位：

1. **`comfy_env_report`（新工具，把"环境对账"与"模型清单"合并成一条）**：
   一次回答"**实例能加载多少个**（`/object_info` 的枚举）/ **盘上有多少个** /
   **哪些是盘上有而实例看不到的**（大于 0 就是实例没挂全模型根）/ 自动捕获盯着的
   output 与 input / Desktop 的模型根与映射文件"，并支持 `folder` / `query` 过滤列出清单 ——
   以前 AI 只能靠十几条 `list_dir` 手搓。分类放在 `FILES`（本机只读巡检），
   **不吃那 9 次"查询 ComfyUI"的轮询预算**（那次现场恰恰是预算被吃满、任务断在半路）。
2. **提交前模型预检（`ModelPreflight`）**：`comfy_submit` 在 `overrides` / `connections`
   **都应用完之后**对最终那张图做一次预检，要的模型加载不了就**当场拒**并给动作
   （`onDiskOnly` → 重启 / 修挂载，**别下载**；`missing` → 查 Civitai + 问用户，
   或者把不在执行链上的旁支摘掉）。复用 `WorkflowFit`，**不另写一份判据**。
   **清单不可信时（`Source.NONE`）一个都不拦** —— 误拦一个能跑的工作流比白跑一次更糟。
   与 `comfy_fit_workflow` 用**同一份** `modelInventory`（两处判据分家就会出现自相矛盾）。
3. **只读白名单放行"已知 ComfyUI 根的上一级"**（只放一级、**绝不放盘根**）：
   `list_dir D:\Comfy-Desktop` 不再被拒 —— 那句 `PATH_DENIED` 当初正是
   "AI 自己发现 ComfyUI-Shared"这条路上唯一被堵死的地方。
   回归：`ModelPreflightTest`(6) + `ComfyRootsTest`(2，含"盘根不许进白名单")。

**顺带定下的两件（未单独提问，按最保守的做）**：
`comfy_use_attachment` 仍先按配置那套推 input（等 `comfy_env_report` 的实例根接进投放路径再改，
改错会让图生图整条链断掉）；`launch_comfyui` 的起完对账先由**返回值里的那句**
"已按 Desktop 配置挂上哪些根"承担（`McpToolBridge` 已在做），独立告警卡留到 P1。

### G. P1 已做：工作流审核器（2026-09-22，提交 `957f288`，515 → 528 例全绿）

用户第 4 条「让 AI 具有审核本地工作流的能力」，按"确定性检查器 + skill 文字"落地
（**不做**入库自动跑）：

- **`WorkflowAudit`（新）**：七项固定检查，只报事实、可复现、可单测。分档严格：
  `BLOCK`（必跑不了）/ `WARN`（跑得起来但大概率白跑）/ `INFO`（背景）。
  ①纯前端节点（rgthree Label/Bookmark/Fast Groups Bypasser、Set/Get、Anything Everywhere、
  显示节点）②与当前节点包**版本不兼容**的节点（`WidgetToString` / `SystemNotification` 的
  IS_CHANGED 崩）③**输出前缀 + 固定 seed**（每次跑落同一个文件名 → 被去重吞掉，mediaCount 0）
  ④参数档位与该模型官方档位不符（Krea2 Turbo 8 步/cfg 1、Anima er_sde）⑤显存风险
  （≥4MP / 放大节点 / ≥3 LoRA，本机 8GB）⑥提示词风格与模型不匹配（给 Krea 2 堆 tag、
  给 Anima 写整段自然语言）⑦加载器与目录不匹配（`UNETLoader` 读 `checkpoints/` 里的文件）。
  两条纪律：**拿不到 `/object_info` 就不判"本机没有这个节点类"**；**不认识的模型不判参数档位**。
- **`comfy_audit_workflow` 工具**（出厂第 26 个）+ **skill `comfy-workflow-audit`**
  （七项的判定与修法、汇报纪律：结论先行 / 只报不改 / 别把"不推荐"说成"跑不了"）。
- **提交闸门**：`comfy_submit` 的 preflight 先跑审核、再跑模型预检；
  审核**只拦 `BLOCK` 那三项**，参数·显存·提示词风格只提醒不拦（用户拍板）。
- 系统提示 **v26**。回归 13 例（含"只提醒不拦"与"拿不到 object_info 不许下结论"两条）。

### H. 第二轮决策记录（2026-09-22，用户答复）

| 问题 | 用户的选择 | 落地含义 |
| --- | --- | --- |
| 审核检查项 | **七项全要**（含"提示词与模型风格匹配""加载器与目录不匹配"） | 见 G 节 |
| 审核是否接提交链路 | A | 接，但**只拦"必跑不了"的三项** |
| 提问权重 | **自定义**：阈值制；**意图/审美/取舍优先问**，"sage attention / 加速 LoRA"这类技术方案**放后面但也可以问**，且这类偏好要**记进长期记忆**；**允许提问多选题** | P2 的判据与选项形态 |
| 多问题的界面形态 | （给了 DSH 的截图）**一张卡装多个问题、分页、可逐题"跳过"、最后一页按钮变"提交"、答案可为空** | P2 的界面与"等全部答完（或跳过）才回模型"，**不是**"每问一张独立卡" |
| 一张卡几问 | **最多 5 问** | 超过就分两张卡 |
| 阈值制的形式 | A：**自评表 + 硬预算**；技术方案类允许问但排在意向类之后；偏好由模型调 `remember` 落 | 见下面的 P2 规格 |
| 定向查询 | A：**一条 `find_model`**，内部按 Civitai → HF → ModelScope 顺序查，**逐个报可达性** | P3 |
| 能不能代下 | A：**绝不代下**（只给链接 + sha256 + 目标目录） | P3；`download_model` 保持 deny |

### I. P2 / P3 规格（下一轮直接照着做，不再提问）

**P2 提问改造（跨栈，必须一次做完 —— 半挂会真的坏掉）**

> ⚠ 为什么不可拆：schema 一旦支持多问，模型就会问多题；旧界面只认 `GET /api/ai/runs/{id}/question`
> 返回的**第一题**，其余几题看不见 → Run 挂到 30 分钟超时。所以后端与提问卡必须同一次改。

*后端*
1. `ask_user_question` 参数加 `questions[]`（每项 `{header, question, options[], multiSelect?}`，**上限 5**）；
   旧的 `question` / `options` 保留 = 单问（向后兼容，不破坏已有会话与用例）。
2. `PendingQuestion` 加 `groupId` / `index` / `total` / `multiSelect` / `allowSkip`；
   一次调用生成 N 条同 `groupId` 的记录。
3. `AskGate.awaitAll(ids)`：一次性等这一组（`CompletableFuture.allOf` + 超时），
   返回 `Map<questionId, AskedAnswer?>` —— **跳过的就是 null**，不当成"没问过"。
4. `AskRegistry.pendingFor(runId)` 旁边加 `pendingGroupFor(runId)`；
   `GET /api/ai/runs/{id}/question` 增返 `questions[]`（旧字段 `question` 仍返回第一条）。
5. 工具结果：逐题列"问题 → 答案/（跳过）"，并写清纪律 ——
   **跳过的按"用户没回答"处理**：基于已有信息做、说明缺什么，**不许**假装他答了、也不许原样再问一遍。
6. 硬预算：**每轮最多 1 张卡**（`askInWindow` 计数，与 `callsInWindow` 同源）。
7. 提问判据（**阈值制**，写进工具描述 + 系统提示）：
   问之前自评三项 —— ① 不问会不会做错？② 错了代价大不大（花钱 / 装模型 / 覆盖文件 / 白烧一轮显卡）？
   ③ 我自己能不能查到（只读工具查得到的一律不许问）？**三项都过才许问**。
   排序：**意图 / 审美 / 取舍优先**（用哪张参考图、哪种画风路线、成品给谁看）；
   "要不要上 Sage Attention / 加速 LoRA / 降一档显存"这类**技术方案可以问，但排在后面**；
   用户答完这类偏好后**用 `remember` 落一条长期记忆**（提示里点名这几类）。
8. 系统提示 bump（v27）。

*前端*（`lib/`，按 AGENTS：改完必须 `flutter analyze` + `flutter test`，
并且**重新构建 debug 版并截图确认**，只看测试不算数）
- 提问卡改成**分页多题**：`第 N/M 题` + `上一题` / `下一题` / `跳过本题`，**最后一页按钮变「提交」**；
- `multiSelect = true` 的题用**复选框**（选项上限 5），每题都保留「自己输入」；
- 答案可为空；提交后卡片收起、结果按题列出；
- 回归用例放 `test/`（提问卡分页 / 多选 / 跳过 / 空答案各一例），必要时扩 `e2e-ai-tools-test.ps1`。

### J. P2 后端已做（2026-09-22，提交 `8c32d40`，528 → 534 例全绿）

用户第 1~3 条（约束提问种类 / 支持一次问多个 / 少问问全），形态照他给的 DSH 截图。
**后端这一半已完成**；前端提问卡（分页 / 多选 / 提交）紧随其后。

- `PendingQuestion` 加 `groupId` / `index` / `total` / `multiSelect` / `optionIndexes`；
  `AskGate.awaitAll(ids)` 等**整张卡**，跳过的按"用户没回答"（null）处理。
  两处关键设计写在 `AskGate` 的注释里：
  ① **先等任意一条**（新界面一次提交整张卡；旧界面只认第一条）—— 不能因为界面没跟上
  就把 Run 挂到 30 分钟超时；② 拿到任意一条之后**先把已完成的收干净**，再给剩下的
  **20 秒宽限窗**（顺序反了会把已经答了的饿成 null —— 实测抓到，已加用例钉住）。
- `ask_user_question`：`questions[]`（≤5，每题可 `multiSelect`），旧的 `question` 保留为单问；
  硬预算 **每轮最多 1 张卡**（`ToolContext.askCardsInWindow`，用户答完归零）；
  逐题回报 + `askedAll`（`asked` 仍是第一条，历史卡片回放向后兼容）。
- 系统提示 **v27**：第 9 条改成阈值制（自评三项 + 少问问全 + 意图/审美/取舍优先、
  技术方案排后面且答完 `remember` 落一条 + 多选题）；第 13 条补"抽卡换 seed、别换前缀"。
- **旧界面的降级路径是安全的**：它只渲染其中一题，用户答完那题 → `awaitAll` 的宽限窗
  20 秒后收口 → Run 继续（把其余几点如实记成"用户没回答"），**不会挂死**。

### K. P3 定向查询（`find_model`，一条工具）
1. 内部按 **Civitai → HuggingFace → ModelScope** 顺序查，**每个源都报可达性**：
   `ok` / `needs_proxy`（civitai 直连不通、必须走 `127.0.0.1:7890`；HF 常需镜像）/
   `needs_login` / `gated`（HF 需同意条款）/ `error`。
2. 统一字段：名字 / 类型 / 底模 / 触发词 / 体积 / 直链 / **sha256**（civitai 用版本 hash、HF 用 LFS oid）/
   **该放哪个目录**（`installInto`）/ 来源。
3. **绝不代下**（用户拍板）：只贴链接 + sha256 + 目标目录；`download_model` 保持 deny。
4. 顺序纪律：**先查本机**（`comfy_env_report` / `comfy_fit_workflow`）**再查外网**；
   本机已有同名的就不要再搜了。
5. `sha256` 的第二用途：用户手下完之后，可以用来判断"盘上那份是不是同一个模型"
   —— 现在 `ModelName.similarity` 只是**字面**相似，正是误判之源。

### L. P3 已做（2026-09-23，提交 `ebf931f`，528 → 556 例全绿）

`find_model` 一条工具，K 节五条全部落地（`FindModel.kt` + 22 例离线回归）：

- **"绝不代下"是形状上的事实，不是自觉**：窄接口 `ModelFinder` **只有 `search` 一个方法**，
  执行体里一条 `Files.*` 都没有；`download_model` 保持 deny。
- **先本机后外网**：与 `comfy_env_report` / `comfy_fit_workflow` 同一份清单
  （`/object_info` ∪ 盘扫）。本机命中 → **一个外部请求都不发**，三源标 `skipped`、`online=false`。
  判据是"分词后同名"或"文件名完全相同"，**刻意不用纯子串** —— `anima` 命中 `anima-turbo`
  的代价是用户手里少一个真需要的模型（有用例钉这条边界）。
- **三个源一条不少**：失败也占一行并带原因，状态 `ok` / `needs_proxy` / `needs_login`(401) /
  `gated`(403) / `error`(429/404/5xx) / `skipped`；单源不可达不影响其它源。
- `installInto` 按实测改成三段推断（文件名 → 仓库名 / 路径 → pipeline_tag）；
  diffusers 整包（`krea/Krea-2-Turbo` 24.48GB）→ `installInto=null` + 明说"里面没有能直接放进
  ComfyUI 的整包 safetensors"。
- `find_model` **没有** `allowNsfw`：NSFW 只留 `civitai_search`（系统提示 v28 里写明这条分工）。
- 系统提示 **v28**（第 12.1 / 17 条，两份副本同步）；`Application.kt` 让两条工具**共用同一个**
  Civitai 客户端。

**出网实测（唯一一次，探针已删）抓到三个真 bug**：① HF 返回的是 **JSON 数组**（按对象解 →
每次都被误判成"HF 挂了"）；② 魔搭检索是 **`PUT /api/v1/dolphin/models`**（GET/POST 一律 404）；
③ 魔搭仓库全名 = `Path` + `/` + `Name`（只拿 `Path` → 每个候选都悄悄丢文件名 / 直链 / sha256）。

**本机环境事实**：HF 与魔搭可达；**Civitai 直连 ConnectException → 如实报 `needs_proxy`**。
后端 JVM 必须带 `-Djava.net.useSystemProxies=true`（或 `COMFYHUB_HTTPS_PROXY`），
否则那一源永远是 `needs_proxy` —— 环境问题，不是代码问题。

### M. P2 前端已做（2026-09-23，提交 `349e3f9`，前端 248 例全绿）

提问卡改成**一卡多题分页**（I 节前端那四条）：`第 N/M 题` + `上一题 / 跳过本题 / 下一题`，
**最后一页按钮变「提交」**；`multiSelect` 画复选框；每题保留"自己输入"；**答案可为空** ——
空着的那题按"跳过"发（= 告诉模型"用户没回答"，不是"没问过"）。单题卡保持点选项即答的老手感。

- `AiToolCallState.questions[]` + `mergeQuestions`：`user.question` 是**一题一条事件**、同 `callId`，
  只留最后一条会把前面的题顶掉（那张卡只剩一题，而模型在等整组 → Run 挂到 30 分钟超时）。
- 整卡的 `status` 只在**最后一题**落定时才转 `ok`：提前转会让人还没答完，输入区就交还给普通输入框。
- 顺带修一处会复发的旧根因：判"还在等"时**空 `questions` 也算在等**
  （`tool.requested` 先到、问题后到那段窗口），否则提问卡会又挂到 `questionId` 上另开一条。
- 多选答案**拼成「A、B」当文本发**：HTTP 那条回答接口只收一个 `optionIndex`
  （`optionIndexes` 只在 `AskGate` 里），不值得为它改已经跑通的后端合同。
- **还没做**：debug 版重建 + 截图确认（AGENTS 第 1 节对界面改动的要求），
  以及扩 `e2e-ai-tools-test.ps1` 的多问那一幕。
