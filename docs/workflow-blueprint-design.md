# 工作流蓝图与接线体检（设计 + 实测）

> **起因（用户 2026-10-17 的原话）**：
> 「1. 让我捕获的工作流能像 comfyUI 一样，可以用蓝图的形式预览乃至修改（**特别注意有些前端 only 的节点**）；
> 2. 学习 krea2 i2i workflow 和 krea2 t2i workflow，让我的项目能够理解这两种常用工作流的结构，
> 让它自己能写出来；3. 对于**错误接线**要能向用户指出。」
>
> **一句话回答**：把"工作流"从**一坨 JSON** 变成**一张能画的图 + 一份能读的体检报告**。
> 画与判都在**后端**（判据要 ComfyUI 的 `/object_info`，只有后端问得到），前端只负责画与改；
> 编辑**只改原文里那一个字段**（按节点 id 定位），所以前端 only 节点、坐标、分组一个都不会丢。
>
> 调研底稿（2026-09-18，含"为什么不做内嵌 litegraph"）：[`archive/workflow-blueprint-research.md`](archive/workflow-blueprint-research.md)。

---

## 1. 三件事分别落在哪

| 用户要的 | 落点 | 判据真源 |
| --- | --- | --- |
| ① 蓝图预览（含前端 only 节点） | `server-rs/src/workflow/workflow_blueprint.rs`（模型）+ `lib/widgets/workflow_blueprint_page.dart`（画布） | 界面格式用**文件里的真实坐标**；API 格式没有坐标 → 分层排 + 界面上如实标注 |
| ① 蓝图**修改** | `lib/core/workflow_blueprint.dart` 的 `WorkflowEditor`（按 id 改原文）+ `POST /api/workflow/save` | 原文是唯一真源；编辑**只碰那一个字段** |
| ② 学会 krea2 两种工作流 | `comfyui/workflows/comfyhub_{t2i,i2i}_krea2.json`（模板）+ skill `krea-2` §6（结构）+ 配方库两条 | 转换链的实测结论（见 §4） |
| ③ 错误接线要指出来（**界面**） | 同 `workflow_blueprint.rs` 的 13 条判据 + 界面上的问题清单/标记 | 见 §3 的表，每条都写明"什么时候报" |
| ③ 错误接线要指出来（**AI**） | 工具 `comfy_diagnose_workflow`（`path` / `promptId` / `graph` 三选一，只读、出厂 allow） | **与界面同一份** `build()` —— 两处判据分叉的表现是"界面说没事、AI 说缺件" |

---

## 2. 蓝图是什么（数据形状）

`build(raw, object_info, diagnose) -> Blueprint`，`to_json()` 给界面、`render()` 给模型：

```jsonc
{
  "format": "ui" | "api" | "unknown",   // 三态，判据与后端转换一致（`is_ui_workflow` / `is_node_graph`）
  "objectInfo": true,                   // 拿不到 /object_info 时体检覆盖面小一圈（界面要如实说）
  "counts": { "nodes": 15, "edges": 14, "groups": 3, "errors": 0, "warnings": 3, "infos": 1 },
  "nodes": [{
    "id": "36", "class": "Anything Everywhere", "title": "Anything Everywhere",
    "pos": [6194.2, 949.4], "size": [181.4, 106.0], "mode": 0,
    "kind": "broadcast",                // normal/note/reroute/variable/broadcast/display/bypasser/subgraph/unknown
    "frontendOnly": true,
    "inputs":  [{ "name": "anything", "type": "CLIP", "link": 45, "widget": false, "value": null }],
    "outputs": [],
    "widgets": [{ "index": 0, "name": "seed", "value": 42 }]
  }],
  "edges": [{ "id": 45, "from": {"node":"32","slot":1}, "to": {"node":"36","slot":0}, "type":"CLIP", "implicit": false }],
  "groups": [{ "title": "主链", "bounding": [-100,-60,1300,400], "color": "#3f789e" }],
  "issues": [{ "code":"TYPE_MISMATCH", "severity":"error", "title":"…", "detail":"…", "fix":"…",
               "nodes":["2","3"], "links":[12], "slots":["3.samples"] }],
  "warnings": ["…"]
}
```

