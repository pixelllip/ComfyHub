# 自动选工作流：选择顺序与待做项（委托执行版）

> **起因（2026-11-08 设计讨论的结论）**：自动运行工作流时，候选来源按
> "保守主义（官方模板改节点）"与"经验主义（上次成功的改一改）"两条路组织，
> 用户仍可显式指定。讨论的结论是：**不做成并列模式，做成一条带回退的优先级链 +
> 统一提交闸门**；并在"床上用手机遥控电脑生图"场景下钉死了各档顺序。
>
> **本文是待做项清单（做什么、改哪里、怎么算过）**，委托给执行 AI 落地。
> 动手之前**必须先读**：[`../AGENTS.md`](../AGENTS.md)（两条底线与各节纪律）、
> [`implementation-master-plan.md`](implementation-master-plan.md)（红线与验收归属）、
> [`workflow-recipe-design.md`](workflow-recipe-design.md)（配方三层分解与任务分类判据）、
> [`workflow-blueprint-design.md`](workflow-blueprint-design.md)（体检判据）。

---

## 1. 已定案的选择顺序

### 1.1 通用优先级链（桌面、有人值守）

```
用户指定 > 配方绑定（同任务，exact 键） > 上次成功（同任务，过闸） > 官方模板（过 fit）
```

- 保守与经验**只是候选生成器**，不是两条执行路径：所有候选收敛到同一条链
  `load/convert → diagnose → check_inputs → fit → comfy_submit`。
- 每档失败**自动回退下一档**，并在结果里**如实说**走了哪档、哪档为什么被跳过。
- 修改只走 `comfy_submit(overrides/connections)` 的既有纪律（按键、按原值类型转换、
  不存在抛错、不静默忽略）；绑定只有 `exact` 自动写键。

### 1.2 床上场景（无人值守，手机遥控，只说模型族）的顺序

> 场景：电脑开着，用户躺在床上用手机让电脑生图，最多只说用哪一族模型
> （anima 系 / krea 系 / flux 系），希望在画廊里看到结果。
> **第一准则：零交互跑通的先验概率** —— 任何一次"问用户"都等于流程卡死。

1. **同任务 + 同模型族的最近一次成功运行**（改提示词与 seed 即可，改动面最小）
2. **配方的已验证绑定**（骨架 × 该族模型有"跑通过"的结构化记录）
3. **完整配方 + exact 绑定到该族模型**（未跑过，但骨架完整、绑定无猜测）
4. **同任务的上次成功 / 完整配方，换族重绑** —— 只有 `exact` 许自动继续；
   `substitute` / `missing` **不许停**，如实跳过落下一档
5. **官方模板（按族选）+ `comfy_fit_workflow` 的 suggestedOverrides** —— 兜底

三条场景纪律：

1. **全程不许停在"问"上**：`ask_user_question` 降级为"留言"，只有所有档都失败
   才留一条"需要你确认 X"。候选图 `comfy_check_inputs` 过不掉（等外部参考图）
   直接跳过 —— 除非用户从手机发了照片。
2. **审批是前提，不是流程的一步**：用户须事前把 `comfy_submit` 放进放权档
   （机制现成）。手机侧走 `comfy_queue_submit` 排队 → 桌面在线即执行 →
   产物经 `capture_ingest` 自动入画廊 —— 这条链已存在，**不需要新代码**。
3. **回退全自动、全程记账**：每档失败原因落 provenance（T3），醒来的汇总写清
   "走了哪档、哪档为什么被跳过"。

---

## 2. 待做项

### T1 新工具 `comfy_recent_successes`：按任务查"上次成功" ★核心缺口

**目标**：给 AI 一个"按任务类型找最近一次成功运行"的查询入口。
现状：数据全在 `capture_runs`（`status` / `raw` API 图 / `created_at`），
`capture.rs::recent_runs()` 已有但只按时间排序、不过滤任务；模型只能靠
`comfy_get_status` 的 recent 20 条标题猜，或逐条 `comfy_get_run(raw=…)` 自己分类。

