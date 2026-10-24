# 2026-10-18 用户报的 5 个问题 + 2 条交互建议（**只分析，未动手**）

> 用户原话："先分析，不执行。"
> 本文只做**现象 → 现场证据 → 根因 → 建议方案 → 防线与风险**。
> 动手前按 AGENTS.md 的两条底线先落基线（`git commit`），代码改动只走 `edit` / `write`。

现场证据一律带 `文件:行号`。凡是**推断**而不是**读到**的，文中明确写"推断"。

---

## 0.2 进度（2026-10-18 第二轮反馈）

用户看过界面之后又提了四条，都已落地：

| 项 | 状态 | 落点 / 防线 |
| --- | --- | --- |
| 蓝图参数行改成「名字 \| 值…」两列，点击在**值那一格**就地编辑 | ✅ | 新增 `blueprintWidgetLabelRect` / `blueprintWidgetValueRect`（与绘制共用一份几何）；面板同一套两列布局（`kBlueprintParamLabelWidth`）。用例：`workflow_blueprint_test.dart` 的「参数行是"名字 \| 值"两列」 |
| 面板里多行参数**改了不落库**（旧 bug） | ✅ | 多行 `TextField` 的 `onFieldSubmitted` 永不触发 —— 现在"点到别处"也提交（`_PanelWidgetField` 自己持有 controller） |
| markdown 文案在有渲染能力的地方**没有渲染** | ✅ | `markdown.dart` 新增本仓库约定的 `##强调##`（与 `**粗体**` 同义，成对才认）；蓝图横幅 / 体检条目 / 节点问题行改用 `MarkdownText`。用例：`markdown_test.dart` 4 例 + 「体检里的 markdown 真的被渲染」 |
| 把库里**已存的**图片重新自动整理 | ✅ | `collection::regroup_auto()`（只解散 `origin='auto'`，manual 与 `collection_opt_out` 一个都不动）+ `POST /api/media/collections/regroup` + 画廊工具栏按钮。用例：`collection/tests.rs` 3 例 + `media_collection_api_test.dart` 1 例 |
| 点集合格子 = **展开列出成员**（不再直接跳封面详情） | ✅ | `_folderPanel`：**非模态**浮层（模态 `ModalBarrier` 会把底下的落点挡在命中测试之外，那样"把成员拖出文件夹"根本收不到落点）。用例：「点集合格子 = 展开列出成员」 |
| 桌面端**直接左键拖动** | ✅ | `_mouseFirst`：桌面即时 `Draggable`（滚轮翻页，不吃滚动）、触屏仍是长按拖。用例：桌面两条 + 触屏一条（后者钉子钉住"别把滚动抢手势的路径改坏"） |
| 拖出集合 / 拖到别的格子完成集合转移 | ✅ | `_dropOut`（拖到"移出集合"落点）+ `_dropOnto`（拖到别的格子 = 并进 or 新建一组）。⚠ `_expandOne` 不再只看 `_picked` —— 桌面直接拖时没有勾选过，只看它会把"动整组"退化成"只动封面" |

⚠ 一处已知的渲染边界（子代理核实时发现并如实报告）：`markdown.dart` **本来就实现标题**，
所以**行首且带空格**的 `## 模拟用户行为 ##` 会先被块级标题规则吃掉。
仓库主流写法 `##模拟用户行为##`（`##` 后无空格）不受影响、已渲染成粗体；
要连带空格那种也当强调，得改块级标题判定并同步 `test/ai_tools_ui_test.dart`，**没有擅自改**。

---

## 0.1 进度（2026-10-18 第一轮，全部收口）

用户拍板（"整理/多选合成一个模式"、"集合格子拖动动整组"、"手动集合不混 kind"、
"用户一碰就归他"）之后的落地情况。**目标里的每一项都已完成并有用例**，
只有一项"真拖拽进窗口"是本次目标之外的（见下表最后一行）。

**验证**：`flutter analyze` 干净；`flutter test` **509 例**；`cargo test` **723 + 154 例**；
`flutter build windows --debug` 通过（含自建原生通道的编译链接）。

| 项 | 状态 | 落点 / 防线 |
| --- | --- | --- |
| 右键删除只删一张（删一张 / 删整组 / 删选中） | ✅ | `gallery_page.dart` 的 `_showThumbMenu` + `_deleteMediaIds` / `_collectionMemberIds` |
| 集合格子**按整组**计入选中 | ✅ | `_picked` 记 `size` + `collection`；`test/gallery_group_selection_test.dart`（3 例） |
| 蓝图"跑到右上角" | ✅ **两半都修了** | ① 适配改用**画布自己**的视口（`_canvasArea`）；② `_fitToView` 的平移参照系用错（见 §2.8）；`workflow_blueprint_test.dart` 加了"精确居中"断言 |
| 蓝图 LOD 在缩小时**整个失效** | ✅ | `Matrix4.getMaxScaleOnAxis()` 把 Z=1 也算进去 ⇒ 缩小永远回 1.0；改用新增的 `blueprintScaleOf()`（见 §2.8） |
| 蓝图加载慢 | ✅ 主要一半 | 新增 `server-rs/src/workflow/object_info_cache.rs`（60s TTL、只缓存成功、按地址失效、`Arc` 零拷贝）+ 超时 20s→8s + 启动预热 |
| 蓝图刺眼 | ✅ | `BlueprintPalette`（深浅两套）+ 语义色只上边框/标题条 + 画布自绘底色与网格；`workflow_blueprint_test.dart` 的「配色」组（5 例） |
| 参数中文溢出 / 省略号永不触发 | ✅ | `_text` 改成 `layout(maxWidth:)`，删掉 `0.62em` 估算 |
| 会话图片跳浏览器 | ✅ | 新增 `showImagePreview()`；`canPreviewInline()` 按 modality / 扩展名分派；`test/ai_attachment_preview_test.dart`（3 例） |
| 空提示词被并成一组 | ✅ | `collection.rs` 新增 `endpoint_for()`（两档都算"不进组"）+ 2 条用例 |
| 手动集合：**移出 / 移进**（后端 + 界面） | ✅ | schema 8→9：`media_collections.title` + `media_assets.collection_opt_out`；`collection.rs` 的 `detach()` / `move_into()`（kind 不一致 → 400）；`PATCH /api/media/{id}` 加 `collectionId` / `clearCollection`（与 `promptId`/`clearPrompt` 同形）；画廊右键「拆开这一组」+ 多选栏「移出集合」+ 详情页「移出集合」。用例：`collection/tests.rs` 4 例 + `infra/db/tests.rs` 老库补列 + `test/media_collection_api_test.dart` 4 例 |
| 手动集合：**新建 / 列集合 / 选目标**（"移到集合…"） | ✅ | `POST /api/media/collections` + `GET /api/media/collections`；`create_manual()` / `list()`；多选栏「移到集合…」→ 选择器（装不下的目标置灰并说明原因）+ 当场新建。用例：`collection/tests.rs` 5 例 + `test/media_collection_api_test.dart` 4 例 |
| 蓝图**面板折叠** | ✅ | 工具栏那颗开关；折叠后画布变宽且**不抢视角**。用例：「面板能折叠…」 |
| 蓝图**画布上直接改参数** | ✅ | `blueprintWidgetRowRect` / `hitTestWidgetRow`（与绘制**共用一份几何**）+ 屏幕坐标浮层 `_InlineWidgetEditor`。用例：「参数行的几何只有一份…」+「点画布上的参数行 = 就地改…」 |
| **移进集合**在界面上的入口 | ✅ | 见上（选择器） |
| **复制文件本身**（Windows） | ✅ | 自建原生通道 `comfyhub/clipboard`（`windows/runner/clipboard_channel.{h,cpp}`）：`CF_HDROP` + 纯文本兜底 + `Preferred DropEffect=复制`；Dart 侧 `lib/core/file_clipboard.dart`；后端新增 `GET /api/media/{id}/path`（**路径只由后端给**）。入口：详情页「更多」与画面右键。⚠ 实机观感未验证（见下） |
| **Ctrl+V 粘贴附件** | ✅ | 输入框外挂一个 `Focus(onKeyEvent:)`（比 `DefaultTextEditingShortcuts` 更靠近 TextField，所以先拿到事件）：剪贴板有文件就吃成附件，**没有就把这次粘贴自己还回去**（不然用户会"什么都粘不上"）。**2026-10-24 补上截图那一半**（用户："允许我在 AI 工作台 Ctrl+V 粘贴附件"）：原生通道 `readImage` 按 `PNG` → `CF_DIBV5` → `CF_DIB` → `CF_BITMAP` 读位图，Dart 把 BMP 转 PNG 后**按字节上传**（同一张 `/api/ai/attachments`，不落临时文件）。用例 `test/file_clipboard_test.dart` 11 例 + `test/ai_attachment_test.dart` 的三条 Ctrl+V 界面用例（粘截图 / 粘文件 / 粘文本） |
| 集合格子**拖动**（整理模式内长按拖） | ✅ | 模式外长按 = 进模式，**模式内长按拖 = 移动**（与 iOS/Android 同一套肌肉记忆，也因此不与 `GridView` 的滚动抢手势）。拖到有集合的格子 = 并进去；拖到没集合的格子 = **当场新建一组**；拖的是集合格子则**整组一起过去**。用例：`gallery_group_selection_test.dart` 的「拖到一起成一组」「拖到没有集合的一格上」，后者断言新建的组是 `{1,2,3,4}`（**整组**，不是只动封面） |
| 真拖拽（把文件从资源管理器拖进窗口） | ⬜ 未做（**不在本次目标内**） | 需要 `DragAcceptFiles` + 子类化 `WndProc` 收 `WM_DROPFILES`。`ai_home_page.dart:2993` / `gallery_page.dart:346` 那两句"拖进来"的文案现在**还是假的**，建议要么做要么改掉 |

