# 界面规范（布局 / 列表 / 文案 / 翻页 / 标题联动）

> 这是 `AGENTS.md` 第 4 节「前端规范」的细则。全局红线见 [`../AGENTS.md`](../AGENTS.md)；现象 → 原因 → 处理见 [`pitfalls.md`](pitfalls.md)。
> 每节末尾的回归用例是防退化的最低防线，改动相关区域时必须跑。

## 1. 导航与首页

`lib/app.dart` 的 `HomeShell` 默认落在「AI 工作台」，顺序 `AI 工作台 → 画廊 → 提示词 → 标签 → 设置`；`_destinations`（图标 / 文案）与 `pages` **必须同序**（`_index` 同时索引它们）。回归 `test\home_nav_test.dart`；`test\localization_test.dart` 也用首页做锚点。

一级导航**只有 5 项**：窄屏上第 6 个图标会挤成两行，所以新功能优先做成**次级页**或挂进已有页面（角色卡 / 画风卡的「卡片库」就是这么挂在「预设库」页的）。

## 2. 两端一起设计

任何界面改动都要在**桌面端和移动端**各问一句"这好用吗"。形态由**一个数字**划开：`lib/core/layout_constants.dart` 的 `kWideLayoutMinWidth = 900`（逻辑像素）。

| | 桌面端（≥ 900） | 移动端（< 900） |
| --- | --- | --- |
| 导航 | 左侧 `NavigationRail`，**常驻** | 底部 `NavigationBar` |
| 次级页（详情 / 结果页…） | **只替换右边内容区**，侧边栏一个字不动 | **铺满整个界面**（底边栏一起盖住） |
| 卡片 | 宽了分列（见 §3） | 窄了退回单列，**360 宽不许溢出** |

- **次级页的落点按宽度分**：`_ContentNavigator` —— 宽屏套一层内容区 `Navigator`，窄屏**不套**、让 `Navigator.of(context).push` 落到 `MaterialApp` 的根 Navigator 上。回归 `test\content_navigator_test.dart`（宽窄两组都在）。
- **窗口最小宽度 360 逻辑像素**（`windows\runner\win32_window.cpp` 的 `kMinWindowWidthLogical`），按 DPI 换算。
- **每个断点只有一份真源**：宽度阈值只有 `kWideLayoutMinWidth`，别再在页面里写 900 / 360。

## 3. 列表与卡片

