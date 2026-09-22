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
  → **已做（2026-09-22，提交 `0a7da68`）**：见 [`conversation-review-9.21.md`](conversation-review-9.21.md)。
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
