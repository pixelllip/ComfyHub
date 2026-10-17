# 工作流「蓝图」化查看：调研与可行性设计

- **调研对象**：`docs/bug-and-suggestion-9.18.md` 第 26 行 —— "查看工作流功能，可以效仿 comfy 内部那样展开成蓝图显示出来（难度比较大，先调研）"
- **调研时间**：2026-09-18（代码与数据库均为当次实测）
- **调研方式**：只读代码 + 只读查询本机 MySQL（`127.0.0.1:3307 / comfy_hub`），未改任何代码、未写库。
  也读了工作区里**尚未提交**的在飞改动（`WorkflowConvert.kt` 等，见 2.5）—— 它们直接推翻了"仓库里没有图形侧能力"这个初始假设
- **一句话结论**：**分两阶段做**——先做「结构化大纲」（A，1~1.5 人日），再做「只读蓝图画布」（B，3.5~5 人日）；**不做**内嵌 litegraph.js / ComfyUI 前端（C）

---

## 1. 结论摘要

### 1.1 推荐

1. **先做 A（结构化大纲/树），再做 B（只读蓝图画布），C 明确不做。**
2. A 与 B **共用同一个解析层**：把工作流 JSON 归一化成 `WorkflowGraph{nodes, edges, groups, format}`，
   解析层必须有**三态**结果：`ui` / `api` / `unknown`（不是 JSON 或不是工作流 → 回落现在的 JSON 文本视图）。
   现在的 `looksLikeApiGraph()` 只返回一个 bool（`lib/widgets/workflow_viewer.dart:12-15`），不足以支撑绘制。
3. 蓝图**只读**：不编辑、不连线、不提交。提交这条路已经有 `comfy_submit` +
   `ComfySubmitter.WorkflowEdit.applyOverrides`（见 `AGENTS.md` 10.1），而且"界面格式 → API 格式"
   的在线转换**也已经有实现**（`WorkflowConvert.kt`，见 2.5）—— 蓝图里再长一条会立刻变成两套逻辑。
4. **API 格式没有坐标**，这是本需求最大的硬约束（第 3 节有实测证据）。蓝图画布必须对 API 格式
   显式标注"位置是自动排的，和 ComfyUI 里的布局不一样"，不许让用户误以为看到了原图。
5. 蓝图画布**不要塞进现在的 `AlertDialog`**：正文被 clamp 在 760×460（`lib/widgets/workflow_viewer.dart:183-185`），
   而库内真实工作流的节点坐标包围盒约 9600×2400 px（实测，见 3.1）。建议**保留现有弹窗**（JSON 原文 + 复制 + 4 种状态），
   在弹窗里加一个「看蓝图」入口，由弹窗把**已经取到的原文**传给一个整屏画布页
   （`Navigator.push` + `fullscreenDialog: true`）。这样 `showWorkflowDialog` / `WorkflowButton` 的签名不变，
   两个详情页和老用例的接线部分都不用动（见 6.4）。

### 1.2 工作量估计

| 阶段 | 内容 | 预估（人日） |
| --- | --- | --- |
| P0 | 三态解析层 + 归一化模型 + 单元测试 | 0.5 ~ 1 |
| P1 | 大纲视图（节点列表 + 连接关系）+ 回归 | 0.5 ~ 1 |
| **小计（建议先做）** | A 方案落地，用户能立刻受益 | **1 ~ 1.5** |
| P2a | 蓝图画布：UI 格式真实坐标（盒子/连线/分组/旁路灰显/视口剔除/缩放平移） | 2 ~ 3 |
| P2b | API 格式分层布局（拓扑分层 + 重心排序 + "猜测布局"提示条） | 1 |
| P2c | 大图性能降级（节点数上限、LOD、抽稀） | 0.5 ~ 1 |
| **小计（P2）** | B 方案落地 | **3.5 ~ 5** |
| P3（可选） | 节点搜索/定位、点节点看参数、导出 PNG | 1 ~ 2 |
| **合计（P0~P2c）** | 推荐范围 | **5 ~ 8 人日（约 1.5 周）** |

> 说明：以上是**估算**，不是承诺。P0/P1 的风险很低（纯 Dart 解析 + 现成的列表组件），
> P2a 是主要不确定项（画布交互与性能），P2b 的算法本身不复杂但"排得好看"要多调几轮。

### 1.3 三条硬约束（写进需求，不许违背）

1. **不许在纯前端 / 离线状态下从 `widgets_values` 反推参数名**。它在绝大多数情况下只有位置、没有键名
   （实测 `["res_multistep"]`、`["max(5, round(a * 24)) + …"]`），猜就是错的 ——
   `README.md:414-415`、`server/src/main/kotlin/com/comfyhub/ComfyCapture.kt:281-289` 都写明了这一点。
   **唯一正确的反推方式是问 ComfyUI 要节点定义**（`/object_info` 才有每个节点的输入声明顺序）——
   这件事仓库里**已经有实现**：`ComfySubmitter.objectInfo()`（`ComfySubmitter.kt:90-125`，带 TTL 缓存）
   + `WorkflowConvert.kt`（见 2.5）。所以蓝图的第一阶段**不做**参数名映射；
   真要做也只能走这条在线路径，不许自己写一份猜测逻辑。
   （补充：少数自定义节点把 `widgets_values` 存成**对象**而不是数组，那种情况**是有键名的** ——
   `WorkflowConvert.kt:200-207`。所以"UI 格式一定没有参数名"这句话也不准确，正确说法是
   "**数组形式**没有键名，且无法离线还原"。）
2. **不许画 API 格式时假装有布局**。没有坐标就是没有，自动排的必须标注。
3. **不许在 UI 线程上一次性布局上千个节点**，也不许把整份 JSON 丢给 `SelectableText`
   （现有 400 KB 上限 `lib/widgets/workflow_viewer.dart:22-26` 的注释就是这个原因）。

---

## 2. 现状（读代码得出，非推测）

### 2.1 现在的「查看工作流」只做一件事：把 JSON 原样交到用户手上

`lib/widgets/workflow_viewer.dart`（312 行）：