- **列表一律用 `lib/widgets/adaptive_layout.dart` 的多列组件**，别自己写单列 `ListView`：高度接近的用 `AdaptiveColumnList`，高度差几倍的用 `AdaptiveColumns`；列数 = 可用宽度 / 550（最多 4 列）。回归 `test/adaptive_layout_test.dart`。
- **卡片宽度按"里面装了多少东西"定**，别一律 550：内容少的（标签卡：名字 + 一行说明 + 计数 + 两个按钮）用 `minColumnWidth: kColumnWidth / 2`（275）。回归 `test\tags_layout_test.dart`（宽屏分列与窄屏单列两种形态都钉了）。
- **长列表必须懒构建**，别用 `children: [for (...) ...]`：模型卡片、聊天气泡这类一律 `ListView.builder`；流式刷新（逐字回包）时每项包 `RepaintBoundary`。`header` 只放**固定几条**，要按状态拼的用 `switch` / `if` 分支 —— 拼回 `children` 懒构建就失效了。
- **高矮差大的长列表用「固定行高 + 编辑弹窗」**，别让每行自己撑高：懒构建的 `ListView` / `SliverList` 只能用平均高度估 `maxScrollExtent`，滚动条滑块会越滚越短。正解是 `CustomScrollView` + `SliverFixedExtentList(itemExtent: 常数)`，行的详细编辑搬进 `showDialog`；代价是每行高度恒定，**别在行里塞会换行的 `Wrap`**。回归 `test\model_list_scroll_test.dart`。
- **大块状态不许占页顶**（用户口径：这块占得太大）：与当前选中项无关的大块内容做成折叠项或详情内的一节，别做常驻横幅。回归 `test\phone_layout_overflow_test.dart`（384 / 320dp 两档都不许溢出）。
- **工作台的模型选择器按 Provider 分组**：`SliverToBoxAdapter(来源标注)` + `SliverPadding(SliverList(模型))` 一组一组。数据源是 `AiWorkspaceStore.catalogByProvider`（打开选择器时才拉），**不是** `store.models`；换 Provider 走 `selectModelFrom(p, m)` —— 别拆成"先 `selectProvider` 再 `selectModel`"，那两步都是 `await`，中间那次选中会被 `_loadModels()` 收尾时的"取第一个启用的"覆盖。回归 `test\ai_model_picker_test.dart`。
- **内置模型目录那张卡与它的 Dart 客户端已删**：能力预填改由 **models.dev** 在线目录负责（`capability_refresh.rs`）。后端 `server-rs\src\provider\builtin.rs` 与 `/api/ai/builtin/{status,sync}` 还在（Rust 用例盯着），但界面上**没有任何入口** —— 别再把 `AiBuiltinCatalogStatus` / `builtinStatus()` 加回来。
- **手机「从电脑同步 Provider」是"用户显式点的一次导入"，不是自动合并**（`lib/core/provider_sync.dart`，契约 [`pairing-design.md`](pairing-design.md) §14.6）：桌面有 / 手机没有 → 新建；同 id → 桌面覆盖（提交的是**本机** `revision`，两端的库各算各的）；只在手机上的 → **保留不删**；模型目录**只在真的拿到时才写**（拉取失败绝不许回落到空目录）；`api` 不在手机那三种协议里的**跳过并如实说**。按钮**只在手机上渲染**。⚠ 这一页**不要自己去读配对令牌**（`PairingTokenStore`）—— `AiApiClient` 默认取进程内缓存即可；硬读法在真机之外的宿主上会悬着不返回（表现是"点了同步永远转圈"，2026-11-01 踩过）。回归 `test\provider_sync_test.dart`。
- **手机端的落盘缓存必须有上限，而且必须看得见、清得掉**（用户 2026-11-01 问"要不要设缓存上限" → 要）。现在唯一一个是 `lib/core/media_cache.dart`：缩略图 150MB、原件默认 1.5GB（设置页「产物缓存」可调 512MB~4GB），超过按**最久未用**淘汰到 80%；打开详情时**超过 200MB 的视频 / 音频不自动下**，要存到手机是显式动作；保存进相册后大文件在离开页面时回收缓存副本。设置页那一节用 `lib/widgets/media_cache_card.dart`（独立 widget 才测得到，它只在手机上渲染），320 / 384dp 两个宽度都有用例。⚠ 落点在 `files/`（不是 `cacheDir`）：系统不会替你清，所以**预算 + 淘汰 + 清空入口**三件缺一不可；`android/app/src/main/res/xml/data_extraction_rules.xml` 把 `media-cache/` 排除出 Auto Backup（25MB 配额，注定失败）。回归 `test\media_cache_test.dart` + `test\media_cache_card_test.dart`。
- 开关行别直接 `SwitchListTile(contentPadding: EdgeInsets.zero)` 贴卡片边缘：用 `settings_page.dart` 的 `_SwitchRow`（自带内边距 + Material 底色；底色不能用 `Container` 的 decoration，否则盖掉水波纹并触发断言）。

## 4. 文案

适用于界面文案（标题 / 副标题 / 空状态 / tooltip / SnackBar / 错误提示，以及后端回给界面的那一句），代码注释同口径。**2026-11-05 定下的硬约束**。

措辞四条：

- **严谨**：一句只表达一个事实，条件、范围、数量、单位写全（"超过 200MB 不自动下载"，不写"大了就不下"）；不夸大、不绝对化，同一概念全篇用同一个词。
- **通俗**：非本项目的读者也能读懂，短句优先，不省略主语和条件。
- **禁止口头化**：不用语气词、网络用语与梗、夸张和比喻、反问句、颜文字；不写对话体、第一人称叙事和开发过程吐槽。
- **注释只陈述现象和约束**：写清条件与边界，不写原因和归因，注释里不出现"用户"二字；不复述代码字面，不用"这里""上面那个"这类指代。

