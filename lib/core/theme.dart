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

/// **Material You 取色档位**（真源在 `SettingsStore.accentMode`，偏好键
/// `comfyhub.accentMode`；老版本只存了布尔 `comfyhub.followSystemAccent`，
/// 由 [parse] 迁移）。
///
/// 三档的差别只在"表面（surface）要不要跟着种子色走"：
///   · [off]    —— 不读系统颜色，永远默认紫；
///   · [accent] —— 2026-10-24 定的口径：系统强调色只用于**组件**（主色 / 选中态 /
///                 进度条 / 焦点环），页面、卡片、侧栏保持中性白灰；
///   · [full]   —— Material You 的标准做法：整份 `ColorScheme`（含各档
///                 `surfaceContainer*`）都由种子派生，页面会带一层很淡的同色相。
///
/// ⚠ 默认 **[full]**（2026-11-05 用户拍板"动态取色的尺度做到 A"）：打开就是
/// Material You —— 系统强调色同时决定组件色与表面色。想回到"只染组件"或
/// "固定紫"仍可在设置里选 [accent] / [off]，但默认值不再是它们。
enum AppAccentMode {
  off('固定配色', '不读系统颜色，始终使用默认种子色。'),
  accent('跟随强调色', '只把系统强调色用于按钮 / 选中态 / 焦点环，页面底色回到默认种子的中性色。'),
  full('完整动态', '整份配色（包含页面与卡片底色）都由系统颜色派生，即 Material You。');

  final String label;
  final String description;

  const AppAccentMode(this.label, this.description);

  /// 读偏好，认不出来的一律回 [full]（**绝不抛**：它在启动路径上）。
  static AppAccentMode parse(String? raw) {
    for (final mode in values) {
      if (mode.name == raw) return mode;
    }
    return AppAccentMode.full;
  }

  /// 老偏好迁移：`comfyhub.followSystemAccent` 为 true → [full]，false → [off]。
  ///
  /// 老用户当时选的"自动套用系统颜色"，在 A 口径下就是完整动态取色。
  static AppAccentMode fromLegacyBool(bool? followSystemAccent) =>
      (followSystemAccent ?? true) ? AppAccentMode.full : AppAccentMode.off;

  bool get usesSystemColor => this != AppAccentMode.off;
}

/// M3 的**形状令牌**（4 / 8 / 12 / 16 / 28，外加"胶囊"）。
///
/// 以前全项目散着 11 种圆角（4/5/6/8/9/10/12/14/16/20/999），同一个角色在不同页面
/// 能差 2~4px。这里把档位收成 5 个数，页面要圆角就 `AppShapes.of(context).medium`，
/// **别再写 `BorderRadius.circular(14)`** —— 那种"看着差不多"的值就是不一致的来源。
///
/// 对应关系（M3 shape scale）：
///   · extraSmall 4  —— 菜单、SnackBar、输入框
///   · small      8  —— Chip、小徽标
///   · medium     12 —— 卡片、列表项、图片格子
///   · large      16 —— FAB、气泡
///   · extraLarge 28 —— 对话框、底部面板
///   · （按钮 / 导航指示器 / 搜索框用 `StadiumBorder`，不在这里）
@immutable
class AppShapes extends ThemeExtension<AppShapes> {
  const AppShapes({
    this.extraSmall = 4,
    this.small = 8,
    this.medium = 12,
    this.large = 16,
    this.extraLarge = 28,
  });

  final double extraSmall;
  final double small;
  final double medium;
  final double large;
  final double extraLarge;

  /// 默认档位（主题里注册的就是它）。测试 / 脱离主题的绘制可以直接用。
  static const AppShapes standard = AppShapes();

  static AppShapes of(BuildContext context) =>
      Theme.of(context).extension<AppShapes>() ?? standard;

  BorderRadius get extraSmallRadius => BorderRadius.circular(extraSmall);
  BorderRadius get smallRadius => BorderRadius.circular(small);
  BorderRadius get mediumRadius => BorderRadius.circular(medium);
  BorderRadius get largeRadius => BorderRadius.circular(large);
  BorderRadius get extraLargeRadius => BorderRadius.circular(extraLarge);

