# AI 工作台实施进度（v0.1 首次实施记录）

> 日期：2026-09-15
> 依据：`docs/ai-home-requirements-v0.1.xlsx`（需求清单 / 待确认决策 / 风险清单 / 里程碑）
> 与 `docs/ai-home-implementation-plan-v0.1.md`（实施方案）
> 本轮范围：**M1（Provider / 模型 / 凭据）+ M2 的持久化底座 + M0 的安全前置项**

## 1. 本轮完成的提交

| 提交 | 内容 |
| --- | --- |
| `需求基线…` | 需求 xlsx 与实施方案入库；修复 `Content-Disposition` 转义 |
| `AI 工作台 M1 地基…` | 安全收紧（回环监听 + CORS 白名单）、Provider/模型/凭据领域、附件准入规则、`/api/ai/*` |
| `AI 工作台成为默认首页…` | 三栏工作台、会话/消息持久化、附件阻断提示、默认导航切换 |
| `文档同步…` | AGENTS 第 3 节、README 功能/目录/接口表 |
| `AI 模型设置页…` | Provider 配置界面、模型目录编辑、只写密钥界面 |

## 2. 逐条对照需求

| 需求 | 状态 | 说明 |
| --- | --- | --- |
| AIH-001 默认落地页 | ✅ | `HomeShell` 默认 `AI 工作台`，顺序 AI 工作台→画廊→提示词→标签→设置；`home_nav_test` / `localization_test` 已同步 |
| AIH-002 宽屏三栏 / 窄屏 | ✅ | ≥900 三栏，≥1200 才显示右侧栏；窄屏会话进抽屉、状态进底部 Sheet，Composer 常驻 |
| AIH-006/007 Provider 自定义 + revision | ✅ | ID 校验（kebab-case、创建后不可改）、URL 校验与规范化、乐观锁冲突返回明确错误 |
| AIH-008 连接测试 | ✅ | `POST /api/ai/providers/{id}/test`：请求前 SSRF 复核、不跟随重定向、10 秒超时、状态码映射到稳定错误码；日志只记 Provider/端点/状态码，**不含密钥与 Header**（有单测） |
| AIH-009 模型发现 | ⬜ | 未做（同上；发现只产生候选的规则已在方案里定稿） |
| AIH-010/011 手工能力声明、不猜能力 | ✅ | 模型目录是能力真源；UI 与预检都以目录为准，未知模态直接拒绝 |
| AIH-012/013 凭据只写 + set/describe/unset | ✅ | 所有 DTO 只含 `{configured, source, writable}`；`resolve` 仅后端内部 |
| AIH-014 改密钥下次请求生效 | ✅（设计成立） | 每次请求重新 `resolve`，不缓存到进程常量；尚无 Run 可验证端到端 |
| AIH-015 Windows 凭据方案 | ✅ | DPAPI(CurrentUser) 加密落盘；DPAPI 不可用时**写入直接失败**，绝不退化为明文 |
| AIH-016 监听/CORS 收紧 | ✅ | 默认 `127.0.0.1`；`COMFYHUB_ALLOW_REMOTE=1` 才对外；CORS 由 `anyHost()` 改为本机 + 白名单 |
| AIH-017 SSRF 保护 | ✅（保存时 + 请求前复核） | 地址范围判定、信任级别匹配、云元数据地址任何级别都拒绝、DNS 解析后复核 |
| AIH-018 会话 CRUD | ✅ | 新建/改名/归档/删除；删除走外键级联（已验证） |
| AIH-019 有序消息块 | ✅ | `ai_message_parts`（text/attachment/tool_call/tool_result），按 `seq`+`ordinal` 无损恢复 |
| AIH-020~024 Run / SSE / 取消 / 快照 / 错误码 | ⬜ | M2 主体，尚未开始；错误码常量与准入快照思想已落地 |
| AIH-027 严格文件识别 | ✅（分类器 + 预检接入） | `FileKindDetector` 签名优先、未知即 UNKNOWN；`StrictIntake` 让**服务端判定压过前端声明**（前端谎报 image 骗不过准入，有单测）。上传落盘链路要等 M3 |
| AIH-028 能力矩阵 | ✅ | 模型声明 ∩ 适配器实现 ∩ MIME ∩ 大小/数量，任一不满足即阻断（有单测） |
| AIH-029/030 前后端阻断、零上游请求 | ◐ | 前端以后端 preflight 为准并禁用发送；`preflight` 是纯计算，不发上游请求。真正的"Run 准入"要等 M2 |
| AIH-048 能力徽标 | ✅ | 模型选择器与侧栏都按目录声明显示，未声明的一律标不支持 |
| AIH-053 首页 Widget 测试 | ✅ | `test/ai_home_test.dart`、`test/ai_provider_settings_test.dart` |
| AIH-055 文档同步 | ✅ | AGENTS 第 3 节 + README 功能表/目录树/API 表/测试表 |