---

## 0. 总览

| # | 现象 | 一句话根因 | 改动面 | 风险 |
| --- | --- | --- | --- | --- |
| 1 | 生成的内容自动归类到**错误的集合** | 判定输入只有「kind + 正/负提示词」；且**正提示词为空**的产物会全部并进同一个集合；再叠加**没有任何手动纠错入口**（`Origin::Manual` 从未被写入） | 后端 `media/collection.rs` + 新端点 + 表结构 + 前端画廊/详情 | **高**（要动判定规则 = `RULE_VERSION`、要动 schema 版本、`backfill` 会把"移出"吸回来） |
| 2 | 蓝图**对比度过高**、刺眼 | 画布配色是**一套写死的浅色系**（节点底 `#F3F5F7` + 近黑正文），而 App **默认深色主题** ⇒ 深底上压一片白，估算量级 **≈15~17:1** | 仅 `lib/widgets/workflow_blueprint_painter.dart` + 页面传参 | 低（纯前端、无数据） |
| 2b | 节点**参数显示**不清楚 | 画笔自定的行高（26/18/16）与布局用的真实节点高度**不是一套**；`_text` 的截断按 `0.62em` 估宽，中文低估 ⇒ 溢出 | 同 2，加 `lib/core/workflow_blueprint.dart` | 低 |
| 2c | 编辑只能去右侧/下侧面板，面板**不能折叠** | 面板固定 `width: 380`（宽屏）/ `maxHeight*0.42`（窄屏），无折叠开关；画布 `onTapUp` 只做**节点级**命中 | `workflow_blueprint_page.dart` | 中（手势与 `InteractiveViewer` 冲突，见 §2.6） |
| 3 | 点会话里的图片**跳到浏览器** | `_MessageAttachment.onTap` 写死 `_openAttachment` → `launchUrl(externalApplication)`，没有按模态分派；应用内已有 `ZoomableImageView` 但聊天页**没 import 它** | `lib/pages/ai_home_page.dart`（3 处） | 低 |
| 4 | **Ctrl+V 粘不了附件** | 输入框的 `Shortcuts` **只绑了 Enter**；全仓库 **0 处 `Clipboard.getData`**；而且 Flutter 内置剪贴板**只有文本**，图片/文件本来就拿不到 | 纯前端 + `win32` FFI（不需要原生） | 中（要引 `win32` 或写原生） |
| 5 | 应用内**不能复制文件本身** | 现有复制全是 `Clipboard.setData(text:)` ⇒ Windows 只写 `CF_UNICODETEXT`，资源管理器里粘不出文件；仓库也**没有直接依赖**任何能写自定义剪贴板格式的包 | 纯前端 + `win32` FFI（**推荐**）或原生通道 | 中（同一个依赖决策，见 §6） |

**一句话**：1 是"判定太窄 + 没有手动入口 + 回填会反悔"；2 是"画布只按一种主题配了色"；
3 是"应用内查看器早就有了，只是没接上"；4/5 是**同一件事的两面**——
仓库没有任何能读写自定义剪贴板格式（`CF_HDROP`）的能力。

---

## 1. 生成的内容自动归类到「错误的集合」

### 1.1 现在的判定是什么

判定**只有一份**，在后端：`server-rs/src/media/collection.rs`。

- key 的组成（`collection.rs:307-320`）：

  ```
  group_key = kind ⊗ 归一化(正提示词) ⊗ 归一化(负提示词)
  ```

  `⊗` 是 `\u{1F}`，片段用**长度前缀**拼，避免歧义（`collection.rs:286-301`）。
- **seed / steps / cfg / 采样器 / 底模 一概不进 key** —— 文件头 `collection.rs:1-43` 写了理由，并且有测试把这条钉死：
  `server-rs/src/media/collection/tests.rs:354` `采样器与底模不同也会并组_这是刻意取舍`。
- 入库时挂组：`assign`（`collection.rs:454-459`）→ `attach`（`collection.rs:410-449`），
  优先挂到**最近建的 `auto` 集合**（`ORDER BY (origin = 'auto') DESC, id DESC`）。
- 存量回填：`backfill`（`collection.rs:469-499`），**只动 `collection_id IS NULL` 的行**；
  启动时跑一趟（`server-rs/src/main.rs:361-420`）。
- 前端**不做任何判定**，只读 `collectionId` / `collectionSize` / `collectionPreview`
  （`lib/models/models.dart:410-421`、`lib/pages/gallery_page.dart:378-380`）。

### 1.2 用户为什么看到「分错了」——三个成因，性质不同

**成因 ①（真缺口，无防线）：正提示词为空的产物会被并成同一个集合。**

`endpoint_of`（`collection.rs:375-403`）的判空**只判"有没有关联提示词"**：

```rust
if prompt_id.is_none() { return Ok(None); }          // ← 只挡了"没关联"
Ok(Some(Endpoint::new(&kind, pos.unwrap_or_default(), neg…)))  // ← 正文可能是空串
```

而捕获入库时**每次运行都会插一条 prompts 行**，`parsed.positive` 原样写入、**不判空**
（`server-rs/src/comfy/capture_ingest.rs:345-372`，第 354 行就是 `parsed.positive`）。
`Parsed.positive` 的默认值就是空串（`server-rs/src/workflow/graph_parse.rs:41,63`），
靠一条 `first_non_blank` 兜底链去找（`graph_parse.rs:270-296`）——**兜不到就是空**。

⇒ 所有"解析不出正向提示词"的运行（自建工作流、非标准文本节点、特殊图生图链路……）
会全部落进同一个 `IMAGE ⊗ ∅` 集合，点进去左右翻一堆互不相干的图。

这条正好**重演**了 2026-10-11 那次事故（21 张手动导入图被并成一组，见 `collection.rs:27-29`），
只是当时的修法（`Assigned::Orphan`）只挡住了 `prompt_id IS NULL` 那一半。
**用例里没有覆盖"有关联但正文为空"这一档**（`media/collection/tests.rs` 的 60 个断言里没有它）。

**成因 ②（刻意取舍，但用户不认）：换了底模 / 采样器 / 尺寸 / 步数，只要提示词一样就并组。**

这是设计上写明的取舍（`collection.rs:22-26`、`tests.rs:354`）。用户感知却是"这两张明明是两回事"。
不评价对错，但这是"分错了"投诉的**主要来源**。

**成因 ③（同类）：归一化把它们认为"说的是同一样东西"的写法并到一起。**

`normalize`（`collection.rs:112-121`）削平大小写 / 空白 / 顺序 / 去重 / **带数字权重的括号**：
`(masterpiece:1.2), 1girl` 与 `masterpiece, (1girl:1.1)` 判同组。裸括号**刻意不削**（`collection.rs:109-111`）。

### 1.3 真正缺的是「手动入口」——现在一个都没有