| 位置 | 内容 |
| --- | --- |
| `:12-15` | `looksLikeApiGraph(Object?)`：**顶层每个 value 都是带 `class_type` 的 Map** 才算 API 格式；空 Map、非 Map、非 JSON 一律 `false` |
| `:22-26` | `_maxDisplayChars = 400 * 1024`：单次渲染字符上限（注释：超大文本丢给 `SelectableText` 布局会卡住好几秒） |
| `:31-43` | `showWorkflowDialog()`：`showDialog` 打开，先抓住调用方的 `ScaffoldMessenger` |
| `:46-70` | `WorkflowButton`：`TextButton.icon` + `account_tree_outlined` 图标，label 默认「查看工作流」 |
| `:84-134` | 弹窗状态机：`_loading` / `_raw` / `_display` / `_apiFormat`；`_load()` 里 `await widget.load()`，异常就地展示 |
| `:136-153` | `_render()`：≤400KB 走 `JsonEncoder.withIndent('  ')`；超过就切开头 + 提示省略了多少字符；`_safeCut()` 防代理对劈开 |
| `:164-177` | `_copy()`：**永远复制完整原文**（不是截断/格式化后的） |
| `:183-185` | 弹窗正文尺寸：`width = (屏宽-120).clamp(280, 760)`、`height = (屏高-260).clamp(220, 460)` |
| `:212-311` | `_body()` 的四种状态：转圈 / 错误+重试 / 没存过 / 正文；正文是 `SingleChildScrollView` + `SelectableText`（`:297-306`） |

**没有**任何节点、连线的概念 —— 它不知道工作流里有几个节点、谁连着谁。

### 2.2 数据从哪来、是什么形状

```
ComfyUI /history ──► ComfyCapture.captureRun() ──► ingestClaimed() ──► prompts.workflow_json
                     (ComfyCapture.kt:258-279)     (:300-410)           media_assets.workflow_json
                                                    │                    (PromptRepo.kt:297-339
                                                    │                     MediaRepo.kt:210-227)
                                                    └─► capture_runs.raw（原文，含 API 图）
```

- **原文解析**：`HistoryEntry.parse()`（`ComfyCapture.kt:717-771`）。`/history` 里的一条运行是
  `{"meta":…, "prompt":[编号, prompt_id, 节点图, extra_data, 输出节点, 敏感数据]}`（六元组，实测
  `capture_runs.raw` 的 `raw` 就是它）。`isNodeGraph()`（`:762-763`）**按内容认**节点图，
  `workflowOf()`（`:768-770`）从 `extra_data.extra_pnginfo.workflow` 取 **UI 格式**工作流。
- **存哪一份**：`ComfyCapture.kt:332-337` —— 优先 `req.workflow`（UI 格式），没有就退回 `req.prompt`
  （**API 格式节点图**），两者都序列化成 JSON 存进 `workflow_json`。所以同一列里可能躺着两种格式。
- **API 图另有一条路**：`ComfyCapture.apiGraphOf(promptId)`（`:281-298`）从 `capture_runs.raw`
  里再取一次 API 图（提交任务用），对应接口 `GET /api/prompts/{id}/api-graph`（`CaptureRoutes.kt:113-128`）。
  这条不影响查看器，但说明"API 图是权威的、可再取的一份数据"。
- **参数解析**：`GraphParse.parse()`（`GraphParse.kt:83-158`）只从 API 图里抠提示词/采样参数，
  **完全不碰 pos/size/links** —— 也就是说，现有的工作流解析能力全都在"参数"那一侧，没有"图形"那一侧。

### 2.3 谁打开这个弹窗

| 入口 | 位置 |
| --- | --- |
| 提示词详情页（`p.hasWorkflow` 时） | `lib/pages/prompt_detail_page.dart:169-173` |
| 产物详情页（`media.hasWorkflow` 时） | `lib/pages/media_detail_page.dart:644-648` |
| 前端取数 | `lib/core/api_client.dart:347-365`：204/404 → `null`；2xx → `utf8.decode(bodyBytes)` |
| 后端接口 | `GET /api/prompts/{id}/workflow`（`CaptureRoutes.kt:105-111`）、`GET /api/media/{id}/workflow`（`:130-135`），没有就 204 |

两个入口调的是同一个 `showWorkflowDialog`，所以**蓝图只要替换弹窗内部**，两个详情页不用改
（但入口按钮的 label/tooltip 可能都要跟着调）。

### 2.4 现在被测试钉住的事实（`test/workflow_viewer_test.dart`，315 行 / 9 个用例）

| 用例 | 行 | 钉住的事实 |
| --- | --- | --- |
| 读取过程中先转圈 | `:175` | 转圈 + 「正在读取工作流…」 |
| 界面格式展示格式化 JSON | `:192` | 出现 `"type": "KSampler"`（证明走过 `JsonEncoder.withIndent`）+ 提示语「这是 ComfyUI 的工作流 JSON，拖回 ComfyUI 即可复现」 |
| API 格式换提示语 | `:208` | 出现「API 格式节点图」 |
| `looksLikeApiGraph` 单测 | `:221-227` | API/UI/`{}`/`null`/非 JSON 五种输入 |
| 没存过 → 空状态 | `:229` | 「这次生成没有保存工作流」+ 没有「复制」 |
| 复制全文 | `:243` | 复制的是**后端原文**（`_uiWorkflowJson`），不是展示文本；SnackBar 文案 |
| 读取失败 → 弹窗内错误 + 重试 | `:263` | 不把整页变成错误页 |
| 提示词详情接线 | `:281` | 徽标 + 「查看工作流」+ 打开 + 关闭 |
| 产物详情接线 | `:299` | 同上（API 格式分支） |

夹具只有 **1 个节点**（`_uiWorkflowJson`，`:32-34`）和 **2 个节点**（`_apiGraphJson`，`:37-38`），
**没有任何多节点/多连线的样本** —— 蓝图实现时第一件事就是补夹具（见第 5 节的 P0 验收）。

仓库里 **0 处 golden / matchesGoldenFile**（全仓 grep 无命中），`AGENTS.md:24-28` 的测试取舍标准是
"删了以后出错还能不能被测试抓住"，`:124-135` 的界面约定是"长列表懒构建 / 高矮差别大的用固定行高"，
`:34-41` 则要求界面类改动用 debug 版重启后**人工截图确认**（截图是给人看的，不进 CI）。
所以**回归一律写成 widget 测试断言可测的量（节点数、懒构建、分层关系、剔除命中数），不写截图**。

### 2.5 仓库里已有的「图形」能力（含工作区里尚未提交的在飞改动）

> ⚠️ 本节引用的 `WorkflowConvert.kt` 与 `AiWorkflowSearch.kt` 的改动
> **在 2026-09-18 时还是工作区里的未提交状态**（`git status` 里 `WorkflowConvert.kt` 是 `??`）。
> 它们是"正在做"的东西，行号可能随时变 —— 读的时候以文件内容为准。

我在开始调研时假设"仓库里完全没有图形侧的解析能力，工作流只是一坨 JSON"，**这个假设是错的**：

