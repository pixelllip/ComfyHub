# AI 首页需求独立验收审计（以代码为准）

> 审计对象：`docs/ai-home-requirements-v0.1.xlsx` 的 `需求清单` 表（`状态` 列全部写「通过」，已知不可信）
> 审计方式：**只读代码**（`lib/` Flutter 前端、`server/src/main/kotlin` Ktor 后端、`scripts/`、`test/`、`db/`、`skills/`、`packaging/`），
> 逐条核对 `验收标准` 列，每条给 `file:line` 证据。
> **后续补充**：2026-09-18 当天已按本审计把 xlsx 的 `状态` 列改成现状（并补登记了原表漏掉的 AIH-056/057，
> 表内新增「验收结论」页），也补跑了全量回归：`flutter analyze` 无问题、`flutter test` **178 例全过**、
> `gradle test` **325 例全过**、`e2e-ai-tools-test.ps1` **67 项**、`e2e-submit-test.ps1` **4 幕**、
> `e2e-capture-test.ps1` 全过 → 下面 AIH-054 的判定由 `?` 升为 **✅**。
> 发布日期：2026-09-18
>
> **行数更正**：任务描述说 `需求清单` 有 59 行（AIH-001..AIH-057 + AIK-001..003）。
> 实际用 openpyxl 数出来是 **58 行**：`AIH-001..AIH-055` + `AIK-001..AIK-003`（见 `.run/ids.txt`）。
> **`AIH-056` / `AIH-057`（思考强度、token 统计）根本不在需求清单里** —— 它们只出现在
> `docs/ai-home-progress-v0.1.md:86-87` 与代码里（`HarnessRunner.kt:982`、`AiDomain.kt:92-151`）。
> 下表覆盖清单里实际存在的全部 58 条，顺序与 xlsx 一致。

---

## 1. 结论

58 条需求里 **✅ 已实现 31 条、◐ 部分实现 19 条、⬜ 未实现 8 条**（AIH-054 原先因"未跑测试"判 `?`，2026-09-18 补跑全量回归后升为 ✅）。
也就是说：清单里 27 条（46%）与验收标准并不完全对得上，`状态` 列全「通过」确实是假的 —— 但**不是"大部分没做"**，
后端的 Provider / 凭据 / 三协议 / SSE / 附件准入 / Comfy 工具 / Skills 扫描这一大片是真做了且做得比较硬（安全不变式基本都守住了）。
**最显著的 5 个缺口**：
① **AIH-043 / AIH-044（第三方 Skill 导入 + ZIP 穿越/bomb 防护）整条不存在** —— 全仓没有任何 ZIP 解包代码，
   装 Skill 只有"手动拷进 `<storage>\ai\skills` 投放口"这一条路，需求要的"导入前预览并确认 / 原子安装 / 可启停"一样都没有；
② **AIH-041 / AIH-042（内置 Anima / H3 Skill）没进内置目录** —— `skills/builtin` 里只有 `img2img-reference` 和 `krea-2`，
   `anima-*` / `h3-*` 全都在 `storage/ai/skills`（被 `.gitignore:64` 的 `/storage/` 忽略、`git ls-files` 为空）里，是**本机运行期数据不是随包内容**；
③ **AIH-047（用户自定义指令追加段）零实现** —— 后端没有追加段、前端没有编辑界面、更没有版本/digest，
   最接近的长期记忆是另一套东西（`MemoryStore` 注入时明写"是数据不是指令"）；
④ **AIH-002 在 900~1199px 区间 Comfy 状态整块不可达** —— 第三栏门槛写的是 `>=1200`（`ai_home_page.dart:251`），
   而窄屏唯一的 FAB 入口只在 `<900` 分支（`:270-274`），中间这段既没三栏也没入口；
⑤ **AIH-009「请求可取消」完全没做**（超时做了）、**AIK-001 端口仍固定 8080**（只是"可配"不是"每次启动动态分配"）、
   **AIH-031 像素预算与"显式确认后转换"没做**、**AIH-049 工具卡"敏感原文不展示"没做**（只截断不脱敏）。

另需注意：**安全不变式这一类基本都守住了**（AIH-012/013/015/017/027/030/045/051 全部 ✅），
这一片没有"文档说做了其实没做"的情况。

---

## 2. 逐条判定

