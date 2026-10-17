import 'dart:io' show Platform;

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

/// 界面外观的三个选项（真源在 `SettingsStore.themeMode`，偏好键 `comfyhub.themeMode`）。
///
/// 它**不持有状态**：只描述"用户选了什么"，由 [material] 翻成 `MaterialApp.themeMode`。
/// 默认 [dark] —— 2026-10-13 之前 App 只有这一套外观（`themeMode` 写死），
/// 改默认值等于给所有存量用户换一套配色，所以**别动它**。
///
/// ⚠ 浅色不是"把 brightness 一改就完事"：界面里压在图片 / 视频上的白字黑底
/// （缩略图渐变、图片查看器）在两种主题下都成立，但**占位图标**那种
/// "白图标画在 surface 上"的写法在浅色下会消失 —— 加浅色时那些点要一起看
/// （`media_thumb` / `collection_thumb` 的占位已经改成 `onSurfaceVariant`）。
enum AppThemeMode {
  dark('深色'),
  light('浅色'),
  system('跟随系统');

  final String label;

  const AppThemeMode(this.label);

  ThemeMode get material => switch (this) {
        AppThemeMode.dark => ThemeMode.dark,
        AppThemeMode.light => ThemeMode.light,
        AppThemeMode.system => ThemeMode.system,
      };

  /// 读偏好。认不出来的（老版本写的、用户手改过的）一律回默认，**绝不抛** ——
  /// 它在 App 启动路径上，崩了就是白屏。
  static AppThemeMode parse(String? raw) {
    for (final mode in values) {
      if (mode.name == raw) return mode;
    }
    return AppThemeMode.dark;
  }
}

/// ComfyHub 主题。
///
/// ## 为什么要单独定制字体
///
/// Flutter 在 Windows 上的默认字体族是 **Segoe UI**，它**不含中文字形**，
/// 中文只能靠引擎的系统兜底去挑一个字体。这会带来两个后果：
///   1. 中英混排时字形风格不统一，中文明显偏细、发虚；
///   2. Material 3 的默认字号 / 字距是按 Roboto（拉丁字体）调的
///      （bodyMedium 14px + letterSpacing 0.25），套到方块字上会显得又小又挤。
///
/// 所以这里做三件事：
///   · 显式指定中文字体族，并按平台给一串兜底；
///   · 把字距归零、行高放到 1.5~1.6（中文需要更大的行距才不挤）；
///   · 小字号统一提高一档字重，让笔画在低分屏上也立得住。
class AppTheme {
  AppTheme._();

  /// 中文兜底字体链。按优先级排列，缺哪个会自动往后找。
  static const List<String> cjkFallback = <String>[
    'Microsoft YaHei UI', // Windows 11 默认中文 UI 字体
    'Microsoft YaHei',    // Windows 中文正文字体
    'PingFang SC',        // macOS / iOS
    'Hiragino Sans GB',   // 老版本 macOS
    'Noto Sans CJK SC',   // Linux / Android 常见
    'Source Han Sans SC', // 思源黑体
    'WenQuanYi Micro Hei',// Linux
    'SimHei',
    'sans-serif',
  ];

  /// 各平台首选字体。返回 null 表示交给系统默认（Android / iOS / Web 自带的中文都不错）。
  static String? primaryFontFamily() {
    if (kIsWeb) return null;
    try {
      if (Platform.isWindows) return 'Microsoft YaHei UI';
      if (Platform.isLinux) return 'Noto Sans CJK SC';
    } catch (_) {
      // 平台判断在个别环境会抛异常，忽略
    }
    return null;
  }