  /// 顶角是 [extraLarge] 的底部面板形状（M3 bottom sheet）。
  RoundedRectangleBorder get bottomSheet => RoundedRectangleBorder(
    borderRadius: BorderRadius.vertical(top: Radius.circular(extraLarge)),
  );

  RoundedRectangleBorder rounded(double radius, {BorderSide? side}) =>
      RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(radius),
        side: side ?? BorderSide.none,
      );

  @override
  AppShapes copyWith({
    double? extraSmall,
    double? small,
    double? medium,
    double? large,
    double? extraLarge,
  }) => AppShapes(
    extraSmall: extraSmall ?? this.extraSmall,
    small: small ?? this.small,
    medium: medium ?? this.medium,
    large: large ?? this.large,
    extraLarge: extraLarge ?? this.extraLarge,
  );

  @override
  AppShapes lerp(ThemeExtension<AppShapes>? other, double t) {
    if (other is! AppShapes) return this;
    double mix(double a, double b) => a + (b - a) * t;
    return AppShapes(
      extraSmall: mix(extraSmall, other.extraSmall),
      small: mix(small, other.small),
      medium: mix(medium, other.medium),
      large: mix(large, other.large),
      extraLarge: mix(extraLarge, other.extraLarge),
    );
  }
}

/// **语义色**（成功 / 警告 / 信息）。
///
/// `ColorScheme` 只给了 primary / secondary / tertiary / error 四组语义，
/// 而界面里"跑通了 / 要小心 / 只是提示"这三件事一直在用 `Colors.green` /
/// `Colors.amber` / `Colors.orange` 硬编码（17 + 9 + 5 处）—— 它们在浅色主题下
/// 对比度不够、在动态取色下和主色打架。
///
/// 每组都按当前 [ColorScheme.brightness] 由独立种子派生，所以深浅两套的
/// 色阶与 M3 一致（浅色 tone 40 / 深色 tone 80），不会出现"绿字在深底上发黑"。
@immutable
class AppColors extends ThemeExtension<AppColors> {
  const AppColors({
    required this.success,
    required this.onSuccess,
    required this.successContainer,
    required this.onSuccessContainer,
    required this.warning,
    required this.onWarning,
    required this.warningContainer,
    required this.onWarningContainer,
    required this.info,
    required this.onInfo,
    required this.infoContainer,
    required this.onInfoContainer,
  });

  final Color success;
  final Color onSuccess;
  final Color successContainer;
  final Color onSuccessContainer;
  final Color warning;
  final Color onWarning;
  final Color warningContainer;
  final Color onWarningContainer;
  final Color info;
  final Color onInfo;
  final Color infoContainer;
  final Color onInfoContainer;

  /// 从当前方案派生三组语义色（种子只定色相，明暗跟着 [scheme] 走）。
  factory AppColors.from(ColorScheme scheme) {
    ColorScheme palette(int seed) => ColorScheme.fromSeed(
      seedColor: Color(seed),
      brightness: scheme.brightness,
    );
    final s = palette(0xFF2E7D32);
    final w = palette(0xFF8A5A00);
    final i = palette(0xFF1565C0);
    return AppColors(
      success: s.primary,
      onSuccess: s.onPrimary,
      successContainer: s.primaryContainer,
      onSuccessContainer: s.onPrimaryContainer,
      warning: w.primary,
      onWarning: w.onPrimary,
      warningContainer: w.primaryContainer,
      onWarningContainer: w.onPrimaryContainer,
      info: i.primary,
      onInfo: i.onPrimary,
      infoContainer: i.primaryContainer,
      onInfoContainer: i.onPrimaryContainer,
    );
  }

  static AppColors of(BuildContext context) {
    final theme = Theme.of(context);
    return theme.extension<AppColors>() ?? AppColors.from(theme.colorScheme);
  }