| ID | 需求（短） | 判定 | 证据（file:line） | 缺什么 / 备注 |
| --- | --- | --- | --- | --- |
| AIH-001 | 新增 AI 工作台并设为默认首页 | ✅ 已实现 | `lib/app.dart:156,301,303-320,344`; `test/home_nav_test.dart:77-108` | 后端未就绪时先显示 `_StartupView`（`lib/app.dart:157`），就绪后才落地 AI 工作台 |
| AIH-002 | 工作台宽屏三栏 / 窄屏抽屉 | ◐ 部分实现 | `lib/pages/ai_home_page.dart:190`(≥900)、`:251`(≥1200 才有第三栏)、`:255-260`、`:268`、`:270-274`、`:278-287` | 缺 ①「≥900px 三栏」：900~1199px 只有两栏，且该分支没有 FAB → **Comfy 状态整块不可达**；缺 ②「Comfy状态<600px 转 Bar」：全 lib 无 600 断点，实际是底部 Sheet。Composer 常驻 ✅ |
| AIH-003 | openai-completions 协议 | ✅ 已实现 | `server/src/main/kotlin/com/comfyhub/ai/protocol/Adapters.kt:25,30`; `ProtocolAdapter.kt:19-61,107-137`; `server/src/test/kotlin/com/comfyhub/ai/protocol/ProtocolAdapterTest.kt:79-132`; `ToolProtocolTest.kt:40-91` | — |
| AIH-004 | openai-responses 协议 | ✅ 已实现 | `Adapters.kt:393,398,430,472`; `ProtocolAdapterTest.kt:183-216`; `ToolProtocolTest.kt:171-230` | 无真实 API Key 实测（进度文档 §4 自述） |
| AIH-005 | anthropic-messages 协议 | ✅ 已实现 | `Adapters.kt:197,202,250-266`; `ProtocolAdapterTest.kt:138-179`; `ToolProtocolTest.kt:91-170` | — |
| AIH-006 | 自定义 Provider ID/名/URL/协议 | ✅ 已实现 | `ai/AiRoutes.kt:82-104,85-87,114-118`; `ai/AiDomain.kt:287-306`; `lib/pages/ai_provider_settings_page.dart:1158-1191` | ID 唯一且创建后不可改 ✅、URL 校验 ✅；但**无 Provider 编辑入口**（`updateProvider` 只有定义 `lib/core/ai_api_client.dart:66`，无调用者）→ 创建后改不了显示名/Base URL |
| AIH-007 | Provider revision 乐观锁 | ◐ 部分实现 | `ai/AiRepo.kt:55-77,69-70`; `ai/AiRoutes.kt:122-123,133` | 「不能覆盖新配置」✅（`WHERE id=? AND revision=?` + `revision+1`）；缺 ① 冲突不是"冲突"信号：`Application.kt:275-277` 把 `AiException` 一律映射 400，码是 `CONFIG_ERROR`（`AiDomain.kt:75-86` 无 CONFLICT），非 409；缺 ② 前端无 Provider 编辑入口 → 验收路径在 App 内不可达；缺 ③ 无测试 |
| AIH-008 | 测试 Provider 连接 | ✅ 已实现 | `ai/AiRoutes.kt:147-161,1033-1040`; `ai/AiUpstream.kt:37-46,131,207-216,230-239,165-168` | `AiUpstream.kt:197` 的 `safeEndpoint` 是死代码（日志直接打 url，但 `normalizeBaseUrl` 已禁 userInfo/query，实际不带密钥） |
| AIH-009 | 从兼容端点发现模型 | ◐ 部分实现 | 不落库 `ai/AiRoutes.kt:167-193`；保存 `ai_provider_settings_page.dart:846-890,932-958`；**超时** `ai/AiUpstream.kt:551,107-108` | **「请求可取消」完全没做**：后端是阻塞式 `client.send`（`AiUpstream.kt:121`），无 `isActive`/`ensureActive`；前端无 CancelToken 也无超时（`lib/core/ai_api_client.dart:76-77`） |
| AIH-010 | 手工添加 / 编辑模型 | ◐ 部分实现 | `ai_provider_settings_page.dart:1416-1506`（手工加）、`:1795-1807`（模态/工具可编辑） | 缺 ① 模型 id 创建后不可编辑（`:1786` 只读 `SelectableText`）；缺 ② 显示名无编辑框；缺 ③ **上下文 / 输出上限完全没有编辑入口**（只有 `:871-872` 预填、`:951-952` 透传） |
| AIH-011 | 不按 model id 猜能力 | ◐ 部分实现 | `ai/ModelCapabilityCatalog.kt:275-282`; `ai/AiUpstream.kt:363-367,401-403`; `ai_home_page.dart:2425` | 缺 ①「工具」维度默认 **true**（`ModelCapabilityCatalog.kt:282`、`AiUpstream.kt:367`），与"未知默认不支持"相反（进度文档 §4.5 自述为用户要求，注释还写着"请求体根本不发 tools"已过期）；缺 ② UI **不显示"验证时间"**：`capabilityVerifiedAt` 后端有（`AiDomain.kt:226`、`db/schema.sql:242`）但 Flutter 无此字段，且 `AiRepo.replaceModels` 是 DELETE+INSERT（`AiRepo.kt:100-129`）会把它清成 NULL |
| AIH-012 | API Key 只写存储 | ✅ 已实现 | `ai/CredentialService.kt:47-55,57-69,79-88`; `ai/AiRoutes.kt:199-232,1095-1096`; `ai_provider_settings_page.dart:455-456,587` | 输入框永远空串、DTO 只有 `{configured,source,writable}`；无导出功能（grep `export/backup` 零命中），故"导出不返回密钥"空成立 |
| AIH-013 | set/describe/unset/resolve 语义 | ✅ 已实现 | `CredentialService.kt:47,57,71,79-88`; `AiRoutes.kt:199-232`; `resolve` 仅 `AiRoutes.kt:150,169` + `ai/HarnessRunner.kt:273` | `resolve` 没有对外路由，前端只能 set/describe/unset |
| AIH-014 | 凭据修改下次请求生效 | ✅ 已实现 | `ai/HarnessRunner.kt:273`（Run 开始时 resolve 一次并复用 `:325`）; `AiRoutes.kt:150,169` | 无需重启；在飞 Run 用开始时的那个值 |
| AIH-015 | Windows 受保护凭据（DPAPI） | ✅ 已实现 | `CredentialService.kt:57-69`（加密失败直接抛错，**绝不退化成明文**）、`:162-215`（DPAPI CurrentUser）、`:143-151` | 通过 `powershell.exe -EncodedCommand` 调 DPAPI（`CredentialService.kt:189-206`），载荷走环境变量无拼串注入 |
| AIH-016 | 收紧监听与 CORS | ◐ 部分实现 | `Config.kt:29-31,55-57`（默认回环、显式 `COMFYHUB_ALLOW_REMOTE`）; `Application.kt:240-270`（CORS 由 anyHost 改本机 + 白名单） | 缺「远程访问需显式启用**和认证**」中的认证：`Application.kt` 只 install 了 DefaultHeaders / CallLogging / ContentNegotiation / Compression / PartialContent / CORS / StatusPages（`:218-298`），**没有任何 Authentication / 会话令牌拦截** |
| AIH-017 | Base URL SSRF 保护 | ✅ 已实现 | `ai/AiDomain.kt:367-434`（信任级别匹配、云元数据硬拒、DNS 解析后复核）; `AiUpstream.kt:31-34` 与 `HarnessRunner.kt:95` `followRedirects(NEVER)`; `HarnessRunner.kt:827` | 端点信任等级可在 UI 选（`AiDomain.kt:169`、`AiProviderDto.endpointTrust`），覆盖"localhost 和私网显式确认" |
| AIH-018 | 会话创建/重命名/归档/删除 | ◐ 部分实现 | `AiRoutes.kt:262-286`; `ai/AiConversationRepo.kt:121-155,47,103,138`; 级联 `Migrate.kt:170-171,189-190,224-225,240-241` | 缺「**无引用附件可回收**」：全仓无孤儿附件回收（grep `orphan/回收/gc` 零命中）；删会话不碰 `ai_attachments`；且 `DELETE /api/ai/attachments/{id}`（`AiRoutes.kt:781-786` → `AiAttachments.kt:272-277`）**不检查引用**就直接删行+删文件，与 `lib/core/ai_api_client.dart:239` 的注释「已被引用的会报错」相反 |
| AIH-019 | 保存有序消息块 | ✅ 已实现 | `AiConversationRepo.kt:51-58,219-229,244-257,168-193`; `HarnessRunner.kt:288-300,693-718` | text/reasoning/attachment/tool_call/tool_result 五类，按 ordinal 无损恢复 |
| AIH-020 | 独立 Run 并返回 runId | ✅ 已实现 | `AiRoutes.kt:426-429`(202)、`:432-436`、`:477-486`、`:398`(retryOfRunId); `ai/AiRunRepo.kt:88-95` | Flutter 侧从不调用 `GET /api/ai/runs/{id}`（`ai_api_client.dart` 无此方法），全靠 SSE + 重载 |
| AIH-021 | 统一 SSE（单调 seq / 续传） | ◐ 部分实现 | 服务端 `ai/RunEventBus.kt:15-21,41-49,67-78`; `AiRoutes.kt:442-475`; DB 回放 `AiRunRepo.kt:209-222` | 缺「**客户端断线可恢复**」：`lib/core/ai_api_client.dart:355-381` 只认 `event:`/`data:`、**丢弃 `id:` 行**，唯一调用点 `lib/state/ai_workspace_store.dart:1070` 固定 `after=0`，断流只报错+重载（`:1190-1199`）→ `after=seq` 只有接口能力，端到端恢复不成立。"Flutter 不解析供应商 SSE" ✅ |
| AIH-022 | 取消上游模型请求 | ✅ 已实现 | `AiRoutes.kt:477-486`; `HarnessRunner.kt:108-112,822,874-891,583-589`; 前端 `ai_workspace_store.dart:1202-1213`、`ai_home_page.dart:1550-1555` | 取消即中断阻塞读、关连接、记 `cancelled`+`ABORTED`、不再进工具循环 |
| AIH-023 | 按 Run 固定 Provider/Model/Skill 快照 | ◐ 部分实现 | 写入 `AiRoutes.kt:375-422`、落库 `AiRunRepo.kt:86-103`、不含密钥 ✅ `AiRoutes.kt:386` | 缺「读快照」：执行期全读实时库（`HarnessRunner.kt:206` provider、`:212` model、`:215` ToolPolicy、`:222` skills）；`AiRunDto` 无快照字段、`GET /runs/{id}` 不回传（`AiRunRepo.kt:284-300`）→"改设置不影响在飞 Run"只靠"执行开始时读一次"的极窄窗口侥幸成立 |
| AIH-024 | 稳定错误码 + 有限重试 | ◐ 部分实现 | 错误码 ✅ `ai/AiDomain.kt:76-83`；产生路径 `AiUpstream.kt:38-46,248-263`、`AiRoutes.kt:320,353-358`、`HarnessRunner.kt:757-767` | 缺 ① **没有任何自动重试/退避**（grep `delay/backoff/attempts` 零命中），"有限重试"= 人工点重试新建 Run（`ai_home_page.dart:768-775`）；缺 ② 该重试有缺陷：`AiRoutes.kt:361-372` 不看 `retryOfRunId` 照旧 append 新用户消息 → 重载后会话里出现**重复提问 + 旧失败气泡**；缺 ③「有可见输出不重放」无显式护栏（`HarnessRunner.kt:381-398` 只看错误码）。码字面是 `QUOTA_EXCEEDED` 而非需求写的 `QUOTA`（语义一致） |
| AIH-025 | 从本机选择附件 | ✅ 已实现 | `ai_home_page.dart:1571-1589`(FilePicker)、`:2072-2083`(名称+大小)、`:2090-2106`(类型=缩略图/预览帧/图标)、`:2049-2054`+`:2008-2011`(准入红框+tooltip)；上传即按签名判定 `AiRoutes.kt:688-746` | 名称/大小/类型/准入状态四样都有（类型以预览形态表达，非文字） |
| AIH-026 | 从画廊选择已有媒体 | ⬜ 未实现 | 全仓无入口：`lib` 内无"发送到 AI / 从画廊选媒体"（grep 零命中）；`AiRoutes.kt` 无 `attachments/from-media`；`gallery_page.dart` / `media_detail_page.dart` 无 AI 入口 | **整条未做**（进度文档 §4 第 1 条亦自述"从画廊选已有媒体…那半边"没做）。也不满足"不重复复制可复用媒体" |
| AIH-027 | 严格文件类型识别（magic bytes） | ✅ 已实现 | `ai/FileKindDetector.kt:43-67,30,79-113,154-161`; `ai/AiAttachments.kt:188-195`; `ai/AiDomain.kt:492-499` | 明确不复用 `MediaFiles.kindOf`（`FileKindDetector.kt:6-8`），认不出即 `UNKNOWN` 一律阻断；e2e 有"文本改名成 .png 被拒"断言（`scripts/e2e-ai-tools-test.ps1:631`） |
| AIH-028 | 能力矩阵 text/image/video/audio/document | ✅ 已实现 | `AiDomain.kt:53-72,520-569`; `Adapters.kt:27-29,199-201,395-397`; 事实来源 `AiRoutes.kt:1131-1137` | 模型模态 ∩ 适配器传输 ∩ MIME ∩ 大小/数量全检、未知即拒 ✅。注：**video/audio/document 没有任何适配器实现** → 永久阻断（与"未知默认拒绝"一致）；内置目录只预填 text / text+image（`ModelCapabilityCatalog.kt:29`） |
| AIH-029 | 前端选择/发送前提示不支持原因 | ✅ 已实现 | `ai_home_page.dart:1410,1390-1395,1550-1552`; `ai_workspace_store.dart:230-244,961-972,680-687,906-926` | 逐张 blocker + 汇总横幅 + 发送按钮置灰；`selectModel()` 切换模型即时重算 preflight ✅。（`lib/widgets/upload_sheet.dart` 是画廊上传，与 AI 附件无关） |
| AIH-030 | 后端准入阻断 + 零上游请求 | ✅ 已实现 | `AiRoutes.kt:337-358`（**在 append 用户消息与建 Run 之前**，抛 400 `UNSUPPORTED_CONTENT`）; `Adapters.kt:106-108,314-316,452-455`（未实现模态抛 `UnsupportedContentFailure`）; e2e `scripts/e2e-ai-tools-test.ps1:665-680` | 上游请求数为 0 有 e2e 断言（`before == after`） |
| AIH-031 | 图片大小、数量、像素预算 | ◐ 部分实现 | 大小/数量 ✅ `AiDomain.kt:512,551-563,568`、`AiAttachments.kt:157,165` | 缺 ① **像素预算没落地**：`AttachmentFact.pixels`（`AiDomain.kt:447`，由 `AiAttachments.kt:69` 算出）**从不参与比较**，`AiModelDto` 也没有 `maxPixels`（`AiDomain.kt:220-223`）；缺 ② 「转换必须由用户显式确认并形成新投影」**零实现** —— 无压缩/缩放/派生附件，超限只提示"请先压缩或换一张更小的图"（`AiDomain.kt:552-553`） |
| AIH-032 | 视频不允许静默抽帧降级 | ✅ 已实现 | `AiDomain.kt:534,545-550`; `HarnessRunner.kt:646-673,732-737,759-761`; `AiAttachments.kt:286-306`（进请求的是原件） | 唯一的抽帧是**托盘预览帧**（`AiAttachments.kt:258-266` → `MediaFiles.kt:281-346`），界面标注"（已取预览帧）"（`ai_home_page.dart:2011`），**从不作为模型输入** → 不存在静默降级 |
| AIH-033 | comfy_get_status 只读工具 | ✅ 已实现 | `ai/tools/ToolRegistry.kt:363-385`; `Application.kt:160-170` | 参数只有 `includeRecent` / `recentLimit`（上限 20），**不接受任意 URL** ✅ |
| AIH-034 | comfy_get_run 工具 | ✅ 已实现 | `ToolRegistry.kt:387-401`; `Application.kt:171-173`; `CaptureRepo.kt:223-237` | 返回 `run_key`/`prompt_id`/`status`/`media_count`/`title`/`error`/`created_at` = 状态 + 错误 + 产物数 + 关联 ID；查不到如实报 `NOT_FOUND`（`ToolRegistry.kt:399`） |
| AIH-035 | comfy_sync_history + 审批 | ✅ 已实现 | `ToolRegistry.kt:403-416`（`defaultAccess = ASK` `:412`）、`:34-61`（闸门：5 分钟超时 / Run 取消 = 拒绝）、`:595-607`（先 `open` 再 `emit`）; `Application.kt:174`（复用 `pollOnce`） | 「或由设置明确允许」= `permissionMode = full`（`ToolPolicy.kt:131-134`）；「复用 pollOnce 并发锁」✅ |
| AIH-036 | 限制 AI 主动轮询次数 | ◐ 部分实现 | 强制点 ✅ `ToolRegistry.kt:567-579`（`QUERY_BUDGET_EXCEEDED`）; 额度 `ToolPolicy.kt:90`; 提示词插值 `HarnessRunner.kt:1501-1502` | **额度是 9 不是需求写的 3**（2026-09-17 用户要求放宽，`ToolPolicy.kt:86-90` 有注释、`normalizeStored` 保证库里旧值不生效）；机制本身有强制（按 `ToolCategory.COMFY` 计数，`ToolContext` 每 Run 一份 `HarnessRunner.kt:302-306`）。"工具失败如实报告/禁止无限运行"✅ |
| AIH-037 | 扫描内置与用户 Skill 根目录 | ✅ 已实现 | `ai/tools/SkillStore.kt:224-238,451-479,462-464,330-334`; `AiRoutes.kt:533-535,544-554`; `Application.kt:131-142` | 只扫根下一层（bundle/`SKILL.md` 或平铺 `.md`）；无缓存，每次 Run 现读磁盘（`HarnessRunner.kt:222`）；手动重扫 ✅；同名用户版胜出并标冲突（`SkillStore.kt:228-236`） |
| AIH-038 | frontmatter 严格校验 | ✅ 已实现 | `SkillStore.kt:105,133-140,498,502-505`; 禁用 `:241,248-250`; UI 诊断 `ai_home_page.dart:2676,2697-2715` | kebab-case ✅、description 必填 ✅、非法项带 `validationError` 列出但**不进提示、不能加载** ✅。小瑕疵：`/` 补全菜单未过滤 `validationError`（`ai_home_page.dart:1345`），但真正加载仍被后端拒 |
| AIH-039 | 只注入 Skill 摘要目录 | ✅ 已实现 | `HarnessRunner.kt:222,252-261,1030-1040`; `SkillStore.kt:61` | 只给 name + description（压成一行、截 240 字）+ whenToUse（元数据，非正文），最多 60 条 |
| AIH-040 | load_skill 按需加载正文 | ✅ 已实现 | `ToolRegistry.kt:158-183,170-172`; `ToolModel.kt:51-52`; `HarnessRunner.kt:302-306`; `ToolRegistryTest.kt:203-213` | 返回规范 `<skill_content name version>…</skill_content>` 块（`ToolRegistry.kt:175-181`）；单 Run 内重复加载直接报 `SKILL_ALREADY_LOADED` ✅ |
| AIH-041 | 内置注册 Anima 相关 Skills | ⬜ 未实现 | 实测 `skills/builtin` 只有 `img2img-reference/` 与 `krea-2/`；`anima-prompt` / `anima-scene-prompt` / `anima-workflow` 只在 `storage/ai/skills/`（`.gitignore:64` 的 `/storage/` 忽略，`git ls-files storage/ai/skills` 为空） | 「至少包含 anima-prompt、scene、workflow」+「正文、版本与许可完整」**都没达成**。附：`krea-2` 在 git 里甚至**未跟踪**（`git status` → `?? skills/builtin/krea-2/`），许可只在它自己的 `references/SOURCES.md`；`img2img-reference` 无许可声明 |
| AIH-042 | 内置注册 H3 与视频制作 Skills | ⬜ 未实现 | 同上：`h3-prompt-writing` 与 4 个 video generator 全在 `storage/ai/skills/`，内置根没有 | 除"内置注册"外，「模型 Skill 集可选择」也无实现 —— 没有按模型绑定 skill 集的任何入口（`AiRoutes.kt:495-554` 只有全局 list/read/save/delete/roots/rescan） |
| AIH-043 | 允许导入第三方 Skill 目录或 ZIP | ⬜ 未实现 | `SkillStore.kt` 全文无 import/zip；`AiRoutes.kt:495-554` 只有 list/read/save/delete/roots/rescan；`ai_home_page.dart:2465,2789-2820` 明确删掉了导入按钮、改叫"投放口" | 只有"手动拷进 `<storage>\ai\skills` + rescan 自动补 frontmatter"（`SkillStore.kt:353-401`）。需求要的**导入前预览并确认 ❌、原子安装 ❌、可启停 ❌**（`SkillDto.enabled` 永远是 `true`，`SkillStore.kt:516`；无切换接口）、可删除 ✅（只限用户来源，`:297-318`） |
| AIH-044 | 阻断 Skill ZIP 路径穿越与 ZIP bomb | ⬜ 未实现 | 全仓无 ZIP 解包代码（grep `ZipInputStream`/`ZipFile`/`Zip` 在 `server/src/main` 零命中，只有 `PngMeta.kt:9` 的 `InflaterInputStream` 与 `FileKindDetector.kt:109,137-138` 的 MIME 字符串） | 无解压入口，故"绝对路径 / `..` / 链接 / 超大条目 / 超大总解压体积"五类拒绝**一条都不存在**。旁证：`SkillStore.safeChild`（`:430-436`）只保护"注册写的路径"，与 ZIP 无关 |
| AIH-045 | 首期禁止执行第三方 Skill 脚本 | ✅ 已实现 | `ToolRegistry.kt:122-513` 共 14 个工具（`list_skills`/`load_skill`/`register_skill`/`delete_skill`/`remember`/`list_dir`/`read_file`/`write_file`/`comfy_get_status`/`comfy_get_run`/`comfy_sync_history`/`comfy_find_workflow`/`comfy_submit`/`comfy_use_attachment`），**无 shell/exec/process**；`ToolRegistry.kt:72` 注释；grep `shell/exec/ProcessBuilder/Runtime` 零命中 | `scripts/` 目录不进扫描（`SkillStore.kt:461-466` 只认 `SKILL.md` 与平铺 `.md`），只体现在 `fileCount`（`:485-490`）——"展示为不可执行资源"这条只算最小实现 |
| AIH-046 | 设置版本化系统提示 | ✅ 已实现 | `HarnessRunner.kt:982`（`VERSION = "v10"`）、`:90`；六项验收 `:1017-1025`（角色 + 附件诚实）、`:1030-1040`（Skill 目录）、`:1492-1506`（附件诚实 `:1494-1496`、工具审批 `:1497-1499`、防注入 `:1500-1502`、Skill 加载 `:1503`、Comfy 轮询预算 `:1506`）；per-Run 记录 `HarnessRunner.kt:231`、`AiRoutes.kt:395`、`AiRunRepo.kt:77` | 角色 / 附件诚实 / Comfy 查询 / 工具审批 / Skill 加载 / 防提示注入六项逐条对得上 |
| AIH-047 | 用户自定义指令不能覆盖安全段 | ⬜ 未实现 | 全仓 grep `customInstruction`/`userInstruction`/`自定义指令`/`systemPrompt` **零命中**；无 UI 入口 | 「仅作为追加段」「保存版本和 digest」两端都不存在。最接近的 `MemoryStore`（`<storage>\ai\memory.md`，`HarnessRunner.kt:1085-1088` 注入时明写"是数据**不是指令**"）是**另一套功能**，且无版本/digest |
| AIH-048 | 模型选择器显示能力徽标 | ◐ 部分实现 | 选择器 `ai_home_page.dart:1765-1769,1868-1870,1881-1883`；模态全集 `lib/models/ai_models.dart:35-40` | 缺「**不支持项清晰可见**」：选择器只画**已声明**的模态徽标，视频/音频/文档**根本不渲染**（不是灰色或 ✗）。✓/✗ 全量能力只在右侧栏 `ai_home_page.dart:2414-2421`（组件 `:2756-2781`）与设置页 `ai_provider_settings_page.dart:1467-1483`，都不在选择器里 |
| AIH-049 | 工具调用显示状态卡 | ◐ 部分实现 | 名称/徽标/耗时 `ai_home_page.dart:886-901`；审批按钮 `:928-954`；参数+结果 `:955-980`；媒体入口 `:1024-1124` | 缺「**不展示敏感原文**」：工具**参数原样全文贴出**（`ai_home_page.dart:964-966`，不是摘要），结果只**截断不脱敏**（`ToolRegistry.kt:114,116,664-671`）。全仓唯一脱敏是上游报错文本（`AiUpstream.kt:204-216`）；`ai_home_page.dart:829` 注释声称"后端已经把凭据脱敏"但找不到对应实现 |
| AIH-050 | AI 生成提示词可保存到提示词库 | ⬜ 未实现 | `ai_home_page.dart` 里"提示词"只出现 1 次且是系统提示文案（`:1252`）；`lib/core/ai_api_client.dart:58-341` **无任何 `/api/prompts` 调用**；`server/.../Models.kt:72-92` 的 `PromptInput` 无来源会话字段；`prompts.source/source_ref` 是 ComfyUI 来源（`Migrate.kt:23-26`、`PromptRepo.kt:297-339`） | 前端无入口、后端无来源会话列 → **整条未做**（含"带来源会话信息"） |
| AIH-051 | Provider 请求与日志强制脱敏 | ✅ 已实现 | `AiUpstream.kt:204-216`（先抹密钥/Bearer/api_key 再截断 400 字）、`:131`、`:165-168`（日志只打 provider/endpoint/status/code）；`HarnessRunner.kt:844-849,860-862`；`AiRoutes.kt:1033-1040` | Authorization / x-api-key / Key 均不进日志与异常 detail ✅。空成立两处：无「自定义 Header」功能（`AiProviderDto` 无 headers 字段）；无调试 payload 功能（备注里的"临时/警告/自动过期"未实现） |
| AIH-052 | 建立三协议 Fake Provider | ◐ 部分实现 | 三协议流解析 `ProtocolAdapterTest.kt:95,151,254`；工具线格式 `ToolProtocolTest.kt:23-27`；真 HTTP 假网关 `AuthAndModelsUrlTest.kt:37,88-122,127-177`；e2e 假网关 `scripts/e2e-ai-tools-test.ps1:317` | 缺 ① **没有统一的"三协议"假网关**：真 HTTP 只覆盖 `openai-responses` + `/models`，e2e 只有 `openai-completions`，全仓无 anthropic 假网关；缺 ② 401 有真响应，**429/500 只有纯函数映射**（`AiUpstreamTest.kt:21,24`）；缺 ③ **断流零覆盖**；缺 ④ 畸形 SSE 只有单帧畸形（`ProtocolAdapterTest.kt:121-124`）与半截事件（`:66-72`） |
| AIH-053 | 增加 AI 首页 Widget 测试 | ◐ 部分实现 | 默认导航 `test/home_nav_test.dart:77-108`；窄宽 `ai_home_test.dart:242-258,311-322`；阻断 `:431-461`；流 `:463-484`；模型切换 `:398-429`；输入法 `ai_tools_ui_test.dart:1145-1180` | 缺「**取消**」的 widget 测试（`test/` 下 `cancel` / `abort` / `停止` 零命中）。功能本身存在：`ai_workspace_store.dart:1202-1213`、`ai_home_page.dart:1550-1555`、`AiRoutes.kt:477-486` |
| AIH-054 | 跑通 Flutter/Kotlin/Capture 全回归 | ✅ 已实现 | 静态事实：`test/` 26 个文件 / 175 例；`server/src/test/kotlin` 26 个文件 / 304 个 `@Test`；`scripts/e2e-capture-test.ps1`、`e2e-submit-test.ps1`、`e2e-ai-tools-test.ps1` 均在 2026-09-18 补跑：`flutter analyze` 无问题、`flutter test` **178 例全过**、`gradle test` **325 例全过**、`e2e-ai-tools-test.ps1` 67 项、`e2e-submit-test.ps1` 4 幕、`e2e-capture-test.ps1` 全过。另：`README.md` 当时写的 94 / 126 例是旧数字（现已改成 178 / 325） |
| AIH-055 | 同步 README/AGENTS/schema/排错文档 | ◐ 部分实现 | 三处 DDL 同源：`db/schema.sql:344`、`db/migrate.sql:217`、`Migrate.kt:210`；`packaging/manifest.json:20-53`；`AGENTS.md` 第 3/7/10 节 | 缺 ① `README.md:957-958` 用例数 94/126（实测 175/304；`AGENTS.md` 的 168/285 亦过时）；缺 ② `README.md:990` 仍写 `SystemPrompt.render(v3)`，代码是 **v10**（`HarnessRunner.kt:982`）；缺 ③ `docs/ai-tools-and-skills.md:31` 称"**刻意不搬**越狱人格段"，而 `HarnessRunner.kt:1096-1102` 起（`[MODE: UNRESTRICTED SANDBOX — STABLE]` 块）**整段都在系统提示里**（且是用户 2026-09-17 明确要求保留的）；缺 ④ `README.md:979` 测试表断行损坏、漏登记 `test/ai_provider_settings_test.dart` |
| AIK-001 | 默认端口不固定 8080 | ⬜ 未实现 | `Config.kt:66`（默认 `"8080"`，仅 `COMFYHUB_PORT` 可覆盖）；`scripts/server.ps1:22`（`[int]$Port = 8080`）；`scripts/comfyhub.ps1:51`（`$ApiPort = 8080`、`:818-820`）；`lib/core/settings_store.dart:42-49`（默认 baseUrl 写死 8080） | **「随每次启动自动设置可用端口」零实现**：全仓无空闲端口探测（grep `Get-FreePort`/`TcpListener`/`随机` 零命中），App 也没有"读后端实际端口"的机制（`backend_launcher.dart:110,262` 直接用 `settings.baseUrl`）。现状是"**可配置的固定端口**"，不是需求要的动态分配 |
| AIK-002 | 前端大体参考 DSH | ◐ 部分实现 | 深色 `lib/app.dart:54`、`lib/core/theme.dart:48`；`/` 选 skill `ai_home_page.dart:1341-1365,1372`；聊天框内换模型 `ai_home_page.dart:1520,1724-1776,1888-1890` | 五项里 2 项做了、3 项缺失：① **不跟随系统浅/深色** —— `themeMode: ThemeMode.dark` 硬编码（`lib/app.dart:54`），无 `platformBrightness` 判断；② **无文件拖拽** —— grep `DragTarget/desktop_drop/onDrop` 零命中；③ **无 Ctrl+V 粘贴为附件** —— `ai_home_page.dart` 无 paste 处理，`Clipboard` 只用于 `:2605` 复制 skills 路径 |
| AIK-003 | 预留 Android / 远程管理设计 | ◐ 部分实现 | `lib/core/settings_store.dart:44-49`（Android 模拟器默认 `10.0.2.2`）；`lib/pages/settings_page.dart:484`（提示填局域网 IP）；`Config.kt:22-31,55-57`（显式 `COMFYHUB_ALLOW_REMOTE`）；`Application.kt:211-216`（非回环时告警） | 只有"可配 Base URL + 显式开对外监听 + Android 默认地址"这点地基（够 Android 端连本机后端）；**认证、会话令牌、远程管理接口/页面一概没有**（进度文档 `:647` 亦自述"需自行加认证"）。判 ◐ 而非 ⬜ 是因为"预留"这件事有可验证的代码痕迹 |