| 该有的 | 现状 |
| --- | --- |
| 表结构 | **有**：`media_collections.origin CHECK IN ('auto','manual')`（`db/schema.sqlite.sql:102-110`） |
| 写 `manual` 的代码 | **没有**。全仓库 `Origin::Manual` 只出现在 `collection.rs:351`（`as_str`）与一个测试里（`tests.rs:500`）——**生产路径从未写过** |
| 改一条产物 `collection_id` 的 API | **没有**。`PATCH /api/media/{id}` 的请求体 `MediaUpdate`（`server-rs/src/web/inputs.rs:135-153`）只有 `promptId / clearPrompt / title / notes / favorite / source / nsfw` |
| 集合本身的端点 | **没有**。`/api/media…` 那些路由里没有 `/api/collections`（`server-rs/src/web/routes.rs:219-225`、`write_routes.rs:50`） |
| 集合的名字 | **没有**：`media_collections` 只有 `group_key / kind / origin / rule_version / created_at / updated_at`，**没有 title 列** |
| 前端多选 | **已经有**：长按进多选、全选本页、底部 `_SelectionBar`（`lib/pages/gallery_page.dart:23-24, 40-42, 66-104, 116-123, 652-690`） |
| 前端批量动作 | 已有 **关联提示词 / 收藏 / 删除**（`gallery_page.dart:528-561`）——**没有"移动到集合"** |
| 详情页集合内切换 | **已经有**（左右箭头 + ←/→ 快捷键）：`lib/pages/media_detail_page.dart:99-141, 159-163` |

⇒ 结论：**"拖进 / 拖出集合 + 长按多选批量改集合"是一件"接线活"，不是从零造**。
数据库、`origin` 语义、`backfill` 的"不动手动分组"纪律（`collection.rs:463-467`）都已经预留好了。

### 1.4 建议（按手机系统给 App 分组的那套来）

分三步，每步都能单独交付：

**第 1 步：先堵漏（不依赖 UI）** —— 把成因 ① 修掉。
`endpoint_of` 里，当**正、负提示词都归一化为空**时返回 `None`（= 按孤儿处理，不进组、不建集合）。
判据与 `Assigned` 的注释同源，`normalize` 已经现成。
⚠ 这会让一部分存量集合变空 —— `prune_all_empty`（`collection.rs:520-526`）已经在启动时跑，能收尾。
⚠ 若判定规则真的变了，必须 `RULE_VERSION += 1`（`collection.rs:50-54`）让回填重算；
但**"空提示词"这条更像 bug fix 而不是规则变更**，是否 bump 需要拍板（见 §6 待确认）。

**第 2 步：后端开三个口子**

- `PATCH /api/media/{id}` 的 `MediaUpdate` 增加 `collectionId: Option<i64>` +
  `clearCollection: bool`（**照抄 `clearPrompt` 那套语义**：`null` = 这次不动这一列，
  `clearCollection` = 明确踢出集合）。
- `POST /api/media/collections`（建一个手动集合，体里带 `title` 与首批 `mediaIds`）与
  `POST /api/media/collections/{id}/members`（加/减成员）——或者一个更省事的形状：
  `PATCH /api/media/{id}` 只认"加/减"，前端多选循环调它（**与现有 `_bulkLink` 的做法一致**，
  见 `gallery_page.dart:533-535`，那条路就是逐个 `updateMedia`）。
- 建手动集合时 `group_key` 不能空（表上是 `NOT NULL`）⇒ 得生成一个
  `manual:<uuid>` 之类的 key，并且**"名字"需要新列**（`media_collections.title`）。
  ⇒ 这是一次 **schema 版本 +1**（当前 7，见 `server-rs/src/infra/db.rs:77-110`），
  按 AGENTS §7 的规矩：`db/schema.sqlite.sql` + `db/migrate.sql` + `open_or_create` 的补列逻辑一起改，
  并且要补一条 `infra/db/tests.rs` 那种"表与列一起被治好"的用例（`tests.rs:141` 是范式）。

**⚠ 第 2 步之前必须先解决一个陷阱：「拖出集合」会被启动回填静默吸回来。**

`backfill` 的判据是 `WHERE m.collection_id IS NULL`（`collection.rs:474`），
启动时只要还有 NULL 行就会跑（`main.rs:373-379`）。
⇒ 如果把"移出集合"实现成 `collection_id = NULL`，**下次启动这条产物会被重新算回原集合**，
而且**一个错都不报**。`collection.rs:463-467` 那句"用户手动拆开/合并过的一律不碰"目前**只是承诺**，
没有任何落库状态支撑它（`Origin::Manual` 从未被写入 —— 见 §1.3）。

两条出路（**必须先拍板，见 §6**）：

- **(a) 加一列"用户已明确移出"**：`media_assets.collection_opt_out INTEGER NOT NULL DEFAULT 0`，
  `backfill` 的 WHERE 带上 `AND collection_opt_out = 0`。要改三处：
  `server-rs/src/infra/db.rs:106-110` 的 `NEW_COLUMNS` + `SCHEMA_VERSION` + 末尾 `user_version`，
  以及 `db/schema.sqlite.sql`。范式见 `infra/db/tests.rs:141`。
- **(b) 把"拖出"表达成一个空壳 manual 集合**：不优雅，但不动 schema。
  代价是"移出"和"没分过组"在库里长得不一样，语义要多解释一句。

⚠ 注意现成的用例**没覆盖这一步**：`tests.rs:517-550` 的
`回填_只补没分组的_手动拆开的不动` 模拟的是"挪到**另一个集合**"，
**不是"挪出去"** —— 它恰好绕开了这个陷阱。新用例必须钉"移出 → 重启回填 → 仍在集合外"。

**第 3 步：前端交互（照手机分组的直觉）**

- **长按多选**：**已经有**（`gallery_page.dart:396-400` 进入多选、`:93-97` 工具栏按钮、
  `:116-123` 底部 `_SelectionBar`）。只需在 `_SelectionBar`（`:652-689`）加一颗「移到集合…」。
- **拖进集合**：建议**第一版做成"选择器"而不是真拖拽**。
  全仓库 `lib/` 下 `Draggable` / `DragTarget` / `LongPressDraggable` **零命中**，
  真拖拽是全新机制，而且**长按已经被"进入多选"占用了**（`gallery_page.dart:396-400`），
  拖拽手势必须和它分派 —— 成本差一个数量级，收益接近。
  （桌面端拖放还牵涉新依赖，见 §4.4。）
- **从集合里拖出来**：详情页在集合内左右切换时加「移出集合」（`:287-305` 的 actions 菜单），
  改完 `_load()` 重拉 `_siblings`（`media_detail_page.dart:99-112`）；
  画廊右键菜单（`gallery_page.dart:449-522`）与 `_SelectionBar` 也各加一项。

**⚠ 一个必须显式拍板的语义：集合格子上的多选，选中的是"代表那一条"还是"整组"。**

现在是**代表那一条**：`gallery_page.dart:385/392` 传的是 `m.id`，
而 `CollectionThumb` 的回调只有 `onTap` / `onToggleSelect`（`lib/widgets/collection_thumb.dart:33-36, 64`）。
⇒ 对集合格子批量删除，**只会删掉代表那一张**，其余成员变成散图或新集合。
做"移进 / 移出集合"时必须选一种并在 UI 上说清（建议：格子=整组，右键再给"只操作这一张"）。

**⚠ 批量要走批量端点，别照 `_bulkLink` 那样前端循环单条 PATCH。**
（`gallery_page.dart:528-537` 是循环 `updateMedia` 的范式，对"关联提示词"没问题，
但集合语义带副作用——源集合可能中途变空、需要 `prune_if_empty`，见 `collection.rs:507-515`。）
后端建议形状（保持仓库"批量走 POST"的口味，`server-rs/src/web/routes.rs:305-307` 写明
"Ktor 对 DELETE 带请求体不一致，所以用 POST"）：

| 端点 | 用途 |
| --- | --- |
| `POST /api/media/collections` | 建手动集合（`{ mediaIds, name? }`） |
| `POST /api/media/collections/{id}/members` | 移进 |
| `POST /api/media/collections/{id}/members/remove` | 移出 |
| `GET /api/media/collections` | 给"选目标集合"的弹出框 |
| （可选）`PATCH /api/media/{id}` 加 `collectionId` + `clearCollection` | 单条操作，与 `promptId`/`clearPrompt` 同形 |

⚠ 新增 DTO 字段若是 `Option`，必须带 `#[serde(skip_serializing_if = "Option::is_none")]`
—— 否则全仓扫描 `server-rs/tests/shape_lint.rs:20-46` 会红。

**还有两条"不用额外做"的**（免得重复劳动）：

- ✅ **手动集合不会被新图吸走**：`attach` 的 `ORDER BY (origin = 'auto') DESC`（`collection.rs:416`）
  保证新图优先挂 auto；而手动集合的 `group_key` 是合成的 `manual:*`，
  与新图算出来的 key 根本不同 ⇒ 天然隔离。**不用额外写逻辑。**
- ✅ **删产物后空集合会自动消失**：`prune_if_empty` / `prune_all_empty` 已有
  （`collection.rs:507-526`），启动路径也在调（`main.rs:391-403`）。
- ❌ **但"移出集合"不在这两条里** —— 见上面那段陷阱：`backfill` 会把
  `collection_id = NULL` 的行重新算回去，**必须靠 `collection_opt_out` 这类状态兜住**。
  （"移进另一个集合"倒是安全的：它有 `collection_id`，`backfill` 不会碰。）

