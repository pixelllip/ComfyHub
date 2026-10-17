# docs/archive/ —— 已归档的调研 / 审计 / 进度快照

**这里不是当前依据。** 放进来的是**一次性**的东西：做完了的任务书、被取代的进度快照、当时的现场记录、只调研未实施的方案。

活着的结论在 `docs/` 根目录：目标架构与红线看 [`../implementation-master-plan.md`](../implementation-master-plan.md)、库结构看 [`../sqlite-schema-contract.md`](../sqlite-schema-contract.md)、事件契约看 [`../run-event-contract.md`](../run-event-contract.md)、同步看 [`../sync-api-contract.md`](../sync-api-contract.md)、工具接线看 [`../ai-tools-and-skills.md`](../ai-tools-and-skills.md)、排错看 [`../pitfalls.md`](../pitfalls.md)。

## 归档清单

| 文件 | 是什么 | 为什么归档 | 结论去哪了 |
| --- | --- | --- | --- |
| `ai-home-progress-v0.1.md` | AIH-001~057 的逐轮实现进度（100KB） | 做完了；状态列后来被独立审计取代 | 需求真源 [`../ai-home-requirements-v0.1.xlsx`](../ai-home-requirements-v0.1.xlsx)；红线与验收在 `implementation-master-plan.md` |
| `ai-home-implementation-plan-v0.1.md` | 同一批需求的分步实施方案（2026-09-14 审阅稿） | 已实施完 | 同上 |
| `ai-home-requirements-audit.md` | 逐条读代码的独立审计（每条给 file:line） | 审的是**已删除的 Kotlin 代码**（`HarnessRunner.kt` / `ToolRegistry.kt`），必然过期 | 现状看代码本身 |
| `embedded-db-sync-options-report.md` | 六个同步方案为什么都不用（163KB 深度调研） | 决策已落地 | `sqlite-schema-contract.md` / `sync-api-contract.md` / `cross-device-sync-design.md` |
| `audit-client-architecture-android-readiness.md` | 客户端架构 + 安卓就绪度审计 | 审计对象是 Kotlin 后端 | `mobile-harness-design.md` / `android-client-plan.md` |
| `workflow-blueprint-research.md` | 工作流「蓝图化」查看的可行性调研 | 只调研、未实施 | 要做得重新评估（`pitfalls.md` 里那几条渲染坑仍然有效） |
| `dsh-tool-layer-report.md` | DSH 工具层逐项实测（对照用） | 一次性对照记录 | `ai-tools-and-skills.md` 第 1 节的对照表 |
| `comfy-mcp-report.md` | 接 comfy-mcp 的查证与决定 | 已实施完 | `ai-tools-and-skills.md`、`server-rs/src/tools/mcp.rs` |
| `conversation-review-9.21.md` | 2026-09-21 对话记录的复盘与优化清单 | 一次性复盘 | 逐条已落到对应功能 |
| `copy-review-design.md` | "AI 文案审校"独立软件的设计 v2（v1 已废弃） | 不在本仓库实施、无入站引用 | 若还要做，从这里起 |
| `todo-10.11.md` | 用户 2026-10-11 的口述待做清单 | 已实施 | 代码本身 |
| `todo-10.11-implementation.md` | 上述清单的可实施方案（含拍板记录） | 已实施 | 代码本身 |

## 规矩

1. **引用归档件要带 `archive/`**（`docs/archive/xxx.md`）；相对链接按所在目录算。
2. **活文档不许放进来** —— 契约 / 设计 / 排错表留在 `docs/` 根目录。
3. **新归档先在清单里登记一行**（是什么 / 为什么 / 结论去哪了），再挪文件；否则三个月后没人知道它是什么。