四条硬约束（都有出处）：

1. **两种 `links` 写法都认**（老版数组 / 新版对象）—— 判据只有 `workflow_convert::raw_links` 一份。
2. **控件值两份都认**：`widgets_values`（只有位置）与 `widgets_values_named`（有名字，**优先**）。
   只有位置又没有 `/object_info` 时，如实给 `name: null`（界面显示"参数 #0"），**不猜名字**。
3. **前端 only 节点一个都不丢**（`Note` / `MarkdownNote` / `Reroute` / `Set/Get` / 广播 / 显示 /
   rgthree 那一族 / 组节点实例）：蓝图不是转换，它画的是**用户存下来的那张图**。
4. **API 格式没有坐标**：位置是分层排出来的（`layer = 1 + max(上游)` + 同层重心排序，**确定性**），
   界面上有一条常驻提示说清楚"这与你 ComfyUI 里看到的那张不是同一张"。

---

## 3. 接线体检的 13 条判据

| 码 | 严重度 | 什么时候报 | 需要 `/object_info` |
| --- | --- | --- | --- |
| `DUPLICATE_ID` | 错误 | 两个节点用同一个 id | 否 |
| `LINK_DANGLING` | 错误 | 线的一头指向不存在的节点 | 否 |
| `LINK_NOT_FOUND` | 错误 | 输入写着"我接了线 N"，但 `links` 表里没有 N | 否 |
| `LINK_SLOT_MISMATCH` | 警告 | `links` 表与节点插口对不上（存档被改过 / 迁移残留） | 否 |
| `TYPE_MISMATCH` | 错误 | 上游输出的类型与下游输入声明的类型不同（ComfyUI 原话 `Return type mismatch between linked nodes`） | 是（界面格式可用连线上的类型兜底） |
| `INPUT_UNCONNECTED` | 错误/警告 | 插口空着（有 `/object_info` 且必填 → 错误；否则只提醒） | 判必填要 |
| `CYCLE` | 错误 | 图里有环 | 否 |
| `NO_OUTPUT_NODE` | 错误 | 一个输出节点都没有（`Prompt has no outputs`） | 是 |
| `UNREACHABLE` | 提示 | 有节点不参与出图（**判据是转换之后的图**） | 是 |
| `FRONTEND_NODE` | 警告 | 图里还有纯前端节点（服务端没有这个类） | 是（离线只能按名字猜） |
| `UNKNOWN_CLASS` | 错误 | `/object_info` 里没有这个类（没装那个节点包） | 是 |
| `BROADCAST_STUB` | 警告 | 广播型节点被注册成"零输出桩"：语义只在前端 | 是 |
| `CONVERT_GAP` | 错误/警告 | **提交链**（`to_api_graph`）留下的缺口：悬挂引用 / 空必填 | 是 |
| `PARAM_MISMATCH` | 警告 | 控件值个数与输入声明对不上（参数会错位） | 是 |

### 3.1 三条"看着无所谓、其实承重"的规矩

1. **"空必填"只有一个判据**：`Converted.required_open_inputs`（配方学习 / 蓝图体检 / 提交前清单三处共用）。
   `open_inputs` 把**可选**插口也列进去了（那是给广播用的候选清单）—— 拿它当缺口会凭空多出假警报，
   实测代价是"整份 i2i 工作流学不下来"（`TextGenerate.video` / `.audio` / `RMBG.background_color` 都是可选）。
2. **拿不到 `/object_info` 就不下结论**：不判类型、不判必填、不判"本机没有这个类"，
   只做结构检查（连线表、重复 id、环、空插口提醒），并在界面上如实说明覆盖面小一圈。
3. **"哪些节点不参与出图"要看转换之后的图**：界面图里广播节点的下游看不出连线（由前端补上），
   直接按界面图算会把真正干活的那一串全报成"不参与出图"（2026-10-17 在 `krea2 t2i` 上实测误报了 5 个节点）。

---

## 4. krea2 两份真工作流：实测结论（**判据来自真实提交记录**）

### 4.1 现场

用户 2026-10-17 从 ComfyUI 里存出两份工作流（`krea2 t2i workflow.json` / `krea2 i2i workflow.json`）。
它们**依赖三件只有前端才会做的事**，而这三件事当时一件都做不了：