  @override
  AppColors copyWith({
    Color? success,
    Color? onSuccess,
    Color? successContainer,
    Color? onSuccessContainer,
    Color? warning,
    Color? onWarning,
    Color? warningContainer,
    Color? onWarningContainer,
    Color? info,
    Color? onInfo,
    Color? infoContainer,
    Color? onInfoContainer,
  }) => AppColors(
    success: success ?? this.success,
    onSuccess: onSuccess ?? this.onSuccess,
    successContainer: successContainer ?? this.successContainer,
    onSuccessContainer: onSuccessContainer ?? this.onSuccessContainer,
    warning: warning ?? this.warning,
    onWarning: onWarning ?? this.onWarning,
    warningContainer: warningContainer ?? this.warningContainer,
    onWarningContainer: onWarningContainer ?? this.onWarningContainer,
    info: info ?? this.info,
    onInfo: onInfo ?? this.onInfo,
    infoContainer: infoContainer ?? this.infoContainer,
    onInfoContainer: onInfoContainer ?? this.onInfoContainer,
  );

  @override
  AppColors lerp(ThemeExtension<AppColors>? other, double t) {
    if (other is! AppColors) return this;
    Color mix(Color a, Color b) => Color.lerp(a, b, t)!;
    return AppColors(
      success: mix(success, other.success),
      onSuccess: mix(onSuccess, other.onSuccess),
      successContainer: mix(successContainer, other.successContainer),
      onSuccessContainer: mix(onSuccessContainer, other.onSuccessContainer),
      warning: mix(warning, other.warning),
      onWarning: mix(onWarning, other.onWarning),
      warningContainer: mix(warningContainer, other.warningContainer),
      onWarningContainer: mix(onWarningContainer, other.onWarningContainer),
      info: mix(info, other.info),
      onInfo: mix(onInfo, other.onInfo),
      infoContainer: mix(infoContainer, other.infoContainer),
      onInfoContainer: mix(onInfoContainer, other.onInfoContainer),
    );
  }
}

/// M3 的**动效令牌**（时长 + 强调缓动）。
///
/// 以前页面里散着 150 / 180 / 200 / 220 / 250 / 260 / 280 / 300 / 1400ms 与两种曲线。
/// 时长本身不是重点，"同一类动作时长一致"才是 —— 换页、展开、淡入各用各的会显得散。
class AppMotion {
  AppMotion._();

  /// 小状态切换（图标、选中态、淡入）。
  static const Duration short = Duration(milliseconds: 150);

  /// 常规过渡（展开 / 收起、进度）。
  static const Duration medium = Duration(milliseconds: 250);

  /// 大块内容（页面切换、面板）。
  static const Duration long = Duration(milliseconds: 400);

  /// M3 的 emphasized decelerate 近似值（进场）。
  static const Curve emphasized = Cubic(0.2, 0.0, 0.0, 1.0);

  /// M3 的 emphasized accelerate 近似值（退场）。
  static const Curve emphasizedAccelerate = Cubic(0.3, 0.0, 0.8, 0.15);
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
///   · 显式指定中文字体族（**思源黑体**，见 [cjkFallback]），并按平台给一串兜底；
///   · 把字距归零、行高放到 1.5~1.6（中文需要更大的行距才不挤）；
///   · 小字号统一提高一档字重，让笔画在低分屏上也立得住。
class AppTheme {
  AppTheme._();

  /// 默认种子色（深紫）。系统拿不到强调色时用它；**存量外观就是这一套**。
  ///
  /// 别随手改它：`SettingsStore.accentMode` 为 `off`、或平台没有可读的
  /// 系统强调色时，界面会回到这里 —— 这个值等于"以前所有人看到的那套配色"。
  static const Color defaultSeed = Color(0xFF7C5CFF);

  /// 中文兜底字体链。按优先级排列，缺哪个会自动往后找。
  ///
  /// 第一位是**思源黑体**（Source Han Sans / Noto Sans SC 是同一套字形的两个名字）：
  /// 桌面端装上就用它；没装则退到系统自带的 YaHei / PingFang，行为与以前一致。
  static const List<String> cjkFallback = <String>[
    'Source Han Sans SC', // 思源黑体（Adobe 名）
    'Noto Sans SC', // 思源黑体的 Google 名（Windows 10/11 常见）
    'Microsoft YaHei UI', // Windows 11 默认中文 UI 字体
    'Microsoft YaHei', // Windows 中文正文字体
    'PingFang SC', // macOS / iOS
    'Hiragino Sans GB', // 老版本 macOS
    'Noto Sans CJK SC', // Linux / Android 常见
    'Source Han Sans CN', // 思源黑体（简体中文的另一个家族名）
    'WenQuanYi Micro Hei', // Linux
    'SimHei',
    'sans-serif',
  ];