### 1.5 手机同步：**不用改**

`media_collections` 与 `media_assets` **都已在同步白名单里**
（`server-rs/src/mobile/sync.rs:47-66` 的 `SYNCED`，第 55-57 行专门注释了"两张表都要有，
否则手机上折叠不出集合"），主键登记在 `sync.rs:206`，
账本触发器在 `db/schema.sqlite.sql:672-685`。

- **桌面手动改集合 → 手机会自动跟着变**（两表的 UPDATE 都进 `_sync_log` 下发）。
- 上行方向**手机被明确禁止**造组：`PHONE_FORBIDDEN` 含 `media_assets` 与 `media_collections`
  （`sync.rs:456-466`），`apply_one` 回 `FORBIDDEN_KIND`。**这条不要动。**
- Android 侧是**表驱动镜像**，不硬编码这两张表（`android/` grep `media_collections` 零命中），
  所以**加列也不用改 Kotlin**。

⇒ 结论修正：§1.3 担心的"同步要跟着改"其实**不成立**，这条风险比原先估的低。

### 1.6 防线与风险

- 新增用例（`server-rs/src/media/collection/tests.rs`，现有 60 条断言里**没有**下面这些）：
  1. 成因 ①：正、负提示词都为空 ⇒ 不进组、不建集合；
  2. **移出 → 重启回填 → 那条仍在集合外**（§1.4 的陷阱，最容易漏且静默失败）；
  3. 手动加入 / 移出之后 `backfill` 不许改动它；
  4. 手动集合**不吸新图**（`attach` 的"优先 auto"那条，`:405-409` 的注释在防它）；
  5. 源集合清空后要消失（`prune_if_empty` 已有，`collection.rs:507-515`）。
- 端点用例照 `server-rs/tests/api_test/media.rs` 的现成范式（含键集合断言）。
- ⚠ **前端画廊的多选目前零测试覆盖**（`test/` 下只有 `test/prompt_batch_test.dart:123-175`
  钉了预设库那一套），新动作要照它钩。
- **风险点**：`RULE_VERSION` 与 `SCHEMA_VERSION` 可能**双 bump**（前者看 §6 待确认，
  后者看是否加 `title` / `collection_opt_out` 列）。
- **升级路径**：老库的 `collection_id` 全是自动分组的结果，第 1 步修完之后会有集合变空 ⇒
  `prune_all_empty` 收尾即可（启动路径已经在调，`main.rs:391-403`）。
- 这份设计当初留的 TODO 在 `docs/archive/todo-10.11-implementation.md:104`
  （"❗用户'拆开/合并'的决定要落库且不参与重算：`origin=manual`"）—— **两年来一直没接上**。

---

## 2. 蓝图对比度过高（刺眼）

### 2.1 现场

画布**没有自己的底色**：`workflow_blueprint_page.dart:223-265` 是
`ClipRect > InteractiveViewer > SizedBox > GestureDetector > CustomPaint`，
一路透明 ⇒ 露出 `Scaffold` 的 surface。

而节点颜色是**写死的浅色系**（`lib/widgets/workflow_blueprint_painter.dart:376-394`）：

| kind | 节点底色 |
| --- | --- |
| 默认（普通节点） | `#F3F5F7`（近白） |
| note | `#FFF8E1` |
| reroute | `#CFD8DC` |
| variable | `#E1BEE7` |
| broadcast | `#FFE0B2` |
| display | `#B2EBF2` |
| bypasser | `#D7CCC8` |
| subgraph | `#C5CAE9` |
| unknown | `#FFCDD2` |

正文同样是浅色主题的值：标题 `#101418`（近黑，`painter.dart:247`）、
类名 `#5A6673`（`:258`）、插口名 `#37474F`（`:305,317`）、参数值 `#455A64`（`:337`）。

**而 App 默认是深色**：`lib/core/settings_store.dart:58` `_themeMode = AppThemeMode.dark`，
`lib/core/theme.dart:25-38` 也是这个默认值。

### 2.2 量化

节点底 `#F3F5F7` 的相对亮度 ≈ 0.91；M3 深色 `surface` 量级 `#141218` 的相对亮度 ≈ 0.0065。
WCAG 对比度 ≈ `(0.91+0.05)/(0.0065+0.05)` ≈ **17:1**
（这是按 M3 深色 surface 量级估的**近似值**，不是实测像素；WCAG AAA 正文门槛是 7:1）。
大块**实心填充**长期停在 15:1 以上，就是"刺眼"的物理来源。

**顺带一个反过来的毛病**：切到浅色主题时，节点底 `#F3F5F7` 与浅色 surface 几乎同色，
而分组底板是 `color.withValues(alpha: 0.06)`（`painter.dart:109`）——基本看不见。
⇒ 这一套配色**只对"它没被设计来适配"的那个主题成立**，两边都不对。

### 2.3 建议

**核心思路：把"按 kind 上色"从"整块底色"降级成"边框 + 标题条"，正文永远用与底色同侧的灰。**

1. 新增一个**纯函数调色板**，例如 `BlueprintPalette.of(Brightness)`，放在
   `lib/core/` 或 painter 内（要能单测）。`BlueprintPainterSpec` 增加 `palette` 字段。
   - 深色：画布底比 `surface` 再低一档；节点底比画布高**一档**（不是高一整片白）；
     正文用 `onSurfaceVariant` 量级的浅灰。
   - 浅色：节点底略白于画布 + 一圈描边来分层（**别靠阴影**，AGENTS §6.1/`theme.dart:99-103`
     已经定了"层次用色彩表达、不用 elevation"）。
2. `kind` 的颜色改用在**边框 + 标题条**上（`painter.dart:231-240` 那段已经有"标题条"这个结构，
   只需把 `alpha 0.32` 的填充换成饱和度高一点的 kind 色）。
3. 分组底板把 alpha 提上来（深色下 0.06 等于没有），或改成"只画描边 + 标题"。
4. **加一个画布自己的底色**（不要再借 Scaffold 的 surface），顺带可以画淡网格
   ——ComfyUI 的观感一半来自网格。
5. ⚠ **`shouldRepaint` 必须把 `palette` 一起比进去**（`painter.dart:446-457`）：
   漏了的表现是"用户在设置里切主题，蓝图不变色"，而**一个错都不报**。

### 2.4 防线（这条要按仓库的规矩来）

`docs/workflow-blueprint-design.md:164` 写明了**不做 golden / 截图回归**，
"观感靠开发期 debug 版人工看"。所以这条修复**不能**用截图来守。

可测的替代（仓库里已有范式 `test/theme_text_color_test.dart` 是"逐个样式查颜色"）：
把"节点底 vs 画布底"的对比度做成断言——**深色与浅色两套都要落进一个区间**
（例如 1.2 ~ 4.5:1），并把"深色下不许有比画布亮 N 倍的大色块"变成对
`BlueprintPalette` 的纯函数断言。加进 `test/workflow_blueprint_test.dart` 的「画布」组。

### 2.5 节点参数显示不清楚

三个独立原因，都在 `painter.dart`：

1. **行高与真实节点高度不是一套。**
   画笔自定 `_headerHeight = 26` / `_portHeight = 18` / 参数行 `16`（`painter.dart:67-68, 326`），
   而布局对**界面格式**是**原样用文件里的 `size`**（`lib/core/workflow_blueprint.dart:515-528`
   只在 `height <= 0` 时才估）。真实存档里节点高度常见 46~100，
   于是 `y > rect.bottom - 10`（`painter.dart:327`）经常一上来就 `break` —— **参数一行都不画**。
   ⇒ 正解：把"节点内几何"抽成**一个真源**（例如 `blueprintMetrics`），让布局估高与画笔绘制都读它；
   或者在界面格式下**忽略 `size.height`、按内容算**（但那会与"如实还原 ComfyUI 的坐标"冲突——
   `workflow_blueprint.dart:370-376` 有这条纪律，所以选前者更安全）。
2. **中文截断估宽是错的。** `painter.dart:407-413`：

   ```dart
   final limit = (maxWidth / (size * 0.62)).floor();
   ```

   `0.62em` 是拉丁字体的平均字宽；中文≈1.0em ⇒ **低估约 40%**，长中文值会溢出、压到相邻元素。
   而且 `_text` 最后是 `TextPainter(...)..layout()`（`painter.dart:414-427`）
   **没有传 `maxWidth`** ⇒ `maxLines: 1, ellipsis: '…'` 形同虚设，永远不会触发省略号。
   ⇒ 直接给 `layout(maxWidth:)` 并把估算删掉即可。
   ⚠ `painter.dart:408` 的注释担心"上千节点时逐字量是主要开销"——但
   `_paintNodes` **已经做了视口剔除**（`painter.dart:187-201`），逐字量的只剩屏幕里那几十个节点，
   而且当前实现**本来就已经在建 TextPainter 并 layout 了**，所以换成正确约束**几乎不额外花钱**。