| 已有能力 | 位置 | 对蓝图的意义 |
| --- | --- | --- |
| **UI 格式 → API 格式的在线转换** | `WorkflowConvert.kt:76-185`（`toApiGraph(ui, objectInfo)`），调用方 `AiWorkflowSearch.kt:142-165` | 证明"UI 格式不是不可解析的"：只要拿到 `/object_info`，`widgets_values` 就能按输入声明顺序贴回名字上。蓝图的解析层可以**借用它的连线/虚拟节点知识**，但**不要复用它做提交** |
| **节点定义表（`/object_info`）的拉取与缓存** | `ComfySubmitter.objectInfo()`（`ComfySubmitter.kt:90-125`，带 TTL 缓存） | 将来若要给 UI 格式也显示真实参数名，这是**唯一正确**的入口（见 4.1 的可选增强） |
| **`mode` 语义有代码依据** | `WorkflowConvert.kt:129-142`：`mode == 2` = 静音（永不执行，ComfyUI 自己也会摘掉）；`mode == 4` = 旁路，且注释明确写"前端会把它接过去，规则与类型强相关，猜错代价太高 —— 如实拒绝" | 印证 3.1 实测的 61/91 个 `mode:4`。**蓝图必须把 `mode` 画出来**，而且**不许**尝试"把旁路节点还原成实际执行的样子" |
| **`links` 有两种写法** | `WorkflowConvert.kt:259-281`：老写法是数组 `[link_id, origin_id, origin_slot, target_id, target_slot, type]`，**新写法是对象** `{id, origin_id, origin_slot, …}` | ⚠️ **蓝图解析必须两种都认**。我最初只按实测到的数组形式写，那样会漏掉新版本 ComfyUI 存的工作流 |
| **前端专用"虚拟节点"清单** | `WorkflowConvert.kt:62-64`：`Note` / `MarkdownNote` / `PrimitiveNode` / `Reroute` / `SetNode` / `GetNode`；`:30-33` 举了**无法等价转换**的例子 `Anything Everywhere` | 蓝图会遇到"`/object_info` 里根本没有的节点类型"。要按"前端专用节点"分类渲染（`Note` 画便签、`Reroute` 画成小中转点），**不能假设每个节点都有后端定义** |
| **UI 格式判据** | `WorkflowConvert.isUiWorkflow()`（`WorkflowConvert.kt:70`）：`obj["nodes"] is JsonArray` | 前端 `looksLikeApiGraph()` 是另一套判据（`workflow_viewer.dart:12-15`），两者**不是互补关系** → 蓝图必须自己给一个明确的三态判定（见 P0 验收） |
| **`_meta.title` 的来源之一** | `WorkflowConvert.kt:172-174`：转换时会把 UI 节点的 `title` 写进 `_meta.title` | 解释了 API 图里为什么有的节点带中文标题（3.1 实测：只有新数据带） |

**对蓝图的两条直接结论**：
1. 解析层要**同时**处理两种 `links` 写法、`widgets_values` 的数组/对象两种形态、以及"没有后端定义"的节点类型；
2. "UI 格式能拿到真实参数名"是**可能的**，但**需要 ComfyUI 在线**（`/object_info`）。
   第一阶段不做（离线也必须能用），列为 P3 的可选增强。

---

## 3. 数据可得性

### 3.1 本机库实测（只读查询，2026-09-18）

```sql
-- ① 规模
SELECT COUNT(*) total, SUM(workflow_json IS NOT NULL) with_wf FROM prompts;        -- 14 / 13
SELECT COUNT(*) total, SUM(workflow_json IS NOT NULL) with_wf,
       MAX(CHAR_LENGTH(workflow_json)) max_len FROM media_assets;                  -- 14 / 13 / 114129
SELECT COUNT(*) FROM capture_runs;                                                 -- 67

-- ② 区分两种格式：UI 格式有 $.version，API 格式没有
SELECT id, CHAR_LENGTH(workflow_json) len, JSON_EXTRACT(workflow_json,'$.version') ver,
       JSON_LENGTH(JSON_EXTRACT(workflow_json,'$.nodes'))   n_nodes,
       JSON_LENGTH(JSON_EXTRACT(workflow_json,'$.links'))   n_links,
       JSON_LENGTH(JSON_EXTRACT(workflow_json,'$.groups'))  n_groups
FROM prompts WHERE workflow_json IS NOT NULL ORDER BY id DESC;
```

**结论（重要）**：

| 事实 | 实测值 |
| --- | --- |
| `prompts` 有工作流的行 | 13 行：**UI 格式 3 行**（id 117 / 116 / 115，`version: 0.4`）、**API 格式 10 行** |
| `media_assets` 同样 | 13 行：UI 3 行（id 126 / 125 / 124）、API 10 行 |
| 体积 | 最大 **114 129 字节**（`media_assets.id=125`，MiniMaxH3 视频）；`prompts` 最大 106 699 字节（id 115） |
| 与 400 KB 上限的关系 | 现有上限是**防御性**的，不是当前数据的常态（当前最大 114 KB ≈ 上限的 28%） |
| UI 格式节点规模 | id 116：**91 节点 / 95 连线 / 4 组**；id 117：12 节点 / 13 连线 / 1 组 |
| UI 格式坐标范围 | id 116：x ∈ [−2403.5, 7217.3]，y ∈ [5324.4, 7691.3] —— **只按 `pos` 算的包围盒约 9620×2370 px（还没算节点自身的 size）**；id 117：x ∈ [−1314, 335]，y ∈ [−894, 219] |
| 节点尺寸 | 极不均匀：最大一个 `Label (rgthree)` 是 **7547×300**，采样器 242×527，普通节点 140×46 |
| **旁路节点** | id 116 的 91 个节点里 **61 个 `mode: 4`**（旁路/未启用），只有 30 个 `mode: 0` |
| 注释节点 | id 116 里有 6 个 `Label (rgthree)`（第三方注释框） |
| 同一次运行的两份图 | `capture_runs.run_key='75d33d29-…'`：**API 图 18 节点 vs UI 图 91 节点** |
| API 格式节点标题 | id 113 / 118 / 119 **12/12 个节点都带 `_meta.title`**（如 `"VAE解码"`、`"保存图像"`）；但 id 82/81/51/43/42/26/25 **0 个节点带 `_meta.title`** |
| API 格式节点数 | 10 ~ 17 个（id 119 = 12，id 51 = 17） |
| 子图 | 3 份 UI 工作流里 `$.definitions` **都不存在**（没有子图样本可参考） |
| 初始视角 | 3 份 UI 工作流都有 `extra.ds = {"scale":0.47~0.74, "offset":[x,y]}`（`frontendVersion: "1.52.7"`）；`extra.title` 均为 null |

