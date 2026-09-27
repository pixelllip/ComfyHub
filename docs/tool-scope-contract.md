# 🔒 工具分端契约：ToolScope（冻结，M0）

> **状态：已冻结（2026-09-23）。** 依据 `docs/implementation-master-plan.md` §3.5 与红线 **R11 / R12**。
>
> 落地物：`ToolModel.kt` 的 `enum class ToolScope` + `AgentTool.scope`（默认 `DESKTOP`）
> + `ToolRegistry.toolsFor(scope)` / `specs(policy, scope)`；
> 防线：`server/src/test/kotlin/com/comfyhub/ai/tools/ToolScopeTest.kt`（会**从本文件读** MOBILE 表来比对）。

---

## 1. 这是什么、不是什么

**是**："别让模型看见一个它根本调不到的工具"。

**不是**：安全边界。安全边界仍然是 `ToolPolicy`
（写白名单、`.git`/`.mysql`/`.run` 永远禁写、审批闸门）。两者别混：
`ToolScope` 过滤的是**下发给模型的清单**，不是执行时的判定。

### 三个值 + 一个"全集"

| 值 | 含义 | 谁用 |
| --- | --- | --- |
| `ALL` | **两端合起来**的全集 | **桌面自己跑 Run 时的默认值** —— 它的存在是为了让这一步**纯加字段、零行为变化** |
| `DESKTOP` | 只有桌面能履行 | comfy_*、MCP 拉来的 39 个、本机文件读写、skill 注册/删除 |
| `BOTH` | 两端都能做（移植过去的那一套） | 见 §2 的 7 个 |
| `MOBILE` | 只有手机有 | 手机独有的 `comfy_queue_submit`（R11） |

**默认值刻意是 `DESKTOP`，不是 `BOTH`**：新增工具时忘了写 `scope`，后果是
"**手机上少一个工具**"（看得见、能发现），而不是"手机上多一个调不到的工具"
（看不见、只表现为模型一本正经地胡说）。MCP 那 39 个工具也因此自动落在 `DESKTOP`。

### R11：手机不许复用 `comfy_submit` 这个名字

桌面离线时它**无法履行**"提交 → 等生成 → 返回 mediaIds"的契约。
叫同一个名字的后果不是报错，而是**模型误报成功**（它会如实汇报"我提交了"，
而实际上什么都没发生）。手机侧叫 **`comfy_queue_submit`**，
系统提示里必须写明"**排队 ≠ 已提交**"。

---

## 2. MOBILE 可执行的工具（正好这 8 个，唯一真源 = `ToolRegistry` 里的 `scope =`）

| 工具 | 为什么手机能做 |
| --- | --- |
| `web_search` | 纯出网调用，不依赖桌面 |
| `web_fetch` | 同上 |
| `civitai_search` | 同上 |
| `list_skills` | skill 目录真源在本地磁盘，两端各有一份 |
| `load_skill` | 同上（正文从本地读） |
| `remember` | 长期记忆是本地文件（`memory.md`），两端各有一份 |
| `ask_user_question` | 走**本端**事件流（`user.question` / `.answered`），不需要桌面在场 |
| **`comfy_queue_submit`** | **只有手机有**（`scope = MOBILE`）：它只把"提交意图"排进本机 `_outbox`，**不是提交**（R11）。真正提交由桌面在它自己的审批通过后执行 —— 所以它**不进桌面的 `implemented()`**，但**必须在 run-kit 的手机清单里**：`declaredTo = kit ∩ 本地实现`，不在 kit 里的话手机 harness 实现了也**不会被宣称**，模型永远看不到它（2026-09-27 实测踩到过） |

> `ToolScopeTest` 会**从这张表里读**名字去比对 `toolsFor(MOBILE)`，所以
> **改这里必须同时改代码、改测试之外的任一处都会被拦下**（漏了哪边都红）。
>
> ⚠ 2026-09-27 补记：这张表原先漏了 `comfy_queue_submit`（只列了 7 个 `BOTH`），
> 于是 Rust 侧的 `mobile_tools.rs` 与 run-kit 也跟着漏 —— 三处一起补上，
> 并把 `api_test` 的两条清单断言（`/api/ai/tools` 的 `tools[]`、run-kit 的 `tools[]`）
> 一起改成 8 个。