3. **低缩放时参数静默消失**：`if (spec.scale > 0.45)`（`painter.dart:324`）。
   这是 LOD，本身合理（`painter.dart:42-43` 写了理由），但用户看不出"这里还有参数"。
   ⇒ 建议在 LOD 以下改画一条"参数 N 项"的摘要行，而不是干脆不画。

### 2.6 编辑应当"在蓝图上直接操作"

**现状**：所有编辑都在面板里 —— `_nodeDetails`（`workflow_blueprint_page.dart:612-695`）里的
标题框 / 模式 `SegmentedButton`（`:644-677`）、参数行 `_widgetRow`（`:724-767`）。
画布的交互**只有两件**：`InteractiveViewer` 的缩放平移，以及
`onTapUp → hitTestNode`（`workflow_blueprint_page.dart:233-242`；
`hitTestNode` 在 `painter.dart:463-470`，**只按节点框命中，没有"点在某一行参数上"这个概念**）。

**建议做法**：

1. 把"参数行 → 矩形"抽成纯函数（与 §2.5 的 `blueprintMetrics` 共用），
   加 `hitTestWidget(layout, node, point) -> int?`（返回 `widgets_values` 的下标）。
   与 `hitTestNode(hitTestNode, ...)` 完全同构，**天然可单测**。
2. 命中之后，**在屏幕空间**（`InteractiveViewer` **外面**）叠一层真实 Flutter 输入控件：
   用当前 `Matrix4` 把节点/参数行的矩形投影到屏幕坐标，把 `TextField` / `Switch` 放在那儿。
   - **不要**把控件塞进 `InteractiveViewer` 的 child 里：那会让控件跟着 `scale` 变换，
     高倍缩放时文字会糊，而且内嵌 `TextField` 会和 `InteractiveViewer` 的拖动抢手势。
   - 屏幕空间叠层天然绕开手势冲突：它盖在 `InteractiveViewer` 之上，点击/输入是普通控件行为。
   - 页面已经在用 `ValueListenableBuilder<Matrix4>`（`workflow_blueprint_page.dart:245-259`），
     投影所需的 matrix 现成。
3. 保存路径**一个字都不改**：仍然走 `WorkflowEditor.setWidgetValue / setNodeMode / setNodeTitle`
   ⇒ `_afterEdit()`（`workflow_blueprint_page.dart:140-145`）里那句"先重新序列化 `_raw`"
   是硬纪律（注释里写了：漏了会"界面改了、复检还是旧的、一个错都不报"）。
4. **窄屏要退让**：`< 900`（`kWideLayoutMinWidth`）时画布只有 360dp 宽，
   在图上直接编辑不现实 —— 保留底部面板，参数点击改成弹 `showModalBottomSheet`。
   （AGENTS §6.0 要求两端各自问一句"这好用吗"。）
5. **不要顺手把"画布拖动节点"一起做**：`docs/workflow-blueprint-design.md:161` 明确写了
   它与 `InteractiveViewer` 的平移冲突、要自己接管平移，且已排进 §7 的下一阶段第 1 条。
   本条的收益是"编辑参数不用来回看两处"，与拖动节点是两件事，**别混在一起做**。

### 2.7 面板不能折叠

- 宽屏：`SizedBox(width: 380, child: panel)`（`workflow_blueprint_page.dart:392`）——**写死**。
- 窄屏：`height: (constraints.maxHeight * 0.42).clamp(200.0, 420.0)`（`:404-407`）——也写死。
- 建议：加折叠开关（记住状态即可，不必落盘），宽屏再给一条可拖的分隔条。
  ⚠ 折叠之后**画布必须重排**（`_fitToView` 只在 `_fitted == false` 时跑一次，
  `:374-379`；面板一变宽，`constraints.biggest` 就变了）——否则会出现"展开面板后画布被裁掉一块"。
  这条要进用例。

---

## 2.8 「跑到右上角」与「加载慢」的真因（动手时才挖出来，比 §2.1 深一层）

### ① 适配的平移量用错了参照系

`_fitToView` 原来写的是：

```dart
final dx = (viewport.width - bounds.width * fit) / 2 - bounds.left * fit;
final dy = (viewport.height - bounds.height * fit) / 2 - bounds.top  * fit;
```

但画布的 `origin = _canvasPadding - bounds.left` —— `bounds.left` **已经被 `origin`
抵消过一次**了。再减一次 `bounds.left * fit`，等于在一个已经是"画布坐标"的空间里
按"图坐标"平移。`bounds.left` 为负（真存档常见）时这一项是**大额正值**，
内容整体被推到右边；`bounds.top` 同理往下推。

正确写法（已改）：

```dart
final dx = viewport.width  / 2 - (_canvasPadding + bounds.width  / 2) * fit;
final dy = viewport.height / 2 - (_canvasPadding + bounds.height / 2) * fit;
```

**防线**：`workflow_blueprint_test.dart` 的
「打开就把内容缩放对齐到画布中心」—— 断言"图包围盒中心映射到屏幕后正好落在视口中心"，
数学上精确（与缩放无关），偏离 0.5px 就红。

### ② LOD（"缩太小就不画正文"）在缩小时**完全失效**

画布传的是 `scale: matrix.getMaxScaleOnAxis()`。而 `Matrix4.getMaxScaleOnAxis()`
**把 Z 轴也一起算**，画布矩阵是 `scale(fit, fit, 1)` —— Z 恒等于 1，
于是**只要在缩小（fit < 1），它永远回 1.0**。

`_paintNode` / `_paintPorts` 里那几道 `spec.scale > 0.22 / 0.3 / 0.45 / 0.55`
因此全部照旧通过：一份 9600px 宽的工作流按 **8%** 打开时会**逐字排版并绘制全部文本**。
这既是卡顿，也是"加载比 ComfyUI 慢一个量级"的观感来源之一（另一半是 `/object_info`，
见 §4/§5 之外的 `object_info_cache`）。

**改法**：新增 `blueprintScaleOf(Matrix4)`（只取 X/Y 两轴的模），页面改用它。
**防线**：同一条用例里断言 `blueprintScaleOf(matrix) ≈ (viewport.width - 48) / bounds.width`
—— 用 `getMaxScaleOnAxis()` 会回 1.0，立刻红。

⚠ 这条是**静默失败**的典型：不报错、不警告，只是"慢一点、糊一点"，
所以必须靠断言钉住，不能靠肉眼看。

---

## 3. 点会话里的图片会跳浏览器

### 3.1 现场

- 消息气泡里的附件块：`lib/pages/ai_home_page.dart:1228-1238`（只在**用户轮**出现）→
  `_MessageAttachment`（`:4198-4234`）。
- 它的 `onTap` 写死：`:4208-4211`

  ```dart
  Tooltip(
    message: '$name\n点击在新窗口打开',
    child: InkWell(onTap: fileUrl == null ? null : () => _openAttachment(fileUrl!)),
  ```

- `_openAttachment`（`:4246-4253`）就是：

  ```dart
  await launchUrl(Uri.parse(url), mode: LaunchMode.externalApplication);
  ```

- URL 是后端的 http 地址：`lib/core/ai_api_client.dart:383-385`
  `'$baseUrl/api/ai/attachments/$attachmentId/file'` ⇒ `externalApplication` 落到**系统浏览器**。
- 输入区待发附件的托盘**共用同一个函数**：`:4090-4109`（`_AttachmentTile`，定义 `:4053`）第 4097 行。
- **不相关的另一处** `launchUrl`：`ai_home_page.dart:5626-5636` 的 `_openFolder`（打开目录，`Uri.file`）。
- Markdown 那条路**也没渲染图片**：`lib/widgets/markdown_view.dart` 只有行内链接
  （`:253-255` 挂 `TapGestureRecognizer` → `:282-291` 的 launchUrl），
  图片语法在 `lib/widgets/markdown.dart` 里**没有解析**。
- AI 产物那条路**已经是应用内的**：`_ProducedMediaCard`（`ai_home_page.dart:2429`）→ `_openDetail`（`:2568-2572`）
  → `MediaDetailPage`，而详情页的图片预览就是 `ZoomableImageView`。

### 3.2 缺的只是一个"应用内预览"