> 这些数字**只代表本机这一次的样本**。换成别的用户（经常用 SDXL 小图、或用 300 节点的 Wan/Flux 图），
> 分布会不一样 —— 所以下面的阈值都要可配置，不要写死。

### 3.2 两种格式的字段对照（对绘制而言）

| 要素 | UI 格式 `{"nodes":[…],"links":[…],"version":0.4}` | API 格式 `{"<id>":{"class_type":…,"inputs":{…}}}` |
| --- | --- | --- |
| **节点框** | ✅ `nodes[].type`（+ `properties["Node name for S&R"]`） | ✅ 顶层 key = 节点 id，`class_type` = 类型 |
| **标题** | ✅ 18/91 个节点带用户改的 `title` 字段；否则回落到 `type` | ⚠️ `_meta.title` **只有新数据有**（113/118/119 有，7 条老数据没有）→ **必须回落到 `class_type`** |
| **位置** | ✅ `pos: [x, y]`（浮点，坐标可为负） | ❌ **完全没有** |
| **尺寸** | ✅ `size: [w, h]`（140×46 ~ 7547×300） | ❌ 没有（只能按标题长度估） |
| **连线** | ✅ `links[]` **有两种写法，必须都认**：老写法数组 `[link_id, 源节点, 源槽位, 目标节点, 目标槽位, 类型]`（实测 `[124, 88, 0, 84, 0, "LATENT"]`；新写法对象 `{id, origin_id, origin_slot, …}` —— 见 `WorkflowConvert.kt:259-281`）。节点侧还有 `inputs[].link` / `outputs[].links` 可交叉验证 | ✅ `inputs` 里值为数组的项就是连接，语义是**"我这个输入来自哪个节点的第几个输出"**：`{"samples": ["88", 0]}` → 88→本节点。方向容易写反 |
| **参数** | ⚠️ `widgets_values` 一般是**只有位置、没有键名**的数组（实测 `["res_multistep"]`、`["max(5, round(a * 24)) + …"]`，61/91 个节点有）；少数自定义节点存成**对象**（那时有键名，`WorkflowConvert.kt:200-207`）。**离线无法把数组贴回参数名**，只有 `/object_info` 的输入声明顺序能还原（见 2.5） | ✅ 有键名 + 值（`inputs`），标量是参数、数组是连线 |
| **分组** | ✅ `groups[]`：`{"title":"Prompt","bounding":[-383,-523,530,910],"color":"#3f789e"}`；id 116 的 4 组是 Text to Video / Image to Video / First and Last Frame to Video / Reference to Video Group | ❌ 没有 |
| **执行顺序** | ✅ 每个节点有 `order`（实测 0、10、71…） | ⚠️ 没有，需要自己拓扑排序 |
| **禁用 / 旁路状态** | ✅ `mode`（id 116 里 30 个 0、**61 个 4**）；另有 `flags.collapsed`。语义有代码依据：`mode==2` = 静音、`mode==4` = 旁路（`WorkflowConvert.kt:129-142`） | ❌ 不存在（被静音/旁路的节点根本不会出现在 API 图里） |
| **备注 / 注释** | ⚠️ 只能按"第三方节点类型 + 大尺寸矩形"近似识别（`Label (rgthree)`）；`Note` / `MarkdownNote` 属于前端虚拟节点（`WorkflowConvert.kt:62-64`） | ❌ 没有 |
| **子图** | ❓ 当前样本里**没有** `definitions` 字段；ComfyUI 1.52 之后有子图概念，但我**没有真实样本**，形状未知 → 实现前必须再取一份样本确认，不许猜 | ❌ 没有 |
| **前端专用节点** | ✅ 会出现 `/object_info` 里查不到的类（`Reroute` / `SetNode` / `GetNode` / `Note` …，以及**无法等价转换**的 `Anything Everywhere`） | ❌ 不存在（前端在排队前就把它们消化掉了） |

> `mode` 的语义**有代码依据**：`WorkflowConvert.kt:129-142` 写明 `mode==2` = 静音（ComfyUI 自己也会摘掉）、
> `mode==4` = 旁路，并明确说"前端会把它接过去，规则与类型强相关，猜错代价太高 —— 如实拒绝"。
> 实测侧也吻合：id 116 有 4 组、4 个 `SamplerCustomAdvanced` 里 3 个 `mode:4`、
> 8 个 `VAELoader` 里 6 个 `mode:4`（只启用一组、其余旁路）。
> **对蓝图的含义**：`mode` 必须画出来（灰显/静音标记），而且**不许**在画布上把旁路节点"还原"成执行态。

### 3.3 所以：能画什么、不能画什么

**UI 格式 —— 可以画得和 ComfyUI 里几乎一样**：真实坐标、真实尺寸、真实连线、分组框、
旁路节点灰显、注释框按矩形画。节点输入的槽位类型（`inputs[].type`）也有，够给连线和槽位着色。
两处别漏：`links` 有数组/对象两种写法；会出现 `/object_info` 里查不到的**前端虚拟节点**
（`Reroute` 这类画成小中转点、`Note` 画成便签，不要当成普通节点框）。

**API 格式 —— 只能画"逻辑图"，画不出"原图"**：

- 能画：节点（`class_type` / `_meta.title`）、有向边（来自 `inputs` 里的数组）、参数键值、入口/出口节点。
- **画不出**：位置、尺寸、分组、旁路状态、注释框、`order`（要靠拓扑排序）。
- **注意**：API 图里的节点是"**真正被执行的那一份**"，与用户在 ComfyUI 里看到的**不是同一张图**。
  实测同一次运行：API 18 节点 vs UI 91 节点（差的 73 个里，61 个是旁路、6 个是注释框，
  剩下的差额我没有逐个核对，**不要依赖任何等式**）。这一点必须写进界面提示条。

### 3.4 API 格式怎么排（**这是猜测，不是原图**）

推荐 **最长路径分层 + 同层重心排序**，一次就够，不要上完整的 Sugiyama：

1. 建图：对每个节点，遍历 `inputs`，值是 `[id, slot]` 的数组就记一条 `id → 本节点` 的边。
2. 分层：`layer(n) = 0`（无上游），否则 `layer(n) = 1 + max(layer(上游))`。有环时（理论上不该有）
   用 Kahn 拓扑排序剩下的节点兜底放到最后一层，**不许死循环**。
3. 层内排序：按上游节点的平均 x（初始为 id 顺序）排一遍（barycenter 一轮），减少明显交叉。
4. 摆放：层间距 **260 px**，同层行距 **140 px**，节点统一 **180×64**（尺寸靠估算，标题按字符数截断）。
5. 备选（最省）：直接网格铺 4 列。最不容易出错，但连线会很乱 —— **不推荐**，除非 P2b 时间超了。