**合计：31 ✅ / 19 ◐ / 8 ⬜（共 58 条；另补登记 AIH-056/057 两条 ✅，见 xlsx 的「需求清单」与「验收结论」页）**

---

## 3. 与 `docs/ai-home-progress-v0.1.md` 的分歧

进度文档整体比 xlsx 可信得多（它自己在 `:564-565` 就承认 xlsx 状态列不可信），但仍有 18 处需要纠正 ——
其中 **15 处是"高估"**（文档 ✅ 而实测未达标），**3 处是"低估 / 过时"**（代码比文档新，或文档自相矛盾）：

### 3.1 文档高估（文档 ✅ / 实测 ◐ 或 ⬜）

| # | 文档位置 | 文档说 | 代码实际 |
| --- | --- | --- | --- |
| 1 | `:59` | AIH-002 ✅「≥900 三栏，≥1200 才显示右侧栏」 | 同一句自相矛盾：`ai_home_page.dart:190` 是 900 分两栏、`:251` 是 1200 才有第三栏 → **900~1199px 只有两栏**，且宽屏分支没有 FAB（`:250-264`），窄屏唯一入口 `:270-274` 不可达 → **Comfy 状态整块不可达**；「<600px 转 Bar」也没有（`:278-287` 是底部 Sheet） |
| 2 | `:70` | AIH-016 ✅ | 需求里「远程访问需显式启用**和认证**」：认证没有（`Application.kt:218-298` 无 Authentication/intercept） |
| 3 | `:72` | AIH-018 ✅「删除走外键级联（已验证）」 | 验收里的「**无引用附件可回收**」零实现；且 `:387` 说"已被聊天记录引用的会被后端拒绝"，而 `AiRoutes.kt:781-786` → `AiAttachments.kt:272-277` **不查引用直接删**，与文档相反 |
| 4 | `:75` | AIH-021 ✅「`?after=seq`…进程重启后仍可回放」 | 服务端有、**客户端没接线**：`lib/core/ai_api_client.dart:355-381` 丢弃 `id:` 行，`ai_workspace_store.dart:1070` 固定 `after=0` → 端到端"断线恢复"不成立 |
| 5 | `:77` | AIH-023 ✅ 快照 | 快照**只写不读**：`HarnessRunner.kt:206,212,215,222` 全读实时库 |
| 6 | `:78` | AIH-024 ✅「有限重试」 | 没有任何自动重试/退避；人工重试还会重复提问（`AiRoutes.kt:361-372` 不看 `retryOfRunId`）+ 留旧失败气泡 |
| 7 | `:81` | 「AIH-029/030 前后端阻断、零上游请求 ✅」——把 AIH-025~032 合并成一片 ✅ | **AIH-026（从画廊选媒体）整条没做**，文档自己在 §4 第 1 条（`:133`）也承认了 → 表格与正文矛盾 |
| 8 | `:82` | AIH-031 ✅「图片真的能发」 | 验收里的「**像素预算**」「**转换必须由用户显式确认并形成新投影**」都没做（`AiDomain.kt:447` 的 `pixels` 从不比较，`AiModelDto` 无 `maxPixels`；无压缩/派生附件） |
| 9 | `:84` | AIH-053 ✅ Widget 测试 | 缺"取消"用例（`test/` 下 `cancel` / `abort` / `停止` 零命中） |
| 10 | `:85` | AIH-055 ✅ 文档同步 | `README.md:957-958` 用例数 94/126（实测 175/304）、`:990` 仍写 v3（代码 v10）、`docs/ai-tools-and-skills.md:31` 与 `HarnessRunner.kt:1096` 起（越狱人格段）相反、同文件 `:31` 写"系统提示 **v9**"（代码 v10）、`:33` 写"出厂 **13** 个工具"（代码 14 个，`ToolRegistry.kt:122-513`）、`README.md:979` 表格断行损坏 |
| 11 | `:91` | AIH-036 ✅（已自述 3 → 9） | 与需求 xlsx 的「最多 3 次」不符（`ToolPolicy.kt:90` 是 9）——这里是**文档与需求不一致**，不是文档与代码不一致 |
| 12 | `:101` | AIH-047 ◐（"安全规则写成必须遵守…用户自定义追加段的界面还没做"） | 按验收标准应判 **⬜**：不只是"界面没做"，**后端追加段也不存在**（全仓 grep 零命中），更没有"保存版本和 digest"；文档把 `skill_snapshot` 的 digest 当成了 AIH-047 的"digest"，两者不是一回事 |
| 13 | `:97-98` | AIH-043/044 ◐ | 判 **⬜** 更准：文档把"投放口 + 256KB 上限"算作已完成的一半，但需求的主路径是"**导入前预览并确认 + 原子安装 + 可启停**"，这部分一样都没有；AIH-044 更是因为**没有 ZIP 解包代码**而完全不存在（不是"等 ZIP 导入"的半成品） |
| 14 | `:65` | AIH-009 ✅「请求可取消和超时」 | 超时 ✅、**可取消完全没有**（`AiUpstream.kt:121` 阻塞式 `send`，前端无 CancelToken）——文档连提都没提这一半 |
| 15 | `:12` | 声称覆盖「M3 附件（AIH-027 ~ AIH-031）」 | AIH-031 的像素预算与转换没做；AIH-026 也没做 |