格式与渲染再按下面几条：

- **一句话说完，不加从句、不套括号**：先给结论再给条件，别写"当……时，如果……则……"；也不许出现 `a:b` / `c;d` 这种话术（`集合详情：宽屏`、`当前模型：X`、`允许 = 直接执行`、`A；B` 都不行）。一句话装不下就拆成两句，别拿分号硬连。
- **用用户熟悉的名词，少用专有名词**：模型、图片、画廊、设置、服务、缓存；别把类名、字段名、内部代号（`run-kit`、`WidgetsBinding`、`AiWorkspaceStore`）直接甩到界面上。必须提的时候，先用一句人话解释它是什么。
- **错误提示同样说人话**：`保存失败，磁盘空间不够。`，不要 `保存失败：ENOSPC`；不要把异常栈 / 内部错误码当正文。
- **先确认渲染器再写格式**：只有会进 `MarkdownText`（聊天正文、蓝图详情）的字符串才能用 Markdown；进 `Text` 的按纯文本写 —— `Text` / `SelectableText` / `SnackBar` / `InputDecoration` 的 label / hint / helper 都不解析 Markdown，`**粗体**`、反引号、`# 标题`、`- 列表` 会原样显示，工具卡结果区与后端回给界面的字符串同理。要强调就用 `Text.rich` / `TextSpan`（`fontWeight: FontWeight.bold`），要排版就用 `MarkdownText`。
- **例外只有字面量**：URL、比例（`16:9`）、代码 / 路径 / 键名，按原样写。
- **改文案要同步改测试断言**（不少用例直接 `find.text('……')`），改完跑 `flutter analyze` + `flutter test`；后端回给界面的那句也要一起改，别只改前半句。

## 5. 界面语言只有一种

App 文案本来就是中文，但文本框选择菜单、返回按钮 tooltip 这类**系统文案来自 `MaterialLocalizations`** —— `lib/app.dart` 必须留着 `locale: zh_CN` + `GlobalMaterialLocalizations.delegates`（Flutter 3.50 起来自 `material_ui`，已包含 Cupertino 与 Widgets 两套），少一个就冒英文 "Copy / Select all"。用例 `test/localization_test.dart`。

## 6. 弹出菜单

统一用 `AppMenuButton`（按钮下拉）/ `showAppContextMenu`（右键），页面根部挂 `ContextMenuScope`（`lib/widgets/app_menu.dart` + `context_menu.dart`，`common.dart` 里转出）。**别改回 `PopupMenuButton` / `showMenu`**：它们铺的 `ModalBarrier` 会把整页滚轮和拖动全吃掉（上游 flutter/flutter#90223）。回归 `test/menu_scroll_test.dart`。传 `context` 要用**在 `ContextMenuScope` 里面**的那一个。**弹窗里的右键要自己再挂一个 scope**：对话框在根 Navigator 的 overlay 里，不在页面的 scope 之下（找不到宿主会直接抛）；附件详情就是这么做的（`attachment_preview.dart` + `zoomable_image_view.dart` 的图片弹窗，回归 `test/ai_attachment_preview_test.dart`）。

## 7. 大标题与顶栏标题的滚动联动

**2026-11-05 定下的硬约束**：页面顶部有**大标题**时（设置家族、画廊、预设、标签……），这个标题属于**主体内容**；它随内容滚出可视区时，**同名标题要在顶栏里逐渐出现**，滚回顶部时再退回去。大标题滚走而顶栏什么都不显示、或顶栏标题与正文大标题同时出现，都不允许。

**按平台分两套**（用户 2026-11-05 口径）：