应用内查看器**已经存在**：`lib/widgets/zoomable_image_view.dart:52`
（`ZoomableImageView`，参数 `url / imageSize / imageProvider / minScale / maxScale / background`；
`test/zoomable_image_test.dart` 已有）。
**但全仓库只有一处用它**：`lib/pages/media_detail_page.dart:644`。
聊天页连 `import` 都没有（`ai_home_page.dart:1-20`）。

### 3.3 建议

1. 在 `lib/widgets/zoomable_image_view.dart` 里加一个 `showImagePreview(context, url, {imageSize})` 封装
   （`showDialog` + 深色底 + 关闭按钮），**做成通用件**，别在聊天页里现写一个。
2. `_MessageAttachment` 要**多接一个模态**（现在只吃 `name/thumbUrl/fileUrl`，`:4198-4203`），
   按模态分派：`image` ⇒ 应用内弹窗预览；视频可考虑应用内播放器（`PlatformVideoView` 已存在）；
   音频 / 文档仍然走 `_openAttachment`（外跳是合理的）。
3. 用**原件** URL（`store.fileUrlFor(id)`，`ai_workspace_store.dart`）而不是 `thumb`。
4. ⚠ **不要图省事复用 `MediaDetailPage`**：它只吃画廊的 `media id`（`/api/media/{id}`），
   附件是**另一套 id 空间**——`lib/core/ai_api_client.dart:387-391` 的注释专门警告过
   "把媒体 id 塞进附件接口只会 404"（用户报过的 bug）。
5. 弹窗要处理两件事：气泡外层包着 `SelectionArea`（`ai_home_page.dart:1219`），
   以及外层 `ListView` 会抢滚轮（`media_detail_page.dart:415-421` 同款问题的历史）。
6. 文案要跟着改：`:4209` 与 `:4083` 的"点击在新窗口打开" ⇒ "点击预览"。
7. 用例：`test/ai_message_render_test.dart` / `test/ai_attachment_test.dart`；
   ⚠ 现在**没有 url_launcher 的 mock**，`_openAttachment` 的 `catch` 会让外跳在测试里静默，
   所以断言要写"弹窗出现"而不是"没调 launchUrl"。

**工作量**：小。半天量级，纯前端，无数据影响。

---

## 4. Ctrl+V 不能粘贴附件

### 4.1 现场

- 输入框的键盘处理**只有一个** `Shortcuts` + `Actions`，而且**只绑了 Enter**：
  `ai_home_page.dart:3208-3237`（`SingleActivator(LogicalKeyboardKey.enter): _SendIntent()`，`:3210`；
  `CallbackAction<_SendIntent>`，`:3214`）。**没有任何 PasteIntent / Ctrl+V**。
- 另一处键盘处理是**手写的 `Focus.onKeyEvent`**（答题卡的 Enter 提交）：
  `ai_home_page.dart:1929-1942`，不是 Shortcuts/Actions 那套。
- **全仓库 0 处 `Clipboard.getData` / `hasStrings`**；只有 4 处 `Clipboard.setData`
  （`common.dart:501`、`ai_home_page.dart:4987`、`workflow_blueprint_page.dart:151`、
  `workflow_viewer.dart:203`）。修饰键判断只用了 `isShiftPressed`（`:1934`），**没用过 `isControlPressed`**。
- 加附件的**唯一入口**是按钮 → 文件选择器：
  `ai_home_page.dart:3345-3352` → `_pickFiles()` `:3403-3421` → `store.attachFiles`
  （`lib/state/ai_workspace_store.dart:2116-2136`）→ multipart 上传（`lib/core/ai_api_client.dart:365-376`）。
- **拖拽也没有**：`lib/` 下 `DragTarget|Draggable|desktop_drop` 零命中；
  `windows/` 下 `WM_DROPFILES|DragAcceptFiles|DragQueryFile` 零命中；`android/` 同。
  ⚠ 但界面**已经承诺了**：`ai_home_page.dart:2991-2993` 写着"也可以把文件拖进来当附件"，
  `lib/pages/gallery_page.dart:346` 也写着"拖进来"。**文案先于功能存在**。

### 4.2 为什么"加个 PasteIntent"不够

Flutter 内置的 `Clipboard.getData` **只支持文本**（`Clipboard.kTextPlain`）：

- 从资源管理器 **Ctrl+C 一个文件**，剪贴板里是 `CF_HDROP`，
  **不含**纯文本路径 ⇒ `getData` 拿不到东西；
- 截图工具截的图在 `CF_DIB` / `CF_BITMAP` ⇒ 同样拿不到。

⇒ 想要"粘图片 / 粘文件"，**必须能读自定义剪贴板格式**（`win32` FFI 或原生通道），
**不是"加个 PasteIntent"能解决的**。这一点决定了 §4 与 §5 是**同一件事的两面**。
（可以分开交付：先做"粘路径文本"，但那只覆盖"复制为路径"这个少数场景，**收益低到不值得单做**。）

### 4.3 建议

**先分清三件能力，它们的成本差很多：**

| 能力 | 能不能纯 Dart + `win32` FFI 做到 | 说明 |
| --- | --- | --- |
| **写**文件到剪贴板（§5） | ✅ 能 | `EmptyClipboard` + `SetClipboardData(CF_HDROP)`；见 §5.3 方案 A |
| **读**剪贴板里的文件/位图（本节） | ✅ 能 | `IsClipboardFormatAvailable` + `GetClipboardData` + `DragQueryFile`，不需要窗口过程 |
| **把文件拖进窗口** | ❌ 不能 | 要 `DragAcceptFiles` + 子类化 `WndProc` 收 `WM_DROPFILES`（或 `IDropTarget`），**必须有原生代码或第三方包** |

⇒ 所以：**粘贴和复制可以纯 Dart（`win32` FFI，见 §5.3 方案 A），拖拽不行。**
这条决定了下面 1、2 两条可以省掉原生代码。

1. 新增一个 `lib/core/clipboard_files.dart`（或并进 §5.3 的 `lib/core/file_clipboard.dart`），
   用 `win32` FFI 提供三个函数（**§4 与 §5 共用**）：
   - `clipboardHasFiles()` → 有没有 `CF_HDROP` / `CF_DIB`（给"要不要拦截 Ctrl+V"当判据）；
   - `clipboardReadFiles()` → 文件路径列表（`DragQueryFile`）；
   - `clipboardWriteFiles(paths)` → 写 `CF_HDROP` + `Preferred DropEffect`（**这就是 §5**）。
2. 输入框的拦截要**只拦该拦的**：给输入框的 `FocusNode` 加 `onKeyEvent`
   （或在那段 `Shortcuts`，`ai_home_page.dart:3208-3237` 里加
   `SingleActivator(keyV, control: true)`），判据是 `clipboardHasFiles()`：
   - 有文件 ⇒ 落进 `attachFiles` 那条既有链路（`ai_workspace_store.dart:2116-2136`，**复用，不要另写上传**）；
   - 没有 ⇒ `KeyEventResult.ignored`，**让 `TextField` 自己粘文本**（别把普通文本粘贴弄坏）。
   ⚠ 现在全仓库**没用过 `isControlPressed`**（只用过 `isShiftPressed`，`:1934`），是新写法。
   ⚠ "粘截图"要先落地成文件（`CF_DIB` 位图 → PNG 写进 `<根>\.run\tmp`，AGENTS §13 的临时目录约定），
   再走上传 —— **建议第一版只做"粘文件路径"，把 `CF_DIB` 往后放**。
   （**2026-10-24 已补**：`readImage` 读位图，Dart 侧转 PNG 后直接 `fromBytes` 上传，
   **不落 `.run\tmp`**；`CF_DIB`/`CF_BITMAP` 的 32bpp alpha 由原生强制置 255，
   免得截图粘上去是一张全透明的图。）
3. **拖拽要单独决策**：Windows 侧 `DragAcceptFiles` + `WM_DROPFILES`（原生），
   或者引 `desktop_drop` 包。本仓库对新依赖很保守（AGENTS §9 的 `video_player_win` 教训），
   而且这条**不影响 §5**（§5 不需要原生）⇒ 建议**先不做拖拽**，
   同时把 `ai_home_page.dart:2993` 与 `gallery_page.dart:346` 那两句"拖进来"的文案**先改掉**
   （现在功能不存在，文案是假的）。
4. Android 侧：粘贴走 `ClipData` / `ContentResolver`，是另一份实现（两端各自做）。

### 4.4 为什么这条必须"先定方案再动手"

§4（粘）与 §5（复制文件）走的是**同一份剪贴板 helper**（同样是 `CF_HDROP`，
一个读一个写，都在 `lib/core/` 里）。如果先零散地加一个 Ctrl+V，
后面做 §5 时大概率会重写一遍 ⇒ 建议**两条一起设计、分开交付**。