### 3.2 文档低估 / 过时（代码比文档新，或文档自相矛盾）

| # | 文档位置 | 文档说 | 代码实际 |
| --- | --- | --- | --- |
| 1 | `:100` | `SystemPrompt.VERSION = v2` | 代码是 **v10**（`HarnessRunner.kt:982`）；README 也还写 v3（`README.md:990`） |
| 2 | `:96` | AIH-041/042「内置根 `<根>\skills\builtin` **仍是空的**」 | 不空了：`skills/builtin` 已有 `img2img-reference` 与 `krea-2`（文档 `:469-482` 自己还写了 krea-2 的引入）→ 文档内前后矛盾。**但判定不变（⬜）**：anima/H3 仍然没有进内置根 |
| 3 | `:564-565` | 警告"xlsx 把**没实现**的需求（AIH-025~032 附件可发、AIH-033~045 工具与 Skills）都标成了通过" | 这句**过宽**：AIH-027/028/029/030/033/034/035/037/038/039/040/045 实测都**真的实现了**。这段警告本身是旧信息，会让人误以为附件与工具整片都没做 |

### 3.3 与本审计一致的地方（互相印证）

- 进度文档 `:96-98`、`:101`、`:103`、`:141-144` 对 AIH-041~044、047、052 的保留/未完成判断方向正确（只是程度与判定符号需按上表修正）。
- 进度文档 `:69`（DPAPI 不回退明文）、`:71`（SSRF 含 DNS 复核与元数据拒绝）、`:79`（签名优先、谎报扩展名被拒）、`:81`（零上游请求）、`:99`（无 shell 工具）与代码完全一致，是本审计里所有"安全不变式 ✅"的印证。