**参数**：`task`（可选，t2i / i2i / edit / inpaint / upscale / video / audio）、
`modelQuery`（可选，按 raw 图里出现的模型文件名子串过滤，用于"同族"匹配）、
`limit`（默认 3，上限 8）。
**返回**：每条约 `runKey` / `promptId` / `title` / `capturedAt` / `task` / 模型文件名清单 /
（可选）`skeletonHash` —— 与配方族对号。
**只许 `status='success'` 且有 `raw`**；`empty` / `error` 不算（那是可重试状态，
见 `capture_ingest::can_reclaim` 的既有语义）。

**关键实现纪律**：

- **任务分类只有一份真源**：复用 `workflow_recipe.rs` 的 `Task` 分类
  （`analyze` 那条路），把分类入口抽出来让本工具与 `comfy_learn_workflow` 共用。
  ⚠ 判据顺序是硬约束（`audio`/`video` 最前、`edit` 先于 `inpaint`），不许另写一份。
- **R12 加工具全流程**：`desktop_tools.rs` 目录项（描述写明"同任务 + 同族优先"）
  + `tools.rs` 的 `IMPLEMENTED` / `execute` 分支 + `defaultAccess = ALLOW`（只读）
  + `mutating = false` + `tool_cache.rs` 的 `freshness` 加 TTL（建议 20s，
  与 `comfy_list_workflows` 同档）+ **bump `PROMPT_VERSION`**（工具清单从 catalog
  派生，签名会变）+ 同步 `run_kit/tests.rs` 与 `ai_run_kit.rs` 的版本/清单断言。
- 手机端**不下发**（mobile catalog 不动；手机提交走 `comfy_queue_submit`，R11）。

**防线（测试）**：

- `tools/tests.rs`：implemented / ALLOW / 非 mutating 断言；`desktop_tools/tests.rs`
  目录清单断言。
- 构造型单测（scratch 库）：造若干 `capture_runs` 行（success/error/empty、不同任务的
  raw 图），断言：只回 success、task 过滤正确、按时间倒序、`modelQuery` 命中、
  limit 截断。落盘只许 `testpaths::scratch()`（AGENTS.md §1）。

### T2 配方"已验证绑定"回写：骨架 × 模型跑通过要记下来 ★经验主义的复利

**目标**：`comfy_submit` 跑通后，把"这条配方 + 这个模型（这组槽位覆盖）已验证"
写回 `workflow_recipes`，下次同模型直接优先（床上场景顺序的第 ② 档）。

**改动点**：

- `workflow_recipes` 加列 `verified_json TEXT NOT NULL DEFAULT '[]'`
  （迁移走 `infra/db.rs` 的幂等 `ALTER TABLE` 老路，参照 `character_cards.prompt`
  那一组；`db/schema.sqlite.sql` / `db/schema.sql` / `db/migrate.sql` 同步）。
  元素形如 `{model, role, overrideKey, runKey, at}`。
- 回写时机：`comfy_submit` 成功路径（等完且入库成功之后）。按 `promptId`
  对上配方（`workflow_recipes.prompt_id`，或 `source_ref = file:<sha256>`），
  把这次实际生效的 overrides 里**属于槽位键**的值记下来。对不上配方就**不记**
  （用户随手跑的不算配方验证）。**失败 / 超时 / 取消绝不许记**。
- `comfy_bind_recipe` 与 `comfy_list_recipes` 的输出带 `verified` 信息：
  绑定结果里目标模型若已在 `verified_json` 里，标注"已验证跑通"并提升优先级表述；
  列表可按 verified 排序。
- ⚠ 该表**不进同步白名单**（既有红线），本项不改变这一点。

**防线**：配方单测（verified 的写入 / 去重 / 读取渲染）；迁移幂等测试（老库无此列
能正常打开并补列）；bind 输出里 verified 优先的断言。

### T3 `capture_runs` 落 provenance：这次提交来自哪一档

**目标**：`capture_runs` 加两列 `source_kind TEXT`（`user` / `recipe` / `recent` /
`template` / 默认 `'ComfyUI'`）与 `recipe_id INTEGER NULL`。
没有它，"哪档成功率高、回退链怎么排"永远查不出来。

**改动点**：

- 迁移同 T2 的幂等 ALTER 老路（三个 db 文件同步）。
- `comfy_submit` 加**可选**参数 `sourceKind`（枚举，缺省不落）与 `recipeId`；
  `recipeId` 尽量**由后端按 promptId 自己推**（对得上配方就填），`source_kind`
  由调用方声明（与 `nsfw` 参数同一条"AI 声明"纪律）。