  /// 各平台首选字体。返回 null 表示交给系统默认（Android / iOS 自带的中文都不错）。
  static String? primaryFontFamily() {
    if (kIsWeb) return null;
    try {
      // 桌面端统一用思源黑体（本机 2026-11-05 用户要求）：装了就生效，
      // 没装会沿 [cjkFallback] 退到 YaHei，不会出现"字体名不存在 → 方块"。
      if (Platform.isWindows || Platform.isLinux || Platform.isMacOS) {
        return 'Source Han Sans SC';
      }
    } catch (_) {
      // 平台判断在个别环境会抛异常，忽略
    }
    return null;
  }

  static ThemeData build(
    Brightness brightness, {
    Color seedColor = defaultSeed,
    AppAccentMode accentMode = AppAccentMode.full,
  }) {
    final seed = accentMode.usesSystemColor ? seedColor : defaultSeed;

    // -------------------------------------------------------------------------
    //  取色档位（Material You 的"个人化"那一半）
    //
    //  · full   —— 整份方案（含 surface / surfaceContainer*）由种子派生；
    //  · accent —— 只把强调角色从种子里取过来，表面角色仍来自默认种子；
    //  · off    —— accent 的 seed 已经被钉成默认紫，等同老外观。
    //
    //  ⚠ 2026-11-05 用户把默认档拍成 [full]（"动态取色的尺度做到 A"），
    //    但 2026-10-24 那句"底色别变"仍然有效 —— 它现在对应 [accent] 这一档，
    //    用户想回去随时能选。**别把这一支删掉**，那是"只染组件"的唯一实现。
    // -------------------------------------------------------------------------
    final ColorScheme scheme;
    if (accentMode == AppAccentMode.full || seed == defaultSeed) {
      scheme = ColorScheme.fromSeed(
        seedColor: seed,
        brightness: brightness,
        // tonalSpot 就是 Material You（壁纸取色）的默认方案。
        dynamicSchemeVariant: DynamicSchemeVariant.tonalSpot,
      );
    } else {
      final accentScheme = ColorScheme.fromSeed(
        seedColor: seed,
        brightness: brightness,
      );
      scheme =
          ColorScheme.fromSeed(
            seedColor: defaultSeed,
            brightness: brightness,
          ).copyWith(
            primary: accentScheme.primary,
            onPrimary: accentScheme.onPrimary,
            primaryContainer: accentScheme.primaryContainer,
            onPrimaryContainer: accentScheme.onPrimaryContainer,
            secondary: accentScheme.secondary,
            onSecondary: accentScheme.onSecondary,
            secondaryContainer: accentScheme.secondaryContainer,
            onSecondaryContainer: accentScheme.onSecondaryContainer,
            tertiary: accentScheme.tertiary,
            onTertiary: accentScheme.onTertiary,
            tertiaryContainer: accentScheme.tertiaryContainer,
            onTertiaryContainer: accentScheme.onTertiaryContainer,
            inversePrimary: accentScheme.inversePrimary,
          );
    }

    // 背景一律保持白 / 灰（用户 2026-10-24 最后定的口径）：页面、卡片、
    // 侧边栏、手机底部导航都用这套中性 surface；强调色只作用于**组件**
    // （按钮、开关、选中胶囊、焦点、进度条…），不铺到底色 / 卡片上。
    final navBackground = scheme.surfaceContainerLow;

    final base = ThemeData(useMaterial3: true, colorScheme: scheme);
    final family = primaryFontFamily();

    // 移动端才用 sparkle 水波纹（Android 12+ 的 M3 手感）；桌面上它是没必要的
    // shader 开销，保持 ripple。
    var mobile = false;
    if (!kIsWeb) {
      try {
        mobile = Platform.isAndroid || Platform.isIOS;
      } catch (_) {
        // 忽略：当成桌面
      }
    }

    // -------------------------------------------------------------------------
    //  两枚"全局色"—— 后面每个组件都用它们，别再各写各的 alpha
    //
    //   · [line] —— 分隔线 / 卡片描边 / 菜单描边。直接用 `outlineVariant`，
    //     与 M3 的 divider 角色一致（以前压到 0.55 是为了深色下不抢戏，
    //     M3 的 outlineVariant 本来就是按主题调好的低对比色）。
    //   · [fill] —— 输入框、分组容器那种"比页面底色高一档"的填充。
    //
    //  ⚠ **层次一律用色彩表达，不用 `elevation`**：阴影是每帧都要重绘的一笔，
    //    而卡片 / 缩略图在画廊、预设、标签页里是几十上百份叠在一起。
    //    M3 的 `surfaceContainer*` 系列就是为这件事准备的 —— 本文件里所有
    //    elevation 都归零、并配 `surfaceTintColor: transparent`（M3 默认会按
    //    elevation 把 primary 叠上去，那才是"越浮越紫"的来源）。
    // -------------------------------------------------------------------------
    final line = scheme.outlineVariant;
    final fill = scheme.surfaceContainerHighest;

    return base.copyWith(
      textTheme: _tune(base.textTheme, family, scheme.onSurface),
      // `primaryTextTheme` 是"画在主色底上"的那一套（AppBar 反过来的时候用它），
      // 所以兜底色是 `onPrimary` 而不是 `onSurface`。
      primaryTextTheme: _tune(base.primaryTextTheme, family, scheme.onPrimary),
      visualDensity: VisualDensity.standard,

      // **形状 / 语义色令牌**：页面里要用圆角或"成功/警告"色，一律从这里取。
      extensions: <ThemeExtension<dynamic>>[
        AppShapes.standard,
        AppColors.from(scheme),
      ],

      // M3 的触摸反馈：移动端 sparkle、桌面 ripple。
      splashFactory: mobile
          ? InkSparkle.splashFactory
          : InkRipple.splashFactory,

      // 页面切换统一走 M3 的 fade-forwards（桌面端也不再用平台默认的"无动画"）。
      pageTransitionsTheme: const PageTransitionsTheme(
        builders: <TargetPlatform, PageTransitionsBuilder>{
          TargetPlatform.android: FadeForwardsPageTransitionsBuilder(),
          TargetPlatform.iOS: FadeForwardsPageTransitionsBuilder(),
          TargetPlatform.macOS: FadeForwardsPageTransitionsBuilder(),
          TargetPlatform.windows: FadeForwardsPageTransitionsBuilder(),
          TargetPlatform.linux: FadeForwardsPageTransitionsBuilder(),
          TargetPlatform.fuchsia: FadeForwardsPageTransitionsBuilder(),
        },
      ),

      // -------------------------------------------------------------------------
      //  顶栏：**唯一一处"页面骨架"级的样式**
      //
      //  M3 的 AppBar 默认**没有底边线**，滚动时也不变色（`scrolledUnderElevation`
      //  在桌面长列表上是一条会跟着滚的"脏带"，这里保持 0）。内容区与顶栏的分界
      //  交给 `surface` 与页面本身 —— 以前那 5 页手写的 `bottom: Divider` 与主题里
      //  的 `shape` 已一并去掉。
      // -------------------------------------------------------------------------
      appBarTheme: AppBarThemeData(
        backgroundColor: scheme.surface,
        foregroundColor: scheme.onSurface,
        elevation: 0,
        scrolledUnderElevation: 0,
        surfaceTintColor: Colors.transparent,
        centerTitle: false,
        // 页面标题比正文大一档（参照的 Material You 设置页是很醒目的大标题；
        // 这里不做整条 LargeAppBar 重构，只把字号提上去，高度仍是 56，窄屏不挤）。
        titleTextStyle: _ui(
          22,
          FontWeight.w700,
          family,
          height: 1.3,
          color: scheme.onSurface,
        ),
      ),

      // 分隔线：给一个默认色，页面里的 `Divider()` 不用再逐个传 color。
      // ⚠ 不设 `space`：各处都显式写了 `height: 1 / 20`，设了反而把显式的顶掉。
      dividerTheme: DividerThemeData(color: line, thickness: 1),

      tabBarTheme: TabBarThemeData(
        labelStyle: _ui(14, FontWeight.w700, family, color: scheme.onSurface),
        unselectedLabelStyle: _ui(
          14,
          FontWeight.w500,
          family,
          color: scheme.onSurfaceVariant,
        ),
        labelColor: scheme.onSurface,
        unselectedLabelColor: scheme.onSurfaceVariant,
        indicatorColor: scheme.primary,
        dividerColor: line,
      ),

      // 卡片：M3 表达性的「填充卡」—— **surfaceContainerHigh + 24 圆角、无描边**。
      //
      // 2026-11-05 用户拿了一份别人的 Material You 主题做参照：那一版的分组卡
      // 是"实心浅一档 + 大圆角"，而不是"和页面同色 + 一圈细边"。
      // 描边版在深色下会显得干、边界像表格；填充版才是 Android 12+ 的观感。
      // 网格里的缩略图不算卡片（它们有自己的 12 圆角），不受这一条影响。
      cardTheme: CardThemeData(
        color: scheme.surfaceContainerHigh,
        elevation: 0,
        margin: EdgeInsets.zero,
        surfaceTintColor: Colors.transparent,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(24)),
      ),

      // 输入框：M3 的 filled 变体（4 圆角）+ **看得见的焦点环**。
      // 以前只有 `borderSide: none`，键盘用户 Tab 进来除了光标没有任何提示。
      inputDecorationTheme: InputDecorationTheme(
        filled: true,
        fillColor: fill,
        border: OutlineInputBorder(
          borderRadius: BorderRadius.circular(AppShapes.standard.extraSmall),
          borderSide: BorderSide.none,
        ),
        enabledBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(AppShapes.standard.extraSmall),
          borderSide: BorderSide.none,
        ),
        focusedBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(AppShapes.standard.extraSmall),
          borderSide: BorderSide(color: scheme.primary, width: 2),
        ),
        hintStyle: TextStyle(
          fontFamily: family,
          fontFamilyFallback: cjkFallback,
          color: scheme.onSurfaceVariant,
        ),
        isDense: true,
        contentPadding: const EdgeInsets.symmetric(
          horizontal: 12,
          vertical: 12,
        ),
      ),