---

## 4. 建议补做清单（按 价值 / 成本 排序）

**A. 极低成本、直接消掉一个缺口（半天以内）**

1. **AIH-002 的 900~1199px 死角**：把 `showPanel` 门槛从 1200 降到 900（`ai_home_page.dart:251`），或在 `wide` 分支补一个 `_showStatusSheet` 的 FAB（`:250-264`）。
   —— 这是当前最容易被用户撞到的功能缺口（一整块状态区不可达）。
2. **AIH-047 用户自定义指令**：后端在 `SystemPrompt.render` 的**末尾**追加一段（`HarnessRunner.kt:1009-1538` 的 `buildString` 尾部，即 `:1508` 之后），
   存进 `app_settings` 并记 `version`/`digest`；前端加一个多行文本框（可放设置页 AI 分区）。安全段保持在前、用户段只追加。
3. **AIK-002 的两项便宜活**：`lib/app.dart:54` 改 `ThemeMode.system`（跟随系统浅/深色）；输入框接 `Ctrl+V`（读剪贴板图片 → 走已有的 `store.attachFiles` 链路）。
4. **AIH-048 选择器里标出不支持**：复用右侧栏现成的 `_CapabilityChip`（`ai_home_page.dart:2756-2781`），
   在选择器行里把未声明的模态画成灰色 ✗（`:1881-1883`）。