另外它牵连一个**上位的依赖决策**：要不要把 `win32` 从传递依赖提成直接依赖
（§5.3 方案 A），还是自写 C++ 原生通道（方案 B）。
⚠ 无论选哪个，都**不要**退回"用 WMI / wscript / PowerShell 脚本搬文件"那条路 ——
AGENTS §2 明确否掉了那一套（会弹黑框、且脱离进程树）。

---

## 5. 应用内不能复制"文件本身"

### 5.1 现场

现在所有"复制"都是**文本**：

| 位置 | 复制的是什么 |
| --- | --- |
| `lib/widgets/common.dart:500-511` `copyToClipboard` | 任意文本（**唯一实现**，成功弹「$label已复制」；**没有 try/catch**） |
| `lib/widgets/common.dart:434`（`CopyableText`） | 该文本块的内容 |
| `lib/pages/media_detail_page.dart:295-300`（AppBar「更多」→ `:263-265`） | 「复制文件地址」= **HTTP URL**（`api.absolute(m.fileUrl)`，`lib/core/api_client.dart:64-65`） |
| `lib/pages/media_detail_page.dart:551-575` → `:578-591`（**对画面右键**） | 文件地址（HTTP URL）/ 文件名 / 正向提示词 / 负向提示词 |
| `lib/pages/ai_home_page.dart:4987` | skills 投放口的**目录路径字符串**（打不开资源管理器的兜底） |
| `lib/widgets/workflow_blueprint_page.dart:151` | 工作流 JSON 原文 `_raw` |
| `lib/widgets/workflow_viewer.dart:203` | 工作流完整 JSON 原文 |
| `lib/widgets/prompt_params.dart:73, 125` | LoRA 标签串 / 单个 `<lora:名字:权重>` |

⚠ **画廊格子的右键菜单（`gallery_page.dart:449-486`）连一个复制项都没有** —— 这也是个缺口。

`Clipboard.setData(ClipboardData(text: …))` 在 Windows 上只会写 **`CF_UNICODETEXT`**
⇒ 在资源管理器里 Ctrl+V **粘不出文件**（只会粘出一串路径文本，或者什么都没有）。
（已对过引擎源码：`platform_handler.cc` 的 `kTextPlainFormat[] = "text/plain"`，
别的格式回 `"Unknown clipboard format"`；`SetString()` 内部先 `::EmptyClipboard()` 再
`::SetClipboardData(CF_UNICODETEXT, …)`。）

### 5.2 缺什么

要有"复制文件本身"，必须往剪贴板写 **`CF_HDROP`**（外加 `Preferred DropEffect = DROPEFFECT_COPY`，
否则资源管理器会当成"剪切"）：

- Flutter 内置**做不到**；
- 仓库**没有直接依赖**任何能写自定义剪贴板格式的包：`pubspec.yaml:10-58` 里
  **没有** `super_clipboard` / `pasteboard` / `desktop_drop` / `win32` / `ffi`。
  ⚠ 但 `pubspec.lock` 里 **`win32 6.4.0` 已经是传递依赖**（path_provider_windows /
  shared_preferences_windows / windows_file_picker 带进来的），
  而且它的 API 齐：`CF_HDROP`（`enums.g.dart:1097`）、`DROPFILES` 结构体（`structs.g.dart:6698`）、
  `OpenClipboard/EmptyClipboard/SetClipboardData/CloseClipboard/IsClipboardFormatAvailable`
  （`user32.g.dart`）。**本机 pub 缓存里已有这个版本，不需要换版本、不需要联网。**

### 5.3 建议

**方案 A（推荐）：纯 Dart + `win32` FFI，`windows/runner/` 一个文件都不改。**

1. `pubspec.yaml` 的 dependencies 里把 `win32: ^6.4.0` **从传递依赖提成直接依赖**
   （版本已在 lock 里，无新下载）。
   ⚠ 这是**本仓库第一次把 `win32` 提成直接依赖**，要在 `pubspec.yaml` 里写清理由
   （照 `path_provider_windows` / `shared_preferences_windows` 那几行的注释风格）。
2. 新增 `lib/core/file_clipboard.dart`：`DROPFILES` + `GlobalAlloc(GMEM_MOVEABLE)`
   + `OpenClipboard / EmptyClipboard / SetClipboardData(CF_HDROP) / CloseClipboard`。
   ⚠⚠ **同一次里再写一份 `CF_UNICODETEXT`（路径文本）当兜底**；
   ⚠⚠ **绝不要再调 Flutter 的 `Clipboard.setData`** —— 它会先 `EmptyClipboard()`，
   把刚写好的 `CF_HDROP` 一起清掉（这是本方案最容易踩、且**不会报错**的一个坑）。
3. **磁盘绝对路径**：现在前端**拿不到**。`MediaAsset` 只有 `fileUrl`（http）、`storedName`、`originalName`
   （`lib/models/models.dart:379-467`），而 `server-rs/src/web/models.rs:215` 明写
   "`fileUrl`/`thumbUrl` 是后端算好的路径，前端不许自己拼"。两条路：
   - **推荐**：后端加字段/端点 —— `MediaDto` 加 `local_path`（`web/models.rs:275` 附近），
     或加 `GET /api/media/{id}/path`。
     ⚠ 新增 `Option` 字段要带 `skip_serializing_if`（`server-rs/tests/shape_lint.rs:20-46`）。
   - 备选：`health()['storageDir'] + 'media' + storedName`
     （`/api/health` 的 `storageDir` 见 `web/routes.rs:1856, 1871, 1890-1898`；
     但**违反上面那条纪律**，等于在前端拼第二份路径）。
     磁盘布局真源：`server-rs/src/main.rs:260` `storage/media` +
     `server-rs/src/media/media.rs:352-359` 的 `resolve_media`（含目录穿越校验）。
4. 入口：`media_detail_page.dart:295-300` 的菜单加 `MenuOption(value: 'copyFile', label: '复制文件')`
   + `:231-284` 的 switch 加 case；画面右键 `:551-575` / `:578-591` 同加；
   顺手给画廊右键（`gallery_page.dart:449-486`）补上复制项。
5. **失败要如实报**：`copyToClipboard`（`common.dart:500-511`）现在**没有 try/catch**，
   调用点也都没 catch，全仓库 grep `复制失败` **零命中** ⇒ **没有**"复制失败要降级"的既有惯例。
   新 helper 必须自己 try/catch + SnackBar（可顺带给 `copyToClipboard` 补失败提示，
   **别动成功路径的文案**）。既有降级范式在"打开文件夹"那条路上：
   `ai_home_page.dart:5622-5636` 的 `_openFolder`（"打不开就返回 false，由调用方把路径显示给用户"）。
6. 回归：`test/context_menu_test.dart:210-230` 要跟着改
   （另有两处拦 `Clipboard.setData` 的用例：`test/media_prompt_flow_test.dart:293-301`、
   `test/workflow_viewer_test.dart:160-166`）。

**方案 B：原生 MethodChannel**。`windows/runner/flutter_window.cpp:27` 的
`RegisterPlugins(...)` 之后，用 `flutter_controller_->engine()->messenger()` 建
`comfyhub/clipboard`；新增 `.cpp/.h` 要加进 `windows/runner/CMakeLists.txt:9-17`。
比 A 多改 3 个原生文件、多一份 C++ 代码，收益是"不引 pub 依赖"。
⇒ 两个方案请拍板（§6）。

**Android：这条基本不成立，要单独想。**

- 手机上的产物是**远端 HTTP**（desktop 后端的 `/api/media/{id}/file`），
  `lib/` 里 `getTemporaryDirectory` / `cacheDir` **零命中** ⇒ **手机本地根本没有这个文件**，
  "复制文件本身"无从谈起（除非先下载，而落点按 AGENTS §13 要先定）。
- 而且 `AndroidManifest.xml` **没有 FileProvider**（只有 INTERNET + CAMERA）。
- 真要做：① 加 FileProvider + `res/xml/file_paths.xml`；② `HarnessBridge.kt` 的
  `onMethodCall`（`:138-250`）加一个方法走 `ClipData.newUri(...)` +
  `FLAG_GRANT_READ_URI_PERMISSION`；③ 只能覆盖**手机本地附件**
  （`ai_home_page.dart:3403-3421` 的 `FilePicker` 给的真实路径）。
- 建议**先把 Android 排除在"复制文件"之外**，只做"复制文件地址"（现在就有）。

---

## 6. 需要用户拍板的点

**必须拍板的（不定就没法动手）**

1. **「移出集合」的状态怎么表达？**（§1.4 的陷阱）
   (a) 加 `media_assets.collection_opt_out` 列（干净，schema +1）；
   (b) 用空壳 manual 集合表示"已移出"（不动 schema，语义更绕）。
   **不做这一步，"拖出去"会在下次启动被静默吸回来。建议 (a)。**