      // 按钮：M3 的形状是**胶囊**（StadiumBorder）。
      // ⚠ 仍然**不动尺寸**（`minimumSize` / `padding` 一动，320dp 那几页的
      //   `Wrap` 折行位置就变了，属于"为了好看把布局改坏"）。
      // 文字样式不用管：M3 的按钮走 `labelLarge`，已经在 [_tune] 里调过。
      filledButtonTheme: FilledButtonThemeData(style: _buttonStyle()),
      outlinedButtonTheme: OutlinedButtonThemeData(style: _buttonStyle()),
      textButtonTheme: TextButtonThemeData(style: _buttonStyle()),

      snackBarTheme: SnackBarThemeData(
        behavior: SnackBarBehavior.floating,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(AppShapes.standard.extraSmall),
        ),
        elevation: 0,
        // ⚠ 别设 `insetPadding`：`copyToClipboard` 那类定点提示传了 `width`，
        // 而 SnackBar 对"width + 主题里的 insetPadding"有断言（见 SDK 的
        // `assert(width == null || margin == null)`）。默认值本来就是对称的。
      ),

      // 对话框：M3 是 28 圆角、`surfaceContainerHigh`、无描边。
      dialogTheme: DialogThemeData(
        backgroundColor: scheme.surfaceContainerHigh,
        surfaceTintColor: Colors.transparent,
        elevation: 0,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(AppShapes.standard.extraLarge),
        ),
      ),

      // 弹出菜单：M3 的容器是 `surfaceContainer` + 4 圆角 + elevation 3。
      // `AppMenuButton` / 右键菜单都走 `MenuAnchor`（见 widgets/app_menu.dart），
      // 所以这里是**唯一**管到它们的入口。
      menuTheme: MenuThemeData(style: _menuStyle(scheme)),
      // 下拉选择框（Provider 详情页那两处 `DropdownButtonFormField`）
      dropdownMenuTheme: DropdownMenuThemeData(menuStyle: _menuStyle(scheme)),
      // 旧的 `PopupMenuButton` 还剩一处（训练台），形状/底色必须与 MenuAnchor 一致，
      // 否则同一个 App 里两套菜单。
      popupMenuTheme: PopupMenuThemeData(
        color: scheme.surfaceContainer,
        surfaceTintColor: Colors.transparent,
        elevation: 3,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(AppShapes.standard.extraSmall),
        ),
      ),

      // 底部面板：M3 的顶角 28 圆角 + 拖拽把手。
      bottomSheetTheme: BottomSheetThemeData(
        backgroundColor: scheme.surfaceContainerLow,
        modalBackgroundColor: scheme.surfaceContainerLow,
        surfaceTintColor: Colors.transparent,
        elevation: 0,
        showDragHandle: true,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.vertical(
            top: Radius.circular(AppShapes.standard.extraLarge),
          ),
        ),
      ),

      // 搜索框（设置页 / 卡片面板将来换 SearchBar 时用得到）
      searchBarTheme: SearchBarThemeData(
        backgroundColor: WidgetStatePropertyAll(scheme.surfaceContainerHigh),
        surfaceTintColor: const WidgetStatePropertyAll(Colors.transparent),
        elevation: const WidgetStatePropertyAll(0),
        shape: const WidgetStatePropertyAll(StadiumBorder()),
      ),

      bannerTheme: MaterialBannerThemeData(
        backgroundColor: scheme.surfaceContainerHigh,
        surfaceTintColor: Colors.transparent,
        dividerColor: line,
        contentTextStyle: TextStyle(
          fontFamily: family,
          fontFamilyFallback: cjkFallback,
          color: scheme.onSurface,
        ),
      ),

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

      listTileTheme: ListTileThemeData(
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(AppShapes.standard.medium),
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
      //  M3 的指示器是**胶囊**（StadiumBorder），不是圆角矩形；与按钮同一套语言。
      //  ⚠ 宽度 / 高度一个字都不动：`home_nav_test` 拿 `NavigationRail` 的矩形
      //  当"换页不重建"的判据（逐像素相同），改尺寸会让那条用例失去意义。
      // -------------------------------------------------------------------------
      navigationRailTheme: NavigationRailThemeData(
        backgroundColor: navBackground,
        labelType: NavigationRailLabelType.all,
        useIndicator: true,
        indicatorColor: scheme.secondaryContainer,
        indicatorShape: const StadiumBorder(),
        selectedIconTheme: IconThemeData(
          color: scheme.onSecondaryContainer,
          size: 24,
        ),
        unselectedIconTheme: IconThemeData(
          color: scheme.onSurfaceVariant,
          size: 24,
        ),
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
        backgroundColor: navBackground,
        surfaceTintColor: Colors.transparent,
        elevation: 0,
        indicatorColor: scheme.secondaryContainer,
        indicatorShape: const StadiumBorder(),
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

      iconTheme: IconThemeData(color: scheme.onSurfaceVariant, size: 24),

      // 开关：M3 表达性的"带图标拇指"（开 = 勾、关 = 叉）。
      // 参照的 Material You 主题就是这一种；纯色圆点在小尺寸下分不清开没开。
      switchTheme: SwitchThemeData(
        thumbIcon: WidgetStateProperty.resolveWith((states) {
          final selected = states.contains(WidgetState.selected);
          return Icon(
            selected ? Icons.check : Icons.close,
            size: 16,
            // 选中时拇指是 `onPrimary`（深色），图标必须用 `primary` 才看得见；
            // 未选中时拇指是 `onSurfaceVariant` 的浅色块，图标用同色系深一档。
            color: selected ? scheme.primary : scheme.onSurfaceVariant,
          );
        }),
      ),

      // 滑块：M3 2025 的"条形拇指 + 粗轨道"（参照图里的样式）。
      // 默认的圆形拇指在老版 M3 里是 20px 圆，和这份参照差别很大。
      sliderTheme: SliderThemeData(
        trackHeight: 8,
        activeTrackColor: scheme.primary,
        inactiveTrackColor: scheme.surfaceContainerHighest,
        thumbColor: scheme.primary,
        overlayColor: scheme.primary.withValues(alpha: 0.12),
        thumbShape: const _BarSliderThumbShape(),
        trackShape: const RoundedRectSliderTrackShape(),
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
  }) => TextStyle(
    fontFamily: family,
    fontFamilyFallback: cjkFallback,
    fontSize: size,
    fontWeight: weight,
    height: height,
    letterSpacing: 0,
    color: color,
  );

  /// 按钮样式：**只统一形状**（M3 胶囊）。
  ///
  /// ⚠ 刻意不碰 `minimumSize` / `padding` / `tapTargetSize`：那些一动，
  /// 窄屏页面里 `Wrap` 的折行位置就变了（设置页「保存并测试连接」那一排
  /// 在 320dp 上本来就是贴着实测溢出 7.1px 的边缘修的）。
  /// 也不碰 `side`：M3 的描边色本来就来自 `outlineVariant`，而"禁用态要淡化"
  /// 那件事只有默认实现做对了 —— 用 `WidgetStatePropertyAll` 覆盖会把禁用态
  /// 也画成实边。
  static ButtonStyle _buttonStyle() =>
      const ButtonStyle(shape: WidgetStatePropertyAll(StadiumBorder()));

  /// 弹出菜单样式（`AppMenuButton` / 右键菜单 / `DropdownButtonFormField` 共用）。
  ///
  /// 一层很轻的投影是**故意留的**：菜单必须"浮"在内容之上，而它同时只会存在
  /// 一个 —— 与列表项那种每帧 N 份的阴影不是一回事。底色用 `surfaceContainer`
  /// （M3 的菜单角色），配合 4 圆角。
  static MenuStyle _menuStyle(ColorScheme scheme) => MenuStyle(
    backgroundColor: WidgetStatePropertyAll(scheme.surfaceContainer),
    surfaceTintColor: const WidgetStatePropertyAll(Colors.transparent),
    shadowColor: WidgetStatePropertyAll(Colors.black.withValues(alpha: 0.3)),
    elevation: const WidgetStatePropertyAll(3),
    shape: WidgetStatePropertyAll(
      RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(AppShapes.standard.extraSmall),
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
  // ⚠ 滑块拇指的绘制在文件末尾的 [_BarSliderThumbShape]（需要 `PaintingContext`）。

  static TextTheme _tune(TextTheme base, String? family, Color fallback) {
    TextStyle style(
      double size,
      FontWeight weight, {
      double height = 1.55,
      Color? color,
    }) => TextStyle(
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

/// M3 2025 的滑块拇指：一根竖直的圆角短棒（不是圆点）。
///
/// 参照主题（KernelSU 的 Material You 设置页）就是这一种：轨道更粗、拇指是
/// 一根 4~8px 的竖棒，拖动时不会像圆点那样"浮"在轨道外面。
class _BarSliderThumbShape extends SliderComponentShape {
  const _BarSliderThumbShape();

  static const Size _size = Size(8, 28);

  @override
  Size getPreferredSize(bool isEnabled, bool isDiscrete) => _size;

  @override
  void paint(
    PaintingContext context,
    Offset center, {
    required Animation<double> activationAnimation,
    required Animation<double> enableAnimation,
    required bool isDiscrete,
    required TextPainter labelPainter,
    required RenderBox parentBox,
    required SliderThemeData sliderTheme,
    required TextDirection textDirection,
    required double value,
    required double textScaleFactor,
    required Size sizeWithOverflow,
  }) {
    final paint = Paint()..color = sliderTheme.thumbColor ?? Colors.white;
    final rect = Rect.fromCenter(
      center: center,
      width: _size.width,
      height: _size.height,
    );
    context.canvas.drawRRect(
      RRect.fromRectAndRadius(rect, const Radius.circular(4)),
      paint,
    );
  }
}