- ⚠ **轮询捕获那条路（`capture_ingest`）一个字不动**：外部在 ComfyUI 里跑的
  保持默认 `'ComfyUI'`，别把两通路混在一起。
- `comfy_get_run` / `comfy_get_status` 的返回带上这两个键（可空键省略的老规矩）。

**防线**：迁移幂等；submit 落键与推导的构造测试；ingest 路径不改写的回归
（`e2e-capture-test.ps1`）。

### T4 系统提示：选择顺序总规则 + 无人值守纪律

**目标**：把 §1 的链与床上场景三条纪律写进系统提示（`server-rs/src/ai/nsfw_prompt.rs`
的规则区）。

**改动纪律**（⚠ 全是硬约束）：

- **追加新规则条目，不要插在中间导致重新编号** —— 既有条目被按编号引用
  （第 21 条 ↔ `step_mark.rs` 的 `PHASES` 逐字一致、第 25 条 ↔ 布局模板），
  重排号会静默断链。
- 内容要点：优先级链（§1.1）；床上场景顺序（§1.2）与"问用户 = 卡死"的语义；
  回退要如实说；候选 `check_inputs` 不过就跳过；放权档是无人值守的前提。
- **必须 bump `PROMPT_VERSION`**（`ai/nsfw_prompt.rs`），并同步 `run_kit/tests.rs`
  与 `ai_run_kit.rs` 的版本断言 —— bump 的判据是"模型看到的规则变没变"。
- 文案按 AGENTS.md §6.2：说人话，不堆内部代号。

**防线**：`system_prompt/tests.rs` 的前缀稳定用例不红；`run_kit` 版本断言同步绿。

### T5 【需用户拍板，不拍不动】无人值守的审批语义

**问题**：床上场景要求提交那一下没人点批准。两个选项：

- A. 维持现状语义：用户事前把 `comfy_submit` 放进放权档（机制现成），
  提示词只写"无人值守前请确认已放权"。
- B. 给 `comfy_queue_submit`（手机排队）加"授权窗口"语义：用户发起时显式授权
  本 task 族 N 次 / M 分钟内免审批，桌面执行队列时查这个授权。

倾向 A（机制零改动，语义清楚）；B 涉及审批闸门语义变更，**没有用户明确拍板不许做**。
T4 的提示词按拍板结果写。

### T6 边缘项：多实例归属

`capture_runs` 没记"上次成功是在哪个实例跑通的"，跨实例不算数。
**本期只做**：T1 工具的返回里带上当前实例 URL（让人判断），结构性字段以后再说。

---

## 3. 实施顺序与验收

1. **T1 → T3 → T2 → T4**（工具与存证先行，提示词最后写 —— 提示词描述的必须是
   已经存在的能力）；T5 拍板后并入 T4；T6 随 T1。
2. 开工前先 `git commit` 或 `git stash` 落基线（AGENTS.md 顶部两条底线之一）；
   代码改动只走 edit / write 工具。
3. 完成后按 AGENTS.md §1 / §10.1 跑回归（都不出网、不花钱）：
   `cargo test`（受限会话红了的先看 `testpaths::scratch()` 纪律）、
   `pwsh -File scripts\e2e-submit-test.ps1`、`pwsh -File scripts\e2e-capture-test.ps1`、
   `pwsh -File scripts\e2e-recipe-test.ps1`（改了配方链路时）。
   **别在跑完前 Ctrl+C**（清理在 `finally` 里）。
4. 全部落地后，床上场景闭环 = 手机发一句"用 anima 画 …" → 队列执行 →
   画廊出现产物（`mediaIds` 回执），全程无一次"问用户"，醒来那条汇总写清
   走了哪一档。

---

## 4. 2026-11-08 前期准备（实测登记）

### 4.1 三条基准工作流（本次实测）