**验收上必须能看到**：界面上有一条明确的提示（例如"这份是 API 格式节点图，没有保存画布坐标，
位置是自动排的"），并且回归用例断言这条提示存在。

---

## 4. 候选方案对比

| | **A 结构化大纲** | **B CustomPainter + InteractiveViewer 画布** | **C 内嵌 litegraph.js / ComfyUI 前端** |
| --- | --- | --- | --- |
| 形态 | 可折叠的节点树：类型/标题 + 参数 + 上游列表；纯文本为主 | 节点框 + 连线 + 分组框，可缩放平移，接近 ComfyUI 观感 | 把 ComfyUI 的 JS 画布整个跑在应用里 |
| 像 ComfyUI 的程度 | 低 | **中高**（观感接近，但不能编辑） | 最高（就是它本人） |
| UI 格式 | 完全适用 | 完全适用（真实坐标） | 理论适用 |
| API 格式 | 完全适用（边来自 `inputs`） | 适用，但**位置靠猜** | 未验证 |
| 新增依赖 | 0 | 0（Flutter 自带；仓库已有先例 `lib/widgets/zoomable_image_view.dart`） | 需要 WebView / JS 引擎 + DOM；`pubspec.yaml` 目前只有 6 个运行期依赖 |
| 工作量 | 0.5 ~ 1 人日 | 3.5 ~ 5 人日 | 无法估（先把 ComfyUI 前端跑通 + 后端 API + 打包） |
| 主要风险 | 用户可能觉得"还是没展开成蓝图" | 大图性能；猜测布局误导 | 技术栈不通、双 UI 维护、发布包体积、定位冲突 |
| 结论 | **做（第一阶段）** | **做（第二阶段，限只读、限范围）** | **不做** |

### 4.1 A：只做结构化文本树 / 大纲（最省）

- **做什么**：把归一化后的图渲染成"节点一行/一卡"，每个节点显示 `class_type`（或 `title`）、
  参数（API 格式才显示，UI 格式只显示位置化的值或干脆不显示）、以及"上游：A → B → C"。
  可折叠、可搜索、可复制单个节点的参数。**必须 `ListView.builder` 懒构建**（`AGENTS.md:124-128`）。
- **优点**：风险极低、两种格式一视同仁、对超大图（上千节点）也扛得住、复用现有弹窗与 4 种状态。
- **缺点**：不是用户说的"蓝图"，观感上"还是文字"。
- **为什么仍然值得先做**：它是 B 的**前置依赖**（解析层完全共用），而且它自己就能解决
  "91 个节点到底跑了哪几个"这个真实痛点（用户库里 61/91 是旁路，现在完全看不出来）。
- **可选增强（P3，不在第一阶段）**：ComfyUI 在线时，复用 `ComfySubmitter.objectInfo()`
  （`ComfySubmitter.kt:90-125`）把 UI 格式的 `widgets_values` 贴回真实参数名，大纲就能显示
  `steps = 20` 这样的键值。**代价是"离线时显示不出来"**，所以必须做成"有就显示、没有就不显示"，
  绝不能因此把界面变成"必须有 ComfyUI 才能看工作流"。另外要复刻 `WorkflowConvert` 的失败模式
  （虚拟节点、控件值个数对不上）并如实标注 —— 不要自己再写一套猜法。

### 4.2 B：CustomPainter + InteractiveViewer 的画布蓝图（最接近 ComfyUI）

- **做什么**：
  - 一个 `InteractiveViewer`（有界视口）+ `CustomPaint`；节点与连线都用 Painter 直接画
    （不为每个节点建 widget：91 个的规模还能接受，但上千节点时 widget 重建成本会明显上来 ——
    **具体阈值实现时实测**，不要照抄这句话），连线用 `Path.cubicTo` 画贝塞尔。
  - UI 格式：用真实 `pos/size`，分组框按 `groups[].bounding` 画在底层，旁路节点（`mode:4`）降透明度。
  - API 格式：用 3.4 的分层布局，标注"猜测布局"。
  - **视口剔除**：只画与当前 `TransformationController` 视口相交的节点/连线。
- **优点**：满足用户诉求；仓库里已有同类交互的成功先例（`lib/widgets/zoomable_image_view.dart:41-52`
  的滚轮缩放 / 拖动平移约定、`:439` 的 `_MinimapPainter`），可以照抄交互习惯，用户不用学第二套。
- **缺点**：
  - 只能"看"，不能编辑 —— 但**这正是我们要的**（应用定位是只读捕获与查看）。
  - API 格式的布局是猜的（见 3.4）。
  - 大图性能需要专门处理（第 6 节）。
- **工作量拆解**：P2a 2~3 人日 + P2b 1 人日 + P2c 0.5~1 人日。

### 4.3 C：内嵌 / 桥接 litegraph.js 或 ComfyUI 前端 —— **不建议，基本不可行**

理由（按致命程度排序）：