---

## 3. 桌面专属（`DESKTOP`，默认落在这一档）

| 类别 | 工具 | 为什么手机不行 |
| --- | --- | --- |
| ComfyUI | `comfy_get_status` `comfy_get_run` `comfy_sync_history` `comfy_find_workflow` `comfy_list_workflows` `comfy_load_workflow` **`comfy_submit`** `comfy_use_attachment` `comfy_check_inputs` `comfy_fit_workflow` `comfy_env_report` `comfy_audit_workflow` `comfy_prune_workflow` | 手机**不直连 ComfyUI**、不自起服务端（架构纪律 1）；提交永远由桌面执行（纪律 2） |
| MCP | comfy-mcp 的 39 个（`extraTools` 运行时注入） | 手机不装 Python/MCP |
| 文件 | `read_file` `write_file` `list_dir` | 写白名单只认**桌面**的 `<项目根>\comfyui`；手机有自己的沙箱 |
| Skills 写 | `register_skill` `delete_skill` | 投放口在桌面的 `<storage>\ai\skills` |
| 找模型 | `find_model` | 要读本机模型目录 + 三站定向查，属于桌面能力 |

> ⚠ **手机发起的 Run 若被路由到桌面执行**（桌面在线时），工具全都可达 ——
> 那是**策略问题**，不是可达性问题。**2026-09-27 用户拍板，见 §3.1。**

---

## 3.1 跨端发起：能力不砍，审批加强（2026-09-27 用户拍板）

总纲 §7 待定项 3 问的就是"`ToolScope` 到底是可达性过滤还是策略开关"。答案是
**仍然是可达性过滤**，但**跨端发起的 Run 要加强审批**：

| 情形 | `ToolScope` 的作用 | 执行时的判定 |
| --- | --- | --- |
| 手机发起、**手机执行** | `MOBILE` 子集（kit ∩ 本地实现） | 手机自己的闸门（R8 的顺序一样） |
| 桌面发起、桌面执行 | `ALL`（现状，零行为变化） | 桌面 `ToolPolicy` + 审批闸门，照旧 |
| **手机发起、桌面执行** | 用桌面的**全量清单**（不提前砍能力） | ★ **`mutating` / ASK 类工具一律强制走桌面审批**，哪怕桌面上把它设成了"自动允许" |

三条理由（为什么不是"手机发起就只读"）：

1. **只读化会把能做对的事做砸**：模型会以为"手机上不能读文件 / 不能查盘"，
   于是把本来一次就能问清的事拆成好几轮，或者干脆告诉用户"做不到"。
2. **纯"自动允许"档的隐患是真的**：用户把某个写工具设成自动允许，本意是
   "我在桌面上点过、信任它" —— 而手机可能在大厅里、在别人手里。
3. **"谁发起"必须是一个显式事实**：Run 要有 `initiatedBy`（`desktop` / `mobile`），
   它随 Run 落库、进事件流 —— 否则事后没人能回答"这次是谁让改的桌面文件"。

### 与 R8 的关系（顺序一个字不改）

`ToolApprovalGate.open(callId)` **永远在发 `tool.requested` 之前**；跨端这条只是
**多一个"必须问"的来源**（把原本"按配置自动放行"的那些重新拉回闸门），
不是另起一条审批通道。**手机点过"提交"不算批准**（总纲 §7 待定项 2）：
`submit_job` 的消费者在桌面侧照样要问一次。


---

## 4. 不变的既有限制（R12）

- 工具清单**只有一个真源**：`ToolRegistry` 的 `tools` 列表。系统提示、`GET /api/ai/tools`、
  界面都从它派生 —— **别在别处再维护一份"可用工具"名单**。
- 改系统提示词**必须 bump `SystemPrompt.VERSION`**。
- 手机侧拿到的清单来自 run-kit 下发（🔒 §3.4），**不是手机自己硬编码**（R13）。