| 工作流文件 | 族 | 任务 | promptId | 正提示词键 | 种子键 | 前缀键 |
| --- | --- | --- | --- | --- | --- | --- |
| `MiaoMiao Harem.json` | anima | t2i | 1450 | `86.text` | `88.seed` | `85.filename_prefix` |
| `krea2 t2i workflow.json` | krea2 | t2i | 1449 | `55.value`（PrimitiveStringMultiline；`34` 是 CLIPTextEncode，可能被连线覆盖） | `42.noise_seed` | `39.filename_prefix` |
| `image_moss_image_t2i.json` | moss | t2i | 1448 | 子图 `3301b744…` 内的 `4.text`（顶层覆盖不到，先跑默认） | 子图内 `7.seed` | `9.filename_prefix` |

> 这三条就是 T1 的**首批回归数据**：跑完后 `capture_runs` 会各留一条 `success`，
> `raw` 里的模型文件名（`moss_image_*` / `krea2Turbo*` / `silvermoonmixAnima*`）
> 可直接用于 `modelQuery` 的"同族"匹配。
>
> **2026-11-08 实测：三条都跑通。** `moss-sakura`（1217 / 1219）、
> `anima-rooftop`（1218）、`krea-fixed`（1221）。`krea2 t2i` 原文件的
> `Anything Everywhere`（node 36）曾让提交审核按"纯前端节点"整张拦下；
> 现在 `workflow_submit` 在预检前会摘掉广播桩（`strip_broadcast_stubs`），
> 提交图里不再带这个零输出桩。
>
> 回执自动拉取已补（2026-11-09）：手机前台每 25 秒 `pullChanges`
> （已配对、非运行中、不重叠；后台/未配对时停），桌面回执最多 25 秒内出现。

### 4.2 放权开关已落地（对应 §1.2 第 2 条与 T5）

- `POST /api/auth/devices/{id}/auto-approve` `{enabled}`（**仅回环**），按设备
  `autoApproveSubmits`，默认关。
- 开启后 `submit_queue` 受理即写 `approval='auto'`，worker 直接往下走，
  不再等桌面点批准；回执保留 `auto`，结果卡显示「自动提交」。
- 撤销即时生效（下一次受理按新状态）；这是**用户显式放权**，代价是拿到该设备
  或令牌的人可以不经过桌面确认就消耗 GPU。

### 4.3 T1 的数据形状（本次运行会自然产生）

`comfy_recent_successes` 可以先用这三条 success 做构造型回归：按 `task=t2i` 过滤、
`modelQuery` 分别命中 `moss_image` / `krea2Turbo` / `silvermoonmixAnima`、
按 `created_at` 倒序、只认 `success` 且 `raw` 非空。任务分类仍复用
`workflow_recipe::Task` 的既有判据顺序，不另写一份。

### 4.4 T1 已落地（2026-11-09）

- `comfy_recent_successes`（桌面专属、`ALLOW`、只读、20s 缓存）已注册；
  `PROMPT_VERSION` v63；全量 `cargo test --lib` 870 passed。
- 手机端不下发（仍只有那 7 个工具）；后续按 §3 顺序继续 T3（provenance）→
  T2（verified 绑定）→ T4（系统提示，需先拍板 T5）。

### 4.5 T3 已落地（2026-11-09）

- `capture_runs` 加 `source_kind`（默认 `ComfyUI`）/ `recipe_id`，schema v14
  （幂等 `ALTER`，`schema.sqlite.sql` / `schema.sql` / `migrate.sql` 同步）。
- `comfy_submit` 可选 `sourceKind` / `recipeId`；`recipeId` 缺省按 `promptId` 推；
  `comfy_get_run` / `comfy_get_status` 返回来源档；轮询捕获路径一个字没动。
- `cargo test --lib` 872 passed；下一步 T2（verified 绑定）→ T4（系统提示）。

### 4.6 T2 已落地（2026-11-09）

- `workflow_recipes` 加 `verified_json`（默认 `'[]'`），schema v15（幂等 `ALTER`；
  ⚠ `workflow_recipes` 这张表**只在 SQLite 那份 schema 里**，MySQL 两份早停了，所以这条只改 sqlite 那份）。
  元素形状 `{model, role, overrideKey, runKey, at}`。
- 回写只有一处：`workflow_submit` 里 `comfy_submit` **跑通之后**
  （`!failed && (imported>0 || duplicates>0 || already_captured)`）。
  失败 / 超时 / 取消不记；对不上配方（`recipeId` 推不出来）不记；
  只记**这次真的写进 overrides 且属于槽位键**的那几条（旋钮不算"这个模型能用"）。