| 现场 | 当时的结果 | 现在 |
| --- | --- | --- |
| `Anything Everywhere`（cg-use-everywhere）**注册进了 `/object_info`**，注册的是"零输出"桩（`input.optional.anything=["*"]`、`output=[]`） | 原判据"查得到就原样保留" → 既不改写、又不产出 → 该广播的 MODEL/CLIP/VAE 一路都没填 | `workflow_frontend::emulate` 对"注册了但 `output` 为空"的**广播型**节点照样改写（**显示型不动**：丢掉 `PreviewImage` 会把"只想看一眼"变成"自动补 `SaveImage` 落盘"） |
| `Fast Bypasser (rgthree)` 不在 `/object_info` 里 | 被当"没装的节点"摘掉 → `105.model ← 节点 90（没有转换出来）` → **整份工作流进不来** | `FRONTEND_PASSTHROUGH`：恰好 1 个接了线的输入 → 把下游接到真正的上游（形状不对就不猜） |
| `TextGenerate.sampling_mode` 是 `COMFY_DYNAMICCOMBO_V3` | 当成插口（"必填空着"）；子项 `sampling_mode.temperature` 被丢掉（参数静默退回默认值） | `is_widget_input` 认 `COMFY_DYNAMIC*`；`fill_widgets` 收 `名字.子项` |

### 4.2 判据怎么来的（**没有一条是猜的**）

本机 ComfyUI 的 `/history` 里留着那次 i2i 运行**前端真正发出去**的 API 图：

```json
"105": {"inputs": {"model": ["89", 0], "control_latent": ["104", 0]}, "class_type": "Krea2ControlApply"}
"43":  {"inputs": {"sampling_mode": "on", "sampling_mode.temperature": 0.7, "sampling_mode.top_k": 64, …},
        "class_type": "TextGenerate"}
```

两条直接结论：① 直通开关在前端确实被**穿透**了（`105.model` 直接指向 89）；② 动态下拉框的子项在 API 图里
**就是带点的扁平键**。源码侧的印证：`comfy_api/latest/_io.py` 的 `DynamicCombo._expand_schema_for_dynamic`
把选中项的嵌套输入按 `前缀.子项` 加进 `required`/`optional`；rgthree 的 `base_node_mode_changer.js`
在 `onConstructed` 里 `addOutput("OPT_CONNECTION", "*")`。

### 4.3 修完之后

| 模板 | 节点 | 转换结果 | 配方 | 体检 |
| --- | --- | --- | --- | --- |
| `comfyhub_t2i_krea2.json` | 15 | 15 个 API 节点；广播补 5 条线 | `task=t2i`，`complete=true` | 0 错误 / 3 警告 / 1 提示 |
| `comfyhub_i2i_krea2.json` | 29 | 27 个 API 节点（2 个直通开关被穿透）；广播补 5 条线 | `task=i2i`，`complete=true` | 0 错误 / 4 警告 / 1 提示 |

活体回归：`server-rs/tests/krea2_templates.rs`（`--ignored`，直接拿仓库里那两份模板跑，要求**缺口为零**）。
结构说明与可覆盖键写在 skill `krea-2` §6（AI 照着它就能用这两份出图，不必现写）。

---

## 5. 编辑：**只改原文里那一个字段**

| 动作 | 落到原文的哪 | 注意 |
| --- | --- | --- |
| 改控件值 | `nodes[i].widgets_values[k]`（按位置） | 同时同步 `widgets_values_named`（**后端优先读它**）；名字对不上位置时把那一份**删掉**（留着旧值会让编辑静默失效），后端会退回按声明顺序读位置那份 |
| 改节点状态 | `nodes[i].mode`（0 正常 / 2 静音 / 4 旁路） | |
| 改标题 | `nodes[i].title`（空串 = 删掉，回落到类名） | |
| 改坐标 | `nodes[i].pos` | 目前**没有**做画布拖动（见 §7） |