- 判据是 `useCollapsingPageTitle`（`scroll_linked_title.dart`：Android / iOS 为真），**不是窗口宽度** —— 电脑窗口拉窄到 900 以下仍是电脑端，不该长成手机的样子；
- **电脑端（Windows / macOS / Linux）**：标题**始终在 AppBar 里**，正文不再放大标题，滚动也不做标题动画；
- **手机端（Android / iOS）**：才用大标题 + 空白 AppBar + 滚动淡入那一套；
- **返回键一律在 AppBar 里**（用户 2026-11-05 口径），用带外圈的 40×40 圆形 `BackButton`；正文的页头不再放返回键。

组件只有一份：`lib/widgets/scroll_linked_title.dart`

- `ScrollTitleProgress`：进度的唯一真源（0 = 大标题完整可见，1 = 完全离开）；
- `ScrollTitleListener`：把**最外层**滚动（`depth == 0`）换算成进度；
- `CollapsingPageTitle`：主体里的大标题，随滚动上移 + 淡出；
- `FadingAppBarTitle`：有 `AppBar` 的页面用它；进度为 0 时**不建**（避免两个同名标题）；
- `FadingTopBar`：真正**没有 `AppBar`** 的页面才用它（浮层、不占布局高度）。

纪律：

- **设置家族（`SettingsScaffold`）必须预留一条空白 AppBar**（用户 2026-11-05 口径）：大标题在正文里滚走，同名标题在这条 AppBar 里淡入，返回键也跟着一起出现；**不要用浮层压住正文**。
- 接入新页面时**只接这一套**，不要再手写 `Opacity` / `setState` 各一套。
- 标题块高度可以按页面微调（画廊 44，其余默认 56），但**判据只能是 `ScrollTitleProgress`**，不要另算一套阈值。
- 回归：`test/scroll_linked_title_test.dart`（顶部只有大标题 / 滚动后顶栏出现 / 滚回顶部收回）。

## 8. 翻页

**2026-11-07 定下的硬约束**（用户："过渡动画会同时显示新旧两页的内容，动画结束后注销旧界面"）：

- 所有"上一页 / 下一页"统一用 `lib/widgets/paged_swipe.dart` 的 `SwipePageView`，**不要**再写 `AnimatedSwitcher` 那种"旧页淡出、新页淡入"的替换器；
- `SwipePageView` 是"目标页码 + `builder(page)`"：拖动时当前页和相邻页**同时在树里**平移（拖到一半就是左右各半页），松手过阈值才提交页码；**邻居页的数据要先备好**，否则 `builder` 返回 `null`，它会保持旧页并请求 `onPageNeeded`；
- 后端分页列表（画廊 / 预设）的相邻页缓存由 `LibraryStore` 负责：`mediaPageData` / `promptPageData` 取缓存，`ensureMediaPage` / `ensurePromptPage` 预取；改筛选 / 排序 / 删除后必须让缓存失效（`_invalidate*Pages`）；
- 大图查看器在 **1:1 时**把横向拖动交给 `SwipePageController`（`ZoomableImageView` 用 `Listener` 读原始指针，绕开 `InteractiveViewer` 的手势竞技场）；放大之后横向拖动只平移画面，不翻页；
- **桌面画廊关掉滑动翻页**（`SwipePageView.swipeEnabled: !_mouseFirst`）：格子本身可以直接拖去组成集合，横向拖动会被格子先抢走。桌面用 `PageBar` 的显式「上一页 / 下一页」（≥520 宽带文字，360 宽仍是图标），手机长按才拖格子，滑动翻页保持开启；
- 详情页的同批产物必须在 `_load` 时把 `listMedia(collectionId: …)` 返回的**完整 `MediaAsset`** 收进 `_siblingMedia`，否则拖动开始时凑不出相邻那一页；
- 回归：`test/swipe_pager_test.dart`（拖到一半两页都在 / 数据没到不换页）、`test/gallery_paging_test.dart`（相邻页缓存）、`test/media_detail_narrow_test.dart`（看图器拖动翻页 / 动画中两个查看器、结束后只剩一个）。

## 9. 流畅度

改动落在"可能影响流畅度"的那几类时，必须在真跑起来的应用上量帧耗时，判据是长帧而不是平均帧率。工具、接线顺序与回落路径见 [`perf-measurement.md`](perf-measurement.md)。
