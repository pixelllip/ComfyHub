/// **宽屏 / 窄屏的分界**（逻辑像素）—— 全 App 只有这一个数字。
///
/// 它同时决定三件事，改它之前先想清楚这三件都要一起动：
///   1. 导航形态：≥ 它用左侧 `NavigationRail`（常驻），< 它用底部 `NavigationBar`；
///   2. 次级页的落点：≥ 它只替换内容区（侧边栏还在），< 它落在根 Navigator 上**铺满整屏**
///      （见 `app.dart` 的 `_ContentNavigator`）；
///   3. 窗口最小宽度是 360（`windows\runner\win32_window.cpp`），所以 360~900 这一段
///      就是"手机形态"要扛住的全部宽度。
const double kWideLayoutMinWidth = 900;

/// AI 工作台左右两栏的**默认宽度**（逻辑像素，也就是可拖动的最小宽度）。
///
/// 这两个值就是"拖动出现之前"的固定宽度，作为下限使用；上限见
/// [kAiSidePaneMaxWidth]，中栏保底见 [kAiCenterMinWidth]。
const double kAiConversationPaneWidth = 240;
const double kAiContextPaneWidth = 260;

/// 左右两栏拖动后能到的最宽宽度。
const double kAiSidePaneMaxWidth = 480;

/// 拖动时中栏（消息 + 输入）必须保留下来的宽度。
///
/// 没有这条，两栏都拖到最宽时中栏会被压成一条缝 —— 窗口本身没有这个限制，
/// 只能在这里挡住。
const double kAiCenterMinWidth = 420;

/// 左右两栏拖动边界的命中宽度（逻辑像素）。
const double kAiResizeHandleWidth = 8;