1. **Flutter Windows 端没有 DOM。** ComfyUI 的前端是独立的 Web 应用
   （[Comfy-Org/ComfyUI_frontend](https://github.com/Comfy-Org/ComfyUI_frontend)，Vue 3 + litegraph 系画布），
   litegraph 是画在 `<canvas>` 上的 JS 库，只能跑在浏览器/WebView 里。本仓库 `pubspec.yaml`
   当前没有任何 WebView 依赖，引入 `webview_windows` 之类属于**新增一类运行期依赖**，
   还要把前端产物打进发布包 —— 与 `AGENTS.md` 第 7 节的发布包裁剪/体积约定直接冲突（要重做清单与自检）。
2. **它不是一个可以拿来即用的库。** ComfyUI 前端要连 ComfyUI 服务端的 `/object_info`、`/prompt`、
   `/queue`、`/history` 和 WebSocket 才能把工作流正确加载出来。README 里提到的 `loadApiJson`
   是前端内部行为，**不是对外契约** —— 我**没有验证过**"只给一份 JSON、不连服务端也能渲染"，
   不能拿它当方案依据。
3. **它解决不了核心问题。** agent/脚本提交的运行只有 API 格式（10/13 行数据），
   没有坐标，ComfyUI 前端自己也得自动布局 —— 用户看到的仍然不是"他当时在 ComfyUI 里看到的那张图"。
4. **代价与收益不成比例。** 两套 UI、两套升级节奏（本机 UI 工作流里记的 `frontendVersion` 是 1.52.7，
   还在动）、一个几百 KB 的 Web 产物，换来的只是一个"看"。而 B 能拿到其中大部分的观感。

> 如果你依然想验证 C，**唯一合理的最小实验**是：单独起一个 ComfyUI 前端 + 一份真实
> UI 格式工作流，确认"不连服务端能不能只渲染"—— 但这属于另开一个调研任务，
> **不该阻塞 A/B**，也不建议在这个 App 里落地。

---

## 5. 推荐路线（分阶段，含验收与回归测试写法）

> 通用约定（来自 `AGENTS.md:21-28`、`:124-135`、`:34-41`）：
> ① 回归一律是 widget 测试，**断言结构/数量/关系，不断言像素与截图**（全仓 0 处 golden）；
> ② 长列表一律懒构建；③ 交互类改动开发期要用 debug 版人工看一眼（`scripts\dev-app.ps1` + `r` 热重载）
> 并截图确认观感，但截图不进 CI；④ 同一事实不要写两条用例，老用例一条都不删
> （工作流弹窗的回归入口是 `test\workflow_viewer_test.dart`，`AGENTS.md:21-23`）。

### P0 解析层（0.5~1 人日）

**交付物**：`lib/core/workflow_graph.dart`（或 `lib/widgets/workflow/` 下的同类文件）——
`WorkflowGraph.fromJson(String raw)` 返回三态 + 归一化模型：

```text
WorkflowFormat { ui, api, unknown }
WorkflowGraph   { format, nodes[WorkflowNode], edges[WorkflowEdge], groups[WorkflowGroup], warnings[] }
WorkflowNode    { id, title, className, pos?, size?, mode, params, upstream[], downstream[] }
```

**验收标准**：
- UI 格式 → `format == ui`，节点数/连线数与 JSON 一致（用 id 116 的真实节选做夹具）；
- **`links` 的数组写法与对象写法都要能解析**（`WorkflowConvert.kt:259-281` 是判据来源），
  以及 `widgets_values` 的数组/对象两种形态（`:200-207`）；
- 节点里出现"前端虚拟节点"（`Reroute` / `Note` / `SetNode` / `GetNode` …）时**不报错、不丢节点**，
  标记成 `kind = frontendOnly`（清单见 `WorkflowConvert.kt:62-64`）；
- API 格式 → `format == api`，`pos == null`，边方向正确（`{"samples":["88",0]}` ⇒ `88 → 本节点`），
  标题回落顺序为 `_meta.title` → `class_type`；
- 非 JSON / 空对象 / 无关 JSON → `format == unknown`，**不抛异常**；
- `.json` 文本视图与复制行为**一字不变**（复用现有 9 条用例做回归）。
- ⚠️ 判定"是不是 UI 格式"要与后端**对齐**：后端 `WorkflowConvert.isUiWorkflow()` 的判据是
  `obj["nodes"] is JsonArray`（`WorkflowConvert.kt:70`），前端 `looksLikeApiGraph()` 是"顶层每个值都带
  `class_type`"（`workflow_viewer.dart:12-15`）。两者不是互补关系，别写出"前端说是 API、后端说是 UI"
  这种自相矛盾的结论 —— 解析层要给出**一个**三态判定并说明依据。

**回归怎么写**：
- 新增 `test/workflow_graph_test.dart`（纯 Dart 单测，不需要 pump widget）：
  - 夹具直接放进 `test/` 里（从库里真实数据脱敏后取节选，见第 8 节的导出办法），
    至少要有一份**多节点 + 多连线**的 UI 样本（现在的 `_uiWorkflowJson` 只有 1 个节点，不够用）；
  - 断言：`nodes.length`、`edges.length`、**每条边的方向**、入口节点集合、`mode == 4` 的节点数；
  - 断言：对 API 格式，`groups` 为空、所有 `pos` 为 null、`nodes.every((n) => n.title.isNotEmpty)`。

### P1 大纲视图（0.5~1 人日）

**交付物**：弹窗里增加"大纲"标签页（或把弹窗正文换成 `TabBar`：`大纲` / `JSON`）。

**验收标准**：
- 大纲用 `ListView.builder`（懒构建）；
- UI 格式**不显示参数名**（数组形式的 `widgets_values` 离线还原不出来；如果拿到的是**对象**形式，
  自带键名，那种可以直接显示）；API 格式显示 `键 = 值`；
- 每个节点能看到"上游 N 个"并能展开；
- 4 种状态（加载/错误/没存过/正文）与复制按钮行为不变。

**回归怎么写**（追加到 `test/workflow_viewer_test.dart`，与现有 9 条并列）：
- `find.text('KSampler')` 只出现一次（大纲里一项）；`_apiGraphJson` 下断言出现
  `CheckpointLoaderSimple → KSampler` 的连接文案；
- **懒构建断言**（这条是防退化的关键，对应 `AGENTS.md:124-128`）：
  `tester.widget<ListView>(find.byType(ListView)).childrenDelegate is SliverChildBuilderDelegate`；
- 用一份 91 节点的夹具断言"首帧只构建视口内的项"（`find.byType(...)` 命中数 < 节点总数），
  而不是断言具体是哪几项；
- **不要**给同一个事实写两条用例（`AGENTS.md:24-28`），例如"大纲里没有 widgets_values"和
  "UI 格式不显示参数"就是同一条。

### P2 蓝图画布（3.5~5 人日）

拆成三块，**可以分批交付**：
- **P2a（2~3 人日）UI 格式真实坐标画布**：盒子 + 标题 + 连线 + 分组框 + 旁路灰显 + 缩放平移 + 视口剔除。
- **P2b（1 人日）API 格式分层布局**：3.4 的算法 + "猜测布局"提示条。
- **P2c（0.5~1 人日）大图降级**：节点数超阈值时自动退到大纲视图 / 只画边不画参数（LOD）/ 关掉动画。

**验收标准**：
- 用 id 116 的真实样本（91 节点 / 95 连线 / 4 组）：首帧**同一帧内只绘制视口内**的节点（可断言的量）；
- 4 个分组框都画出来了，标题文字可见；
- `mode:4` 的 61 个节点视觉上可区分（例如断言的 painter 入参里 `bypassed == true`）；
- API 格式：布局是**确定性**的（同一输入两次得到相同坐标），且满足分层关系
  （`CLIPLoader` 的层 < `SamplerCustomAdvanced` 的层）；
- 缩放/平移后节点仍可命中（`tester.tap` 一次即可，不用测像素）；
- 超阈值（建议 300 节点，可配置）时自动降级并给出提示，**不卡死**。

**回归怎么写**：
- 新建 `test/workflow_canvas_test.dart`：把"画了什么"变成**可注入的回调/统计数据**
  （例如 painter 收一个 `WorkflowPaintStats{visitedNodes, drawnNodes, drawnEdges, bypassed}`），
  测试断言 `drawnNodes < 节点总数` 且 `drawnNodes > 0`（视口剔除生效）；
- **不要断言具体像素坐标**（一改 padding 就红），要断言**关系**（层序、是否同层、边是否存在）；
- **不要写 golden**（仓库现在 0 处，且渲染跨机器不稳定）。

### P3（可选，1~2 人日）

节点搜索 / 定位、"点击节点看全部参数"、导出蓝图 PNG。**等 P1/P2 上线有反馈后再定，
现在不排期** —— 尤其"导出 PNG"收益最低。

---

## 6. 性能与风险

### 6.1 超大数据

- 现状：库内最大 **114 KB / 91 节点**（实测）。400 KB 的显示上限（`workflow_viewer.dart:22-26`）
  目前是防御性的。
- 但**不能只按当前数据设计**：本机这份 91 节点的样本已经是 4 条完整流水线拼在一起了，
  更复杂的图（多分支、多 ControlNet、Wan/Flux 长链）会更大 —— 我**没有生态统计数据**，
  这里只按"当前样本外推、留出余量"来设计，不给"常见多少节点"的结论。
- 对策：
  1. 解析与归一化放在 `Future` 里；**实现时用 `Stopwatch` 量一次 114 KB / 400 KB 的
     `jsonDecode` + 归一化耗时，再决定要不要上 `compute`/Isolate** ——
     这个数字我**没有实测**，不在这里拍脑袋（体量上看着不大，但"看着不大"不是依据）；
  2. 画布只画视口内的节点与连线（剔除）；
  3. 节点数 > 阈值（建议 300，可配置）时自动降级到大纲视图，或只画框与线不画参数；
  4. 现有 400 KB 的 JSON 文本上限**保留不动**。

### 6.2 节点上千 / 大节点

- 实测里已经出现 **7547×300 的 `Label (rgthree)` 注释框**，比普通节点（140×46）大两个数量级。
  直接按原尺寸画会出现"缩到最小才看得全"的体验；建议对注释框单独处理（画成半透明底板 + 左上角标题，
  并按 3.1 的实测给一个默认"适应窗口"的缩放）。
- 首次进入时的默认视角：优先用 `extra.ds.scale/offset`（3 份 UI 工作流都有），
  **取不到就退回"包围盒 fit"**。注意 `extra.ds` 不是文档化字段（我没在 ComfyUI 官方文档里核过），
  只能当"提示"，绝不能当依赖。

### 6.3 拖拽 / 缩放 / 滚动条

- `InteractiveViewer` 在 Windows 桌面上的滚轮缩放 + 拖动平移，本仓库已经验证过
  （`lib/widgets/zoomable_image_view.dart:41-52`，回归用例 `test/zoomable_image_test.dart`）。
- **必须避开的坑**（`docs/pitfalls.md:53`）：`InteractiveViewer` **不能**再套在外层
  `SingleChildScrollView` 里，否则"放大变成整页滚动"而不是画面平移。所以画布必须是**有界视口**，
  不能塞进现有弹窗的滚动正文里。
- 蓝图页若用整屏路由，就**不会有滚动条问题**；这也是推荐整屏而不是塞进 `AlertDialog` 的原因之一。
- 若最终仍放进弹窗：注意 `AlertDialog` 正文的尺寸 clamp（`workflow_viewer.dart:183-185`
  最大只有 760×460），对 9600×2400 的画布意味着初始缩放约 8%，基本没法用。

### 6.4 与现有弹窗的取舍

- 现有弹窗承担 4 种状态 + 复制全文（`:212-311`、`:164-177`），**这些必须原样保留**：
  用户复制工作流拖回 ComfyUI 是当前最核心的用途（`README.md:411-415`）。
- **建议的单向依赖**：`弹窗（取数 + 4 种状态 + JSON 文本 + 复制）` ──「看蓝图」──►
  `整屏画布页（只接收已经拿到的 raw 字符串）`。
  - 好处：画布页**不需要再取一次数**，也**不用复刻 4 种状态**（它只可能拿到非空原文）；
  - `showWorkflowDialog` / `WorkflowButton` 的签名不变 ⇒ `prompt_detail_page.dart:169-173`、
    `media_detail_page.dart:644-648` 一行都不用改，`:281` / `:299` 两条接线用例继续有效。
- **不要**把蓝图做成弹窗的默认视图：万一蓝图解析出错，用户连复制原文的入口都没了。
  默认视图保持 JSON 文本，蓝图是**新增入口** —— 这条也保证了现有 9 条用例全绿。
- 反向入口（蓝图页里的「看 JSON」）**可做可不做**：弹窗还开着，关掉就能看。建议 P2 先不做，
  少一个返回态的坑。

### 6.5 其他风险

| 风险 | 说明 | 处置 |
| --- | --- | --- |
| 误以为 API 图的布局是原图 | 用户会拿两张图对比然后"发现不对" | 提示条写明"位置是自动排的"；节点数不一致时也说明（API 图只含真正执行的节点） |
| 从 `widgets_values` 猜参数名 | 数组形式**没有键名**（实测 `["res_multistep"]`），离线猜一定错；只有对象形式的自定义节点自带键名 | 硬约束：UI 格式不展示参数名（要展示必须走 `/object_info`，属 P3） |
| 边方向写反 | API 格式的 `inputs` 数组语义是"我的输入来自谁" | 单测专门钉一条边的方向（P0 验收） |
| 子图（`definitions`） | 当前 3 份样本都没有，形状未知 | 遇到时**回落 JSON 文本视图并提示"暂不支持子图"**，不许猜 |
| `_meta.title` 缺失 | 7/10 份 API 数据没有 | 回落 `class_type`，单测覆盖 |
| 老数据回归 | 现有 9 条用例钉住的是文本行为 | 不改默认视图即全绿；蓝图自己的用例独立成文件 |

---

## 7. 明确**不建议现在做**的部分

| 不做的事 | 理由 |
| --- | --- |
| **蓝图上的编辑 / 拖拽 / 连线改写回 ComfyUI** | 与本 App"只读捕获与查看"的定位直接冲突；改图要写回 UI 格式、提交还要转成 API 格式，而"界面格式 → API 格式"这条路**已经有实现**（`WorkflowConvert.kt` + `ComfySubmitter.objectInfo()`，见 2.5），在画布上再实现一遍就是两套逻辑、两套失败模式。 |
| **在蓝图里直接"跑一次"（提交）** | 提交这条路已有 `comfy_submit` + `ComfySubmitter.WorkflowEdit.applyOverrides`（`AGENTS.md` 10.1）以及上一条的在线转换链路，蓝图再长一条就是两套提交逻辑。 |
| **内嵌 litegraph.js / ComfyUI 前端（C）** | 见 4.3：Flutter Windows 没有 DOM、需要 WebView 类新依赖、要打进发布包、且解决不了 API 格式没有坐标的问题。 |
| **子图（`definitions`）展开** | 库里 3 份 UI 工作流**都没有**这个字段，没有真实样本，做了只能猜。 |
| **把 `Label (rgthree)` 之类第三方注释节点语义化**（当成真正的分组标题） | 第三方节点形状不稳定（同一个类型 300px 到 7547px 宽都有）；第一阶段只按"矩形 + 文字"画就够。 |
| **离线反推 `widgets_values` 的参数名** | 明确禁止。要反推只有 `/object_info` 一条路（`WorkflowConvert.kt:20-22` 就是这么说的），且属于 P3 可选增强（见 4.1）。 |
| **golden / 截图回归** | 全仓 0 处 golden；`AGENTS.md:24-28` 的取舍标准是"删了以后出错还能不能被测试抓住"，观感由开发期 debug 版人工确认（`AGENTS.md:34-41`）。跨机器渲染也不稳定。 |
| **给全库工作流做后台预解析 / 缓存** | 库内只有 13 份、最大 114 KB，"按需拉取 + 即用即解析"完全够（`CaptureRoutes.kt:105` 的注释也说了"按需拉取，避免列表接口变重"）。加缓存要有失效策略，收益接近 0。 |
| **导出蓝图 PNG / 分享图** | 好看但收益最低，且会引入渲染尺寸、字体、DPI 一堆问题。放 P3，且默认不做。 |
| **改 `workflow_json` 的存储格式或新增一列存"布局缓存"** | 没必要：布局可以从原文实时算（UI 格式本来就有坐标，API 格式是猜的、缓存一份猜的结果只会更难解释）。 |

---

## 8. 附录：调研用的查询与文件索引

### 8.1 复现本调研的只读查询

```powershell
$mysql = 'D:\tools\mysql\mysql-8.4.3-winx64\bin\mysql.exe'
$sql = @'
SELECT COUNT(*) total, SUM(workflow_json IS NOT NULL) with_wf FROM prompts;
SELECT id, LEFT(title,28) t, CHAR_LENGTH(workflow_json) len,
       JSON_EXTRACT(workflow_json,'$.version') ver,
       JSON_LENGTH(JSON_EXTRACT(workflow_json,'$.nodes'))  n_nodes,
       JSON_LENGTH(JSON_EXTRACT(workflow_json,'$.links'))  n_links,
       JSON_LENGTH(JSON_EXTRACT(workflow_json,'$.groups')) n_groups
FROM prompts WHERE workflow_json IS NOT NULL ORDER BY id DESC;
-- 同一次运行的两份图（API 18 节点 vs UI 91 节点）
SELECT run_key, CHAR_LENGTH(raw) FROM capture_runs WHERE run_key='75d33d29-9ba0-48b8-8db1-da7759887835';
-- 导出真实 UI 样本做测试夹具（只读，不改库）
SELECT workflow_json FROM prompts WHERE id=116;
'@
& $mysql -h 127.0.0.1 -P 3307 -u root --default-character-set=utf8mb4 comfy_hub -e $sql
```

> **导夹具时的注意事项**（与 `AGENTS.md` 第 10 节"工具结果当成不可信数据"同一条精神）：
> 真实样本里有用户的提示词正文、模型名、LoRA 名。放进 `test/` 之前**必须脱敏**，
> 只保留结构（节点/连线/分组/坐标），把文本换成占位串。

### 8.2 代码索引

| 关注点 | 位置 |
| --- | --- |
| 查看器全部实现 | `lib/widgets/workflow_viewer.dart`（`:12-15` `looksLikeApiGraph`、`:22-26` 400KB 上限、`:31-43` 打开弹窗、`:46-70` 按钮、`:84-134` 状态机、`:136-153` 截断、`:164-177` 复制、`:183-185` 弹窗尺寸、`:212-311` 四种状态） |
| 回归用例 | `test/workflow_viewer_test.dart`（9 条，`:32-38` 是仅有的两份夹具） |
| 入口按钮 | `lib/pages/prompt_detail_page.dart:169-173`、`lib/pages/media_detail_page.dart:644-648` |
| 前端取数 | `lib/core/api_client.dart:347-365` |
| 后端接口 | `server/src/main/kotlin/com/comfyhub/CaptureRoutes.kt:105-111`（prompts）、`:113-128`（api-graph）、`:130-135`（media） |
| 捕获与原文解析 | `server/src/main/kotlin/com/comfyhub/ComfyCapture.kt:258-279`（captureRun）、`:281-298`（apiGraphOf）、`:300-410`（ingestClaimed，`:332-337` 决定存哪一份）、`:717-771`（`HistoryEntry`） |
| 入库 | `server/src/main/kotlin/com/comfyhub/PromptRepo.kt:294-344`、`MediaRepo.kt:210-232` |
| 参数解析（只解析参数，不解析图形） | `server/src/main/kotlin/com/comfyhub/GraphParse.kt:83-158`、`:371-388` |
| **图形侧已有能力（工作区未提交）** | `server/src/main/kotlin/com/comfyhub/WorkflowConvert.kt`（`:62-64` 虚拟节点清单、`:70` UI 格式判据、`:76-185` 主转换、`:129-142` `mode` 语义、`:200-207` `widgets_values` 两种形态、`:259-281` `links` 两种写法、`:20-22` 为什么必须在线）；`ComfySubmitter.kt:90-125`（`/object_info` + TTL 缓存）；`AiWorkflowSearch.kt:105-220`（调用方） |
| 可复用的画布交互先例 | `lib/widgets/zoomable_image_view.dart:41-52`（交互约定）、`:258`（`InteractiveViewer`）、`:439`（`_MinimapPainter`），用例 `test/zoomable_image_test.dart` |
| 仓库约定 | `AGENTS.md:7-42`（第 1 节：只改前端用 debug + 热重载、测试的取舍标准）、`:105-135`（第 6 节：懒构建、固定行高、`SliverFixedExtentList`） |
| 踩过的坑 | `docs/pitfalls.md:32`（`SelectableText` 的高度陷阱）、`:50-52`（工作流格式与 `hasWorkflow` 三个坑）、`:53`（`InteractiveViewer` 套滚动视图导致"缩放变整页滚动"） |
| 背景文档 | `README.md:312`（工作流存两列）、`:376-377`（两个接口）、`:414-415`（两份 JSON 不能互相替代）、`:970`（该测试的覆盖面） |
| 需求出处 | `docs/bug-and-suggestion-9.18.md:26` |