  static ThemeData build(Brightness brightness) {
    final scheme = ColorScheme.fromSeed(
      seedColor: const Color(0xFF7C5CFF),
      brightness: brightness,
    );

    final base = ThemeData(useMaterial3: true, colorScheme: scheme);
    final family = primaryFontFamily();

    // -------------------------------------------------------------------------
    //  两枚"全局色"—— 后面每个组件都用它们，别再各写各的 alpha
    //
    //   · [line] —— 分隔线 / 卡片描边 / 菜单描边。`outlineVariant` 直接当线用
    //     在深色主题下会"抢"内容的注意力，压到 0.55 才是"看得见但不打扰"。
    //   · [fill] —— 输入框、分组容器那种"比页面底色高一档"的填充。
    //
    //  ⚠ **层次一律用色彩表达，不用 `elevation`**：阴影是每帧都要重绘的一笔，
    //    而卡片 / 缩略图在画廊、预设、标签页里是几十上百份叠在一起。
    //    M3 的 `surfaceContainer*` 系列就是为这件事准备的 —— 本文件里所有
    //    elevation 都归零、并配 `surfaceTintColor: transparent`（M3 默认会按
    //    elevation 把 primary 叠上去，那才是"越浮越紫"的来源），原因都在这。
    // -------------------------------------------------------------------------
    final line = scheme.outlineVariant.withValues(alpha: 0.55);
    final fill = scheme.surfaceContainerHighest.withValues(alpha: 0.35);

    return base.copyWith(
      textTheme: _tune(base.textTheme, family, scheme.onSurface),
      // `primaryTextTheme` 是"画在主色底上"的那一套（AppBar 反过来的时候用它），
      // 所以兜底色是 `onPrimary` 而不是 `onSurface`。
      primaryTextTheme: _tune(base.primaryTextTheme, family, scheme.onPrimary),
      visualDensity: VisualDensity.standard,

      // -------------------------------------------------------------------------
      //  顶栏：**唯一一处"页面骨架"级的样式**
      //
      //  以前 9 个页面的 `AppBar` 各用 M3 默认值，于是三件事各不相同：
      //    · 滚动内容时顶栏被 `surfaceTint` 叠一层 primary —— 深色主题下就是
      //      一条紫灰色的"脏带"，而且**只在滚动时**出现，最难查；
      //    · 只有 5 个页面自己写了 `bottom: PreferredSize(Divider)` 当底边线
      //      （画廊 / 预设 / 标签 / 角色卡 / 设置），其余页面没有 ——
      //      换页时"内容区从哪儿开始"不一致；
      //    · 标题字号走 M3 的 titleLarge，与主题里调过的中文排版不是一套。
      //
      //  统一到主题之后那 5 处手写已删：底边线改由 [shape] 画 ——
      //  `shape` 的描边画在 AppBar **自己的边界内**，所以总高比"预留 1px"
      //  还少 1px，不会把窄屏页面顶溢出（320dp 那几条用例盯着这个）。
      // -------------------------------------------------------------------------
      appBarTheme: AppBarThemeData(
        backgroundColor: scheme.surface,
        foregroundColor: scheme.onSurface,
        elevation: 0,
        // 滚动时不许变色：M3 的默认行为在深色主题下是一条会跟着滚动出现的脏带
        scrolledUnderElevation: 0,
        surfaceTintColor: Colors.transparent,
        centerTitle: false,
        titleTextStyle: _ui(19, FontWeight.w700, family, height: 1.35, color: scheme.onSurface),
        shape: Border(bottom: BorderSide(color: line)),
      ),

      // 分隔线：给一个默认色，页面里的 `Divider()` 不用再逐个传 color。
      // ⚠ 不设 `space`：各处都显式写了 `height: 1 / 20`，设了反而把显式的顶掉。
      dividerTheme: DividerThemeData(color: line, thickness: 1),

      // TabBar（标签结果页）：文字样式与主题一致，指示条用主色，
      // 底边线与 AppBar 那条同一个颜色 —— 看着才是"一条线"而不是两条。
      tabBarTheme: TabBarThemeData(
        labelStyle: _ui(14, FontWeight.w700, family, color: scheme.onSurface),
        unselectedLabelStyle:
            _ui(14, FontWeight.w500, family, color: scheme.onSurfaceVariant),
        labelColor: scheme.onSurface,
        unselectedLabelColor: scheme.onSurfaceVariant,
        indicatorColor: scheme.primary,
        dividerColor: line,
      ),

      cardTheme: CardThemeData(
        elevation: 0,
        margin: EdgeInsets.zero,
        surfaceTintColor: Colors.transparent,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(14),
          side: BorderSide(color: line),
        ),
      ),
      inputDecorationTheme: InputDecorationTheme(
        filled: true,
        fillColor: fill,
        border: OutlineInputBorder(
          borderRadius: BorderRadius.circular(10),
          borderSide: BorderSide.none,
        ),
        isDense: true,
        contentPadding: const EdgeInsets.symmetric(horizontal: 12, vertical: 12),
      ),
      // 按钮：只统一圆角，**不动尺寸**（`minimumSize` / `padding` 一动，
      // 320dp 那几页的 `Wrap` 折行位置就变了，属于"为了好看把布局改坏"）。
      // 文字样式不用管：M3 的按钮走 `labelLarge`，已经在 [_tune] 里调过。
      filledButtonTheme: FilledButtonThemeData(style: _buttonStyle()),
      outlinedButtonTheme: OutlinedButtonThemeData(style: _buttonStyle()),
      textButtonTheme: TextButtonThemeData(style: _buttonStyle()),
      iconButtonTheme: IconButtonThemeData(
        style: ButtonStyle(
          // 图标按钮的高亮块跟着卡片一起圆角（默认是圆形，压在方角卡片上很跳）
          shape: const WidgetStatePropertyAll(
            RoundedRectangleBorder(borderRadius: BorderRadius.all(Radius.circular(10))),
          ),
        ),
      ),
      snackBarTheme: SnackBarThemeData(
        behavior: SnackBarBehavior.floating,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(10)),
        elevation: 0,
        // ⚠ 别设 `insetPadding`：`copyToClipboard` 那类定点提示传了 `width`，
        // 而 SnackBar 对"width + 主题里的 insetPadding"有断言（见 SDK 的
        // `assert(width == null || margin == null)`）。默认值本来就是对称的。
      ),
      // 对话框：与卡片同一套语言（描边分层、无阴影）。M3 默认是 elevation 6
      // 的投影 —— 一次 `showDialog` 就要为整个对话框blur 一遍，而且和卡片
      // "零海拔 + 一圈描边"的观感明显不是一套。
      dialogTheme: DialogThemeData(
        backgroundColor: scheme.surfaceContainerHigh,
        surfaceTintColor: Colors.transparent,
        elevation: 0,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(16),
          side: BorderSide(color: line),
        ),
      ),
      // 弹出菜单：`AppMenuButton` / 右键菜单都走 `MenuAnchor`（见 widgets/app_menu.dart），
      // 所以这里是**唯一**管到它们的入口 —— 圆角、底色、描边一次到位。
      // 菜单是"浮在内容之上"的少数派，这里保留一层很轻的投影（transient、
      // 一次只有一个），不像列表项那样是每帧 N 份。
      menuTheme: MenuThemeData(style: _menuStyle(scheme, line)),
      // 下拉选择框（Provider 详情页那两处 `DropdownButtonFormField`）
      dropdownMenuTheme: DropdownMenuThemeData(menuStyle: _menuStyle(scheme, line)),
      // 进度条 / 转圈：与主色一致，轨道压淡（默认轨道在深色下几乎和背景融为一体）
      progressIndicatorTheme: ProgressIndicatorThemeData(
        color: scheme.primary,
        linearTrackColor: scheme.surfaceContainerHighest,
        circularTrackColor: scheme.surfaceContainerHighest,
      ),
      // 文本框里的选中色：默认是 primary@0.4，方块字上偏重
      textSelectionTheme: TextSelectionThemeData(
        cursorColor: scheme.primary,
        selectionColor: scheme.primary.withValues(alpha: 0.3),
        selectionHandleColor: scheme.primary,
      ),
      listTileTheme: const ListTileThemeData(
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.all(Radius.circular(10)),
        ),
      ),
      chipTheme: base.chipTheme.copyWith(
        // ⚠ 这两条**必须带颜色**：`ChipThemeData.labelStyle` 一旦非空就会**整个**
        // 顶掉 M3 的默认（默认里带着 onSurfaceVariant / onSecondaryContainer）。
        // 原来只写了字体没写颜色 —— 深色主题下白字看着正常，浅色主题下就是
        // "未选中的 chip 只剩一个空框、字看不见"（就是那种"只有小字有问题"的表象）。
        labelStyle: TextStyle(
          fontFamily: family,
          fontFamilyFallback: cjkFallback,
          fontSize: 13,
          fontWeight: FontWeight.w500,
          letterSpacing: 0,
          color: scheme.onSurfaceVariant,
        ),
        // 选中态（ChoiceChip / FilterChip）用的是 secondaryLabelStyle
        secondaryLabelStyle: TextStyle(
          fontFamily: family,
          fontFamilyFallback: cjkFallback,
          fontSize: 13,
          fontWeight: FontWeight.w600,
          letterSpacing: 0,
          color: scheme.onSecondaryContainer,
        ),
      ),
      // -------------------------------------------------------------------------
      //  导航：侧边栏与内容区**分层**（侧边栏低一档 / 选中的那一项实心胶囊）
      //
      //  以前侧边栏与内容区是同一个 surface 色，只在中间放了一条 `VerticalDivider`，
      //  "哪儿是导航、哪儿是内容"全靠那条线；选中态也只有一个淡色底。
      //  `surfaceContainerLow` + `secondaryContainer` 是 M3 给这两件事的答案，
      //  且都是**纯色填充**（零阴影、零重绘开销）。
      //
      //  ⚠ 宽度 / 高度一个字都不动：`home_nav_test` 拿 `NavigationRail` 的矩形
      //  当"换页不重建"的判据（逐像素相同），改尺寸会让那条用例失去意义。
      // -------------------------------------------------------------------------
      navigationRailTheme: NavigationRailThemeData(
        backgroundColor: scheme.surfaceContainerLow,
        labelType: NavigationRailLabelType.all,
        useIndicator: true,
        indicatorColor: scheme.secondaryContainer,
        indicatorShape: const RoundedRectangleBorder(
          borderRadius: BorderRadius.all(Radius.circular(14)),
        ),
        selectedIconTheme: IconThemeData(color: scheme.onSecondaryContainer, size: 24),
        unselectedIconTheme: IconThemeData(color: scheme.onSurfaceVariant, size: 24),
        selectedLabelTextStyle: TextStyle(
          fontFamily: family,
          fontFamilyFallback: cjkFallback,
          fontSize: 12.5,
          fontWeight: FontWeight.w700,
          letterSpacing: 0,
          color: scheme.onSurface,
        ),
        unselectedLabelTextStyle: TextStyle(
          fontFamily: family,
          fontFamilyFallback: cjkFallback,
          fontSize: 12.5,
          fontWeight: FontWeight.w500,
          letterSpacing: 0,
          color: scheme.onSurfaceVariant,
        ),
      ),
      navigationBarTheme: NavigationBarThemeData(
        backgroundColor: scheme.surfaceContainerLow,
        surfaceTintColor: Colors.transparent,
        elevation: 0,
        indicatorColor: scheme.secondaryContainer,
        indicatorShape: const RoundedRectangleBorder(
          borderRadius: BorderRadius.all(Radius.circular(14)),
        ),
        // ⚠ 用 `resolveWith` 而不是 `WidgetStatePropertyAll`：M3 的默认值本来就是
        // "选中 onSurface / 未选中 onSurfaceVariant"，而**只要我们给了
        // `labelTextStyle`，默认值就整个不生效** —— 给一个不分状态、又没有颜色的
        // 样式，等于把底部导航的文案一起变成白色（浅色主题下直接看不见）。
        labelTextStyle: WidgetStateProperty.resolveWith((states) {
          final disabled = states.contains(WidgetState.disabled);
          final selected = states.contains(WidgetState.selected);
          return TextStyle(
            fontFamily: family,
            fontFamilyFallback: cjkFallback,
            fontSize: 12.5,
            fontWeight: FontWeight.w600,
            letterSpacing: 0,
            color: disabled
                ? scheme.onSurfaceVariant.withValues(alpha: 0.38)
                : (selected ? scheme.onSurface : scheme.onSurfaceVariant),
          );
        }),
      ),
      // 滚动条**贴边**（用户 bug 2026-09-20："AI 工作台的聊天详情，滚动条没有贴边"）。
      //
      // M3 默认给滚动条留了内边距（轨道离视口右边 8px 左右、滑块本身还有一圈留白），
      // 在聊天这种"内容铺到边缘"的页面上看着就是滑块**悬在离边老远的地方**。
      // 这里把两处留白都收掉：
      //   · `thickness` 是滑块本体宽度；
      //   · `crossAxisMargin` 是轨道与视口边缘的距离（默认非 0，就是"没贴边"的元凶）。
      // 全局设一次，聊天 / 画廊 / 提示词列表一起对齐，不用逐个页面写 `Scrollbar`。
      scrollbarTheme: ScrollbarThemeData(
        thickness: const WidgetStatePropertyAll(10.0),
        radius: const Radius.circular(5),
        crossAxisMargin: 0,
      ),
      tooltipTheme: base.tooltipTheme.copyWith(
        textStyle: TextStyle(
          fontFamily: family,
          fontFamilyFallback: cjkFallback,
          fontSize: 12.5,
          letterSpacing: 0,
          color: scheme.onInverseSurface,
        ),
      ),
    );
  }

  /// **主题里任何一处"组件级文案"都用它**（AppBar 标题 / Tab 标签 / 菜单…）。
  ///
  /// 与 [_tune] 里那个局部 `style` 是同一套写法，区别只是它要能被别的组件
  /// 复用：**中文字体链 + 字距归零**这两件事必须处处一致，漏一处就是
  /// "某个角落的字看着和别处不是一个字体"。
  /// ⚠ [color] 是**必填**的：这里的样式都是拿去**替换 M3 默认值**的，不给颜色
  /// 就等于把默认值里那个颜色覆盖成"没有"，而"没有颜色"会被引擎画成白色
  /// （见 [_tune] 的说明）。写成必填，是为了让"漏给颜色"变成编译错误。
  static TextStyle _ui(
    double size,
    FontWeight weight,
    String? family, {
    required Color color,
    double height = 1.4,
  }) =>
      TextStyle(
        fontFamily: family,
        fontFamilyFallback: cjkFallback,
        fontSize: size,
        fontWeight: weight,
        height: height,
        letterSpacing: 0,
        color: color,
      );

  /// 按钮样式：**只统一圆角**。
  ///
  /// ⚠ 刻意不碰 `minimumSize` / `padding` / `tapTargetSize`：那些一动，
  /// 窄屏页面里 `Wrap` 的折行位置就变了（设置页「保存并测试连接」那一排
  /// 在 320dp 上本来就是贴着实测溢出 7.1px 的边缘修的）。
  /// 也不碰 `side`：M3 的描边色本来就来自 `outlineVariant`，而"禁用态要淡化"
  /// 那件事只有默认实现做对了 —— 用 `WidgetStatePropertyAll` 覆盖会把禁用态
  /// 也画成实边。
  static ButtonStyle _buttonStyle() => const ButtonStyle(
        shape: WidgetStatePropertyAll(
          RoundedRectangleBorder(borderRadius: BorderRadius.all(Radius.circular(10))),
        ),
      );

  /// 弹出菜单样式（`AppMenuButton` / 右键菜单 / `DropdownButtonFormField` 共用）。
  ///
  /// 一层很轻的投影是**故意留的**：菜单必须"浮"在内容之上，而它同时只会存在
  /// 一个 —— 与列表项那种每帧 N 份的阴影不是一回事。底色用 `surfaceContainerHigh`
  /// （比卡片再高一档），配合描边，明暗两套主题下都立得住。
  static MenuStyle _menuStyle(ColorScheme scheme, Color line) => MenuStyle(
        backgroundColor: WidgetStatePropertyAll(scheme.surfaceContainerHigh),
        surfaceTintColor: const WidgetStatePropertyAll(Colors.transparent),
        shadowColor: WidgetStatePropertyAll(Colors.black.withValues(alpha: 0.35)),
        elevation: const WidgetStatePropertyAll(6),
        shape: WidgetStatePropertyAll(
          RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(12),
            side: BorderSide(color: line),
          ),
        ),
      );

  /// 把 Material 3 的默认排版改成"中文友好"版本。
  ///
  /// ## ⚠ 每个样式**都必须落到一个具体颜色**（[fallback]）
  ///
  /// 这是 2026-10-13 加浅色主题时才暴露出来的坑，务必别再踩：
  ///
  /// 这一版以前是 `color: color`（调用点不传 = `null`）。**`TextStyle.color` 为空
  /// 不等于"继承"** —— Flutter 到 Skia 那一段没有颜色可用时**按白色画**。深色主题
  /// 下这看着完全正常（白字），所以它藏了很久；一切到浅色，凡是"直接用
  /// `theme.textTheme.xxx` 当 style"的文字就变成**白底白字**（区块小标题、AppBar
  /// 标题、未选中的 chip 文案…），而 `ListTile` / 按钮这些**由 M3 默认值补了颜色**
  /// 的地方又一切正常 —— 现象非常像"只有小字有问题"。
  ///
  /// 同理，主题里凡是**覆盖掉 M3 默认样式**的项（`appBarTheme.titleTextStyle`、
  /// `tabBarTheme.labelStyle`、`chipTheme.labelStyle`、`navigationBarTheme.labelTextStyle`）
  /// 也必须自己带上颜色，否则等于把默认值里那个颜色一起覆盖成"没有"。
  /// 防线：`test/theme_text_color_test.dart`（逐个样式查颜色 + 查解析后的实际颜色）。
  ///
  /// 降级表达（次要文字用 `outline` / `onSurfaceVariant`）由**调用点**自己
  /// `copyWith`，不在这里分档 —— 这样"某个样式到底什么颜色"只有一个答案。
  static TextTheme _tune(TextTheme base, String? family, Color fallback) {
    TextStyle style(
      double size,
      FontWeight weight, {
      double height = 1.55,
      Color? color,
    }) =>
        TextStyle(
          fontFamily: family,
          fontFamilyFallback: cjkFallback,
          fontSize: size,
          fontWeight: weight,
          height: height,
          // 拉丁字体的字距放到中文上会显得散，统一归零
          letterSpacing: 0,
          color: color ?? fallback,
        );

    return base.copyWith(
      // 大标题：字重拉满，中文才有分量
      displaySmall: style(30, FontWeight.w700, height: 1.3),
      headlineMedium: style(24, FontWeight.w700, height: 1.3),
      headlineSmall: style(21, FontWeight.w700, height: 1.35),
      titleLarge: style(19, FontWeight.w700, height: 1.35),
      titleMedium: style(16, FontWeight.w600, height: 1.4),
      titleSmall: style(14.5, FontWeight.w600, height: 1.4),
      // 正文：字号 +0.5、行高 1.6，长时间读提示词不累
      bodyLarge: style(15.5, FontWeight.w400, height: 1.6),
      bodyMedium: style(14.5, FontWeight.w400, height: 1.6),
      bodySmall: style(13, FontWeight.w400, height: 1.55),
      // 标签 / 元信息：小字号必须加字重，否则笔画发虚
      labelLarge: style(14, FontWeight.w600, height: 1.4),
      labelMedium: style(12.5, FontWeight.w500, height: 1.4),
      labelSmall: style(12, FontWeight.w500, height: 1.4),
    );
  }
}