2. **手动集合要不要"名字"？** 要 ⇒ 加 `media_collections.title` ⇒ schema 版本 +1。
   不要 ⇒ 只能显示"N 张图的集合"，体验差。**建议要。**
3. **集合格子上的多选，选中的是"代表那一条"还是"整组"？**（§1.4）
   现在是代表，用户很可能以为是整组。**建议改成整组**，并在右键另给"只操作这一张"。
4. **§1.4 第 1 步（空提示词不进组）算 bug fix 还是规则变更？**
   —— 决定要不要 `RULE_VERSION += 1`。建议算 bug fix（只多挡一种"没有走向可判"的情况，
   与 `Assigned::Orphan` 的既有语义同源），但那会让存量集合变少，用户要知情。

**可以后置的（先按推荐做，回头可改）**

5. **§4/§5 的实现路径：把 `win32` 提成直接依赖（纯 Dart，§5.3 方案 A），还是自写 C++ 原生通道（方案 B）？**
   A 的好处：`windows/runner/` 一个文件都不改、版本已在 lock 里、不需要联网；
   代价：本仓库第一次直接依赖 `win32`（它是 FFI 包，**不打进发布包体积**，
   与 AGENTS §11 的 14MB 不冲突，也不碰 §12 的完整性标签链路）。
   **建议 A。**
6. **§1.4 第 3 步第一版做"选择器"还是"真拖拽"？**
   建议选择器（复用现成的 `_SelectionBar`）；真拖拽是全新机制、长按已被多选占用、
   而且拖进窗口只能靠原生 ⇒ **建议先不做，并把那两句假文案改掉**。
7. **Android 的"复制文件"要不要做？**（§5.3）
   手机上产物是远端 HTTP、本地无文件、也没有 FileProvider ⇒
   **建议先排除，只保留"复制文件地址"。**
8. **蓝图配色走"深色系 + kind 只上边框/标题条"（推荐，接近 ComfyUI），
   还是"两套主题各配一份 kind 色"？**
   前者改动小、语义还在；后者更"忠实"但要维护两套 9 色的表。
9. **节点参数要不要做"LOD 摘要行"**（缩放低于 0.45 时画"参数 N 项"而不是不画，§2.5 第 3 点）？
   纯观感，可后置。

---

## 7. 排期建议（按"收益 / 成本"）

| 顺序 | 事项 | 成本 | 理由 |
| --- | --- | --- | --- |
| 1 | §2 蓝图配色 + 参数截断（`layout(maxWidth:)` 那条） | 小 | 纯前端、无数据、立刻消掉"刺眼"与"中文溢出" |
| 2 | §3 会话图片应用内预览 | 小 | 通用件已存在，只需接上 + 一个 dialog 封装 |
| 3 | §1.4 第 1 步（空提示词不并组） | 小 | 后端一处判断 + 用例，直接减少"分错" |
| 4 | §4.3 第 3 点：把两句假的"拖进来"文案改掉 | 很小 | 现在功能不存在，文案在骗人 |
| 5 | §5 + §4（`win32` FFI：复制文件 / 粘附件） | 中 | **一件依赖决策带出两个能力**；Windows 优先，Android 先排除 |
| 6 | §2.7 面板折叠 + §2.5 参数几何真源 | 中 | 为第 8 项铺路 |
| 7 | §1.4 第 2~3 步（手动集合 + 移进/移出 + 批量） | **大** | 动 schema（`title` / `collection_opt_out`）、动 `backfill`、动同步不必要但端点全新 |
| 8 | §2.6 蓝图上直接编辑参数 | 中 | 收益是体验，前置是第 6 项 |
| 9 | 真拖拽（拖进窗口 + 卡片互拖） | 大 | 唯一**必须**原生的那条；价值最低，最后做 |

---

## 8. 本文的证据清单（便于复核）

- 归类判定：`server-rs/src/media/collection.rs:1-43, 50-54, 112-121, 286-320, 375-403, 405-449, 454-459, 469-499, 520-526`
- 归类缺空判据：`server-rs/src/comfy/capture_ingest.rs:345-372`、`server-rs/src/workflow/graph_parse.rs:41,63,270-296`
- 归类的既有防线与"刻意取舍"：`server-rs/src/media/collection/tests.rs:354, 464-481, 484-515, 517-550`
- **回填陷阱**：`collection.rs:469-475`（`WHERE collection_id IS NULL`）+ `main.rs:373-379`（还有 NULL 就跑）
- 表结构：`db/schema.sqlite.sql:89-112, 117-122, 148, 672-685`
- 无手动入口：`server-rs/src/web/inputs.rs:131-153`、`server-rs/src/web/routes.rs:218-229, 345`、
  `server-rs/src/web/write_routes.rs:50`、`server-rs/src/media/media_write.rs:49-92`
- 手机同步：`server-rs/src/mobile/sync.rs:47-66, 206, 456-466`（`SYNCED` 与 `PHONE_FORBIDDEN` 都已覆盖）
- 遗留 TODO：`docs/archive/todo-10.11-implementation.md:104`
- DTO 形状门禁：`server-rs/tests/shape_lint.rs:20-46`
- 蓝图配色与几何：`lib/widgets/workflow_blueprint_painter.dart:67-68, 103-128, 187-201, 203-278, 280-343, 396-428, 446-457, 463-470`
- 蓝图布局与编辑：`lib/core/workflow_blueprint.dart:362-406, 505-528, 531-549`
- 蓝图页面：`lib/widgets/workflow_blueprint_page.dart:140-145, 215-265, 372-411, 448-494, 612-695, 724-767`
- 蓝图设计纪律：`docs/workflow-blueprint-design.md:137-152, 155-166, 169-175, 181-190`
- 会话图片外跳：`lib/pages/ai_home_page.dart:1219, 1228-1238, 2429-2572, 2989-2996, 4047-4110, 4197-4253, 5622-5636`
- 应用内查看器：`lib/widgets/zoomable_image_view.dart:6-7, 13, 26, 52-73`；唯一使用处 `lib/pages/media_detail_page.dart:644`
- 附件 id 空间警告：`lib/core/ai_api_client.dart:380-393`（附件 `:380-385`，产物 `:387-393`）
- 键盘与剪贴板：`lib/pages/ai_home_page.dart:1929-1942, 3208-3237, 3345-3352, 3403-3421`
- 附件上传链路：`lib/state/ai_workspace_store.dart:2105, 2116-2136, 1757-1776`
- 复制全为文本：`lib/widgets/common.dart:434, 500-511`、`lib/pages/media_detail_page.dart:263-265, 295-300, 551-575, 578-591`、
  `lib/pages/ai_home_page.dart:4987`、`lib/widgets/prompt_params.dart:73, 125`
- 复制失败无降级惯例：全仓 `复制失败` 零命中；降级范式在 `ai_home_page.dart:4971-4989, 5622-5636`
- 主题默认深色：`lib/core/settings_store.dart:58, 155-156, 254-255`、`lib/core/theme.dart:6-39, 99-106`
- 剪贴板只有文本（引擎侧）：`platform_handler.cc` 的 `kTextPlainFormat[] = "text/plain"`、
  `SetString()` 先 `::EmptyClipboard()` 再 `::SetClipboardData(CF_UNICODETEXT, …)`
- `win32` 已在 lock：`pubspec.lock` 的 `win32 6.4.0`（transitive）+ 本机 pub 缓存里的
  `enums.g.dart:1097`(`CF_HDROP`) / `structs.g.dart:6698`(`DROPFILES`) / `user32.g.dart`
- 无原生通道 / 无拖拽：`windows/runner/flutter_window.cpp:21-28`（只有 `RegisterPlugins`，`:27`）、
  `windows/runner/CMakeLists.txt:9-17, 35-36`、`lib/core/run_event_source.dart:331`、
  `android/app/src/main/kotlin/com/comfyhub/harness/bridge/HarnessBridge.kt:91-92, 138-250, 746, 749`、
  `android/app/src/main/kotlin/com/example/viewer/MainActivity.kt:24-29`、`pubspec.yaml:10-58`
- 绝对路径问题：`server-rs/src/web/models.rs:215, 275, 314`、`server-rs/src/main.rs:260`、
  `server-rs/src/media/media.rs:352-359`、`lib/core/backend_paths.dart:104-126, 218-219`、
  `lib/core/backend_launcher.dart:366, 378`、`lib/models/models.dart:379-467`
- Android 无 FileProvider：`android/app/src/main/AndroidManifest.xml`；`lib/` 下 `cacheDir`/`getTemporaryDirectory` 零命中