5. **AIH-055 文档数字与版本**：`README.md:957-958`（用例数）、`:990`（v3→v10）、`:979`（表格断行）、
   `docs/ai-tools-and-skills.md:31`（越狱段的描述与代码和用户意图相反；同处 v9→v10）、`:33`（工具数 13→14）、补登记 `test/ai_provider_settings_test.dart`。

**B. 中等成本、补齐已铺好的路（1~3 天）**

6. **AIH-031 像素预算**：`AiModelDto` 加 `maxPixels` + `AttachmentPolicy.evaluate` 里比较 `attachment.pixels`（`AiDomain.kt:520-569`），
   纯函数 + 单测。**"转换需显式确认并形成新投影"成本高**，建议先按 DEC-003 的口径把"正确阻断"写进需求，暂不实现转换。
7. **AIH-009 取消**：把模型发现改成可取消（`AiUpstream.discoverModels` 接受 `isActive` 或在 Ktor 侧 `withTimeout` + `call.request.isActive`），
   前端至少加超时。这是"卡住只能等 20 秒"的实际体验问题。
8. **AIH-049 工具卡脱敏**：在 `ToolRegistry.invoke` 的结果/参数进 UI 前过一层 `redact`（复用 `AiUpstream.redact` 的规则，`AiUpstream.kt:204-216`），
   并把参数从"全文 JSON"改成"摘要 + 展开"。