图例：✅ 完成　◐ 部分完成　⬜ 未开始

## 3. 验证证据（本轮实测，非推断）

- `pwsh -File scripts\server.ps1 test` → **26 项全通过**（含新增 `AiDomainTest`）。
- `flutter analyze` → 无问题；`flutter test` → **58 项全通过**。
- 起真实后端（`comfyhub.ps1 up`）后逐条打过：
  - 建 Provider（loopback 信任级别）→ 写密钥 → **状态接口只返回 `{configured, source: managed}`，无值**；
  - 模型目录写入 2 个模型；预检：text-only 模型 + PNG → `blocked`；声明了 image 但适配器未实现 → 仍 `blocked`；
  - revision=99 更新 → 按预期返回冲突并说明当前 revision；
  - 会话 → 两条消息 → 恢复出 2 条（块数 2 / 1）→ 归档后默认列表不含它 → 删除后 `ai_conversations/ai_messages/ai_message_parts` 均为 **0 行**（级联生效）。
- 顺带修掉一个真实缺陷：删除接口原先返回 `mapOf("deleted" to true, "id" to id)`，kotlinx 不支持元素类型不同的集合，运行时会 500。

## 4. 未完成 / 下一步（按建议顺序）

1. **M2 会话与流式文本**：`ai_runs` / `ai_tool_calls` / `ai_run_events` 表；三种协议的 SSE 适配器；
   `POST /runs` + `GET /runs/{id}/events?after=seq` + `cancel` / `retry`；把工作台里的"消息已保存"换成真实流式回复。
2. **M3 附件上传链路**：把 `FileKindDetector` / `StrictIntake` 接到真实上传（落盘时读文件头判定，
   而不是等前端传 base64），并把"未知即 IMAGE"的老回退彻底隔离在 AI 侧之外。
3. **AIH-008/009**：Provider 连接测试与模型发现（候选不落库）。
4. **M3 图片真正发送**：实现图片内联后，把 `AdapterCapabilities.transportsFor()` 从空集改为实际实现，
   并在 Run 准入处再验一次（AIH-030 的"上游请求数为 0"用例）。
5. **M4/M5**：ComfyUI 只读工具（含 3 次/轮的主动查询上限）、Skills 目录与 `load_skill`、第三方 Skill 安全导入。
6. `ai_runs` 之前不要对外宣称"能聊天"：现在的工作台会如实说明模型执行尚未接通。

## 5. 给下一个协作者的注意事项

- **别把密钥塞进 `app_settings` / SharedPreferences / 日志**：唯一入口是 `CredentialService`。
- 新增 AI 表时，`db/schema.sql`、`db/migrate.sql`、`Migrate.kt` **三处必须同步**（启动自动补齐靠 `Migrate.kt`）。
- `AdapterCapabilities` 是"协议适配器到底实现了什么"的唯一事实来源；实现新传输方式时先改它，
  否则预检会把附件判为"适配器尚未实现"——这是故意的，宁可阻断也不乐观放行。
- 默认监听已改为回环；如果要用 Android 客户端连本机后端，需要显式 `COMFYHUB_ALLOW_REMOTE=1` 并自行加认证（AIH-016、AIK-003）。