- 去重键是 `(overrideKey, 模型名)`（模型名按 `ModelName::same` 的同一把尺子比）：
  同一组合再跑通只刷新 `runKey` / `at`，新组合插到最前，只留 40 条。
  `save_row`（重新学一份）**不动**这一列 —— 验证是事实，不随这次归纳的槽位变。
- `comfy_bind_recipe`：命中已验证时 `bindings[].verified` / `verifiedAt` 为真，
  正文带 `✔ 已验证跑通`，并给"直接开跑"的结论；**能不能自动写覆盖键的判据仍然只有 `exact`**
  （已验证只影响文案与提示，不放松写权限）。
- `comfy_list_recipes`：每条带 `verified[]` / `verifiedCount`，正文多一行"已验证跑通：…"。
- 回归：`workflow_recipe/tests.rs`（去重 / 不被重新学习清掉 / 绑定标注 / 写法不同仍认出）、
  `workflow_submit/tests.rs`（只认槽位键）、`db/tests.rs`（老库补列幂等）、
  `recipe_test.rs`（列表键集合多一个 `verified`）。`cargo test --lib` 878 passed。

### 4.7 T4 已落地（2026-11-09；T5 按 A 档写）

- 系统提示**追加第 27 条**（不动既有编号）：优先级链（用户点名 > 配方 `exact` 绑定 /
  `✔ 已验证跑通` 优先 > 上次成功 > 官方模板 + fit）、哪档不成**自动落下一档并如实记账**、
  无人值守（手机遥控）时"问用户 = 卡死"、`check_inputs` 过不掉的候选直接跳过、
  前提是事前把 `comfy_submit` 放进放权档。
- **T5 按 A 档落地**（§2 T5 的倾向 A）：机制一个字没改，提示词只写"无人值守前请确认已放权"；
  B 档（授权窗口语义）**没有做** —— 那要改审批闸门语义，须用户明确拍板。
- `PROMPT_VERSION` v63 → **v64**，`run_kit/tests.rs` 与
  `tests/api_test/ai_run_kit.rs` 的版本断言同步。
- 同一条里也写了 T2 / T6 带来的输出变化（verified 标注、instanceUrl），
  因为"模型看到的能力"确实变了，一次 bump 覆盖这三项。

### 4.8 T6 已落地（2026-11-09）

- `comfy_recent_successes` 的返回值多一个 `instanceUrl`（当前实例地址），正文末尾写清
  "这些记录不区分实例，换过实例的话先确认它还能跑再照抄"；
  工具描述同步这一句（模型可见 —— 与 T4 的 v64 是同一次 bump）。
- `capture_runs` 仍然**不记**每行属于哪个实例（结构性字段以后再说）。
- 回归：`tools/tools/tests.rs` 断言 `instanceUrl` 与那句说明。

### 4.9 本轮收尾（2026-11-09）

- T1 / T3 / T2 / T6 / T4 全部落地；**T5 按 A 档**（不改审批闸门语义，B 档等用户拍板）。
- 回归结果：
  - `cargo test` **全量绿**：lib 878、api_test 174、recipe 7、submit 9、submit_queue 8、
    watch_stdin 2 等（`confinement_probe` / `spot_check` / `live_submit_shapes` /
    `train_mcp_live` 是 `#[ignore]` 的活体用例，按设计不跑）。
  - `scripts\e2e-recipe-test.ps1` **32 项全通过**（离线；配方链路改了所以必须跑）。
    第 1 幕需要一份 API 格式工作流，仓库不再随包带模板，用
    `-ApiWorkflow server-rs\tests\fixtures\api_inpaint_fixture.json` 那份
    （3 槽位 + 遮罩链 → task=inpaint，恰好覆盖脚本的断言）。
  - `scripts\e2e-submit-test.ps1` **41 项全通过**、`scripts\e2e-capture-test.ps1` **全通过**
    （12:37 补跑，都是假 ComfyUI / 假网关，不占显卡、不出网）、
    `scripts\e2e-ai-tools-test.ps1` **111 项 0 失败**（改了系统提示，顺带过一遍上下文拼装）。
    跑完已 `backend.ps1 stop`，端口文件与自动捕获配置都还原了。