9. **AIH-053 补一条"取消"widget 测试**（`test/` 下现有 cancel 零覆盖），顺带把 AIH-009 的取消也纳入。
10. **AIK-001 动态端口**：`server.ps1` 探测空闲端口 → 写 `.run/api-port`（或让 App 解析脚本输出）→
    `SettingsStore.defaultBaseUrl()` 与 `backend_launcher` 跟着读。**注意这是最容易"改一半"的一项**：
    现在 `scripts/comfyhub.ps1:51`、`server.ps1:22`、`Config.kt:66`、`settings_store.dart:49` 四处都写死 8080，必须一起改。
11. **AIH-018 孤儿附件回收 + AIH-023 快照读取**：前者按"`ai_message_parts` 无引用 + 超过 N 天"回收；
    后者让 `HarnessRunner` 从 `providerSnapshot`/`modelSnapshot` 取值（`:206,212`），并让 `GET /runs/{id}` 回传快照。
12. **AIH-024 重试去重**：`POST /runs` 检查 `retryOfRunId` —— 有值时不 append 新用户消息、复用旧助手消息槽位，否则"重试一次多一条提问"会一直存在。
13. **AIH-052 断流假网关**：在 `scripts/e2e/fake_openai.py` 加一个"发一半就断"的端点，补上目前完全空缺的断流断言。