出口两条：**复制**（粘回 ComfyUI）与**保存**（`POST /api/workflow/save` → `<根>\comfyui\workflows\`）。
落点由**后端白名单**决定（`ToolPolicy::resolve_write`），前端不拼路径 —— 文件名只取最后一段，
`../../evil.json` 会被净化成 `evil.json` 且仍在白名单内（2026-10-17 实测）。

改完**就地复检**：`POST /api/workflow/blueprint` 拿新的结论（不落库、不动原文件），所以
"我这么改会不会接错"当场就能看到。

---

## 6. 明确不做（诚实清单）

| 不做 | 原因 |
| --- | --- |
| 内嵌 litegraph / ComfyUI 前端 | Flutter Windows 没有 DOM；要引 WebView 一类新依赖、要打进发布包，而且**解决不了 API 格式没有坐标**（调研 §4.3） |
| 在蓝图上**增删节点 / 重连线** | 那会长出第二套转换栈（现在只有 `workflow_convert` 一份）。"改图"要动的是**结构**，得先有 `object_info` 的必填校验 + 转换预检才敢放开 —— 列为下一阶段 |
| 画布上拖动节点 | 手势与 `InteractiveViewer` 的平移冲突（要自己接管平移才行），收益是观感，先不做 |
| 把编辑结果写回用户自己的 ComfyUI 目录 | 写入白名单**只有一份**（`ToolPolicy`）。另开一条路等于让"什么能写"出现第二个判据，而它出问题的表现是**静默写到任意路径** |
| 子图（`definitions`）展开后画内部节点 | 蓝图里画的是**实例那一个框**并如实说明"转换时会展开"；展开本身在 `workflow_convert::flatten` 里（转换那条路），蓝图不重复实现 |
| golden / 截图回归 | 全仓 0 处 golden；画布断言的是**可测的量**（画了几个节点、剔了几个、层序、边方向），观感靠开发期 debug 版人工看 |
| 离线反推 `widgets_values` 的参数名 | 数组形式**没有键名**，只有 `/object_info` 的声明顺序能还原；拿不到就显示"参数 #n" |

---

## 7. 下一阶段（按价值排）

1. **画布上拖动节点**（`setNodePosition` 已经写好，缺的是手势接管）。
2. **结构编辑**（增删节点 / 重连线）：前置是 `/object_info` 必填校验 + 转换预检，产物必须过 `CONVERT_GAP` 才算改成功。
3. **蓝图上的"提交这次修改"**：现在要经过"保存文件 → `comfy_load_workflow` → `comfy_submit`"三步，
   可以把这三步合成一个动作（判据不变：提交永远走 `comfy_submit`，R10）。
4. **`Label (rgthree)` 之类第三方注释框的语义化**（当成真正的分组标题）—— 第三方形状不稳定，先按"矩形 + 文字"画。

---

## 8. 回归防线归属

| 事实 | 防线 |
| --- | --- |
| 蓝图模型 / 布局 / 编辑 / 画布剔除 | `test/workflow_blueprint_test.dart`（15 例：模型、坐标、分层确定性、视口剔除、命中、编辑只碰一个字段、页面接线） |
| 接线体检的每一条判据 | `server-rs/src/workflow/workflow_blueprint/tests.rs`（17 例） |
| 广播改写（含零输出桩） | `workflow_frontend/tests.rs` + `workflow_convert/tests.rs::注册成零输出桩的广播节点照样等价改写` |
| 直通开关 / 动态下拉框 | `workflow_convert/tests.rs::rgthree_直通开关把下游接到真正的上游` / `动态下拉框的子项要跟着主项一起进图` |
| "缺口只算必填" | `workflow_convert/tests.rs` 与 `workflow_blueprint/tests.rs::有object_info时空插口归转换那一遍说_且只算必填` |
| 两份真模板转换零缺口 | `server-rs/tests/krea2_templates.rs`（`--ignored`，要 ComfyUI 在跑） |
| AI 侧的接线体检工具 | `tests/api_test/tools.rs` 两条（不依赖 ComfyUI：指向不存在节点的线要点到 `2.vae`；路径准入 / 三选一缺参数都要说人话）+ `tools/tools/tests.rs` 的出厂档 + `tests/api_test/ai_run_kit.rs` 的工具清单 |
| 端点 | `POST /api/workflow/blueprint` / `POST /api/workflow/save`（界面上走通即覆盖；活体抽查见 §4.3） |
