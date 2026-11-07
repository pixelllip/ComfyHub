/// **宽屏 / 窄屏的分界**（逻辑像素）—— 全 App 只有这一个数字。
///
/// 它同时决定三件事，改它之前先想清楚这三件都要一起动：
///   1. 导航形态：≥ 它用左侧 `NavigationRail`（常驻），< 它用底部 `NavigationBar`；
///   2. 次级页的落点：≥ 它只替换内容区（侧边栏还在），< 它落在根 Navigator 上**铺满整屏**
///      （见 `app.dart` 的 `_ContentNavigator`）；
///   3. 窗口最小宽度是 360（`windows\runner\win32_window.cpp`），所以 360~900 这一段
///      就是"手机形态"要扛住的全部宽度。
const double kWideLayoutMinWidth = 900;