**C. 成本较高、但需求明写要做（需要先确认产品口径或许可）**

14. **AIH-041/042 内置 Skill 落地**：把 `anima-*` / `h3-*` 从 `storage/ai/skills`（本机运行期数据、被 gitignore）**移入 `skills/builtin`** 并纳入版本控制，
    补 `version` 与许可声明；顺带把**未跟踪的 `skills/builtin/krea-2/` 提交进 git**（否则换台机器/发布包里根本没有它）。
    ⚠ 前置：anima / H3 的正文与许可能否随项目分发需单独确认（进度文档 `:143-144` 也标了这一条）。
15. **AIH-026 从画廊选媒体**：画廊右键 / 详情页加"发送到 AI 工作台"，复用 `AiAttachmentStore` 的引用（需求要求"不重复复制可复用媒体"，
    现在附件表存的是**上传副本**，要么改成"可引用画廊媒体"的第二种来源，要么明确放弃该子条件）。
16. **AIH-050 AI 回复保存为提示词**：`prompts` 加 `source_conversation_id`（`db/schema.sql` / `db/migrate.sql` / `Migrate.kt` **三处同步**），
    AI 首页助手气泡加"保存为提示词"（弹确认框 → `POST /api/prompts`）。
17. **AIH-043/044 第三方 Skill ZIP 导入 + 安全防护**（最后做、也最危险）：解压前逐条校验绝对路径 / `..` / 符号链接 / 单条目与总体积上限，
    解压到临时目录 → 预览确认 → 原子 `move` 到投放口；同时补 `enabled` 开关的持久化与接口。
    —— 在此之前，AIH-044 那五类拒绝**一条都不该宣称已实现**。
