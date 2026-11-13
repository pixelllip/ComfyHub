import 'dart:ffi';
import 'dart:io' show Platform;

import 'package:flutter/foundation.dart';
import 'package:material_ui/material_ui.dart';
import 'package:flutter/services.dart';

/// **平台主题色（系统强调色）检测**。
///
/// 能读到强调色的平台用它作为 `ColorScheme.fromSeed` 的种子色，读不到时回退到
/// [AppTheme.defaultSeed]。
///
/// ## 数据来源
///
/// Flutter 只提供 `MediaQuery.platformBrightness`（明 / 暗），不提供系统强调色，
/// 各平台分别读取：
///
/// | 平台 | 来源 | 可用性 |
/// | --- | --- | --- |
/// | Windows 10/11 | `HKCU\…\Explorer\Accent\AccentColorMenu`（ABGR）；取不到再退 `DWM\ColorizationColor`（ARGB） | 可用 |
/// | Android 12+ | 系统资源 `android:color/system_accent1_500`（Material You 的壁纸取色） | 可用 |
/// | 其他 | — | 回退默认 |
///
/// Windows 通过 **`dart:ffi` 直接读注册表**，不引入 `dynamic_color` 等插件，
/// 也不修改 `windows/` 原生代码：后端与原生侧无需改动，也不需要重编 release 产物。
/// ⚠ FFI 库句柄只在 Windows 分支打开（顶层 `DynamicLibrary.open('msvcrt.dll')`
/// 在 Android 上会直接报错）。
///
/// ## 检测时机
///
/// `main()` 在 `runApp` 之前 `await SystemAccent.detect()`：主题首帧就要使用，
/// 延迟会导致先显示默认色再切换。设置页点「重新检测」时会再执行一次
/// （`SettingsStore.refreshSystemAccent()`），随后由 `SettingsStore` 通知重建
/// `MaterialApp`。
class SystemAccent {
  SystemAccent._();

  static Color? _color;
  static bool _detected = false;
  static String? _source;

  /// 检测到的系统强调色；null = 这个平台没有可读的强调色（或读取失败）。
  static Color? get color => _color;

  /// 有没有可用的系统强调色。设置页拿它决定开关能不能点。
  static bool get available => _color != null;

  /// 是否跑过一次检测（**不等于**取到了颜色：Android 11 及以下必然 false）。
  static bool get detected => _detected;

  /// 来源说明（设置页展示用），例如 `Windows 强调色 #720903`。
  static String get sourceLabel {
    if (_color == null) return '当前平台没有可用的系统强调色';
    final rgb = _color!.toARGB32() & 0x00FFFFFF;
    return '${_source ?? '系统强调色'} #${rgb.toRadixString(16).padLeft(6, '0').toUpperCase()}';
  }

  /// 读一次。**绝不抛**（它在启动路径上，抛了就是白屏）——失败等于"没有"。
  static Future<void> detect() async {
    Color? found;
    String? source;
    try {
      if (!kIsWeb && Platform.isWindows) {
        found = _windowsAccent();
        source = 'Windows 强调色';
      } else if (!kIsWeb && Platform.isAndroid) {
        found = await _androidAccent();
        source = 'Android 系统强调色';
      }
    } catch (e) {
      // 单个平台的 API 缺失 / 注册表没这项 / 通道没注册，都只意味着"取不到"
      debugPrint('SystemAccent: 检测失败（按没有处理）：$e');
    }
    _color = found;
    _source = found == null ? null : source;
    _detected = true;
  }

  /// 用例注入用：不碰真实注册表 / 平台通道。
  @visibleForTesting
  static void debugSetColor(Color? color, {String source = '测试强调色'}) {
    _color = color;
    _source = color == null ? null : source;
    _detected = true;
  }

  /// Windows：`AccentColorMenu` 是 **ABGR**，`ColorizationColor` 是 **ARGB**。
  ///
  /// ⚠ 两个值的字节序不同（Win32 的历史问题）：本机实测同一份强调色
  /// `#720903` 在 `AccentColorMenu` 里存成 `0xFF030972`，在 `ColorizationColor`
  /// 里存成 `0xC4720903`。**别拿一个解码函数套两个键**。
  static Color? _windowsAccent() {
    final menu = _regGetDword(
      r'Software\Microsoft\Windows\CurrentVersion\Explorer\Accent',
      'AccentColorMenu',
    );
    final fromMenu = accentFromAbgr(menu);
    if (fromMenu != null) return fromMenu;
    return accentFromArgb(
      _regGetDword(r'Software\Microsoft\Windows\DWM', 'ColorizationColor'),
    );
  }

  /// Android 12+：系统强调色资源。低版本没有 Material You 取色，回 null。
  static Future<Color?> _androidAccent() async {
    final value = await const MethodChannel('comfyhub/system_theme')
        .invokeMethod<int>('accentColor');
    return accentFromArgb(value);
  }

  // ---------------------------------------------------------------------------
  //  解码（@visibleForTesting：两种字节序各钉一条，别再套错）
  // ---------------------------------------------------------------------------

  /// `0xAABBGGRR`（Windows `AccentColorMenu` 的存法）。
  @visibleForTesting
  static Color? accentFromAbgr(int? value) {
    if (value == null || value == 0) return null;
    return Color.fromARGB(
      255,
      value & 0xFF,
      (value >> 8) & 0xFF,
      (value >> 16) & 0xFF,
    );
  }

  /// `0xAARRGGBB`（Android `Color` / Windows `ColorizationColor` 的存法）。
  @visibleForTesting
  static Color? accentFromArgb(int? value) {
    if (value == null) return null;
    final rgb = value & 0x00FFFFFF;
    if (rgb == 0) return null;
    return Color(0xFF000000 | rgb);
  }

  /// 读一个 `REG_DWORD`。读不到返回 null（键不存在 / 值类型不对 / API 失败）。
  ///
  /// 这里用 `RegGetValueW` 而不是 `reg.exe`：不额外起进程、没有输出解析、
  /// 也不会在启动时闪一下命令行窗口。
  static int? _regGetDword(String subKey, String valueName) {
    final advapi32 = DynamicLibrary.open('advapi32.dll');
    final regGetValue = advapi32
        .lookupFunction<_RegGetValueWNative, _RegGetValueWDart>('RegGetValueW');
    // malloc / free 从 CRT 借：省掉 `package:ffi` 这一个依赖
    final msvcrt = DynamicLibrary.open('msvcrt.dll');
    final malloc = msvcrt.lookupFunction<_MallocNative, _MallocDart>('malloc');
    final free = msvcrt.lookupFunction<_FreeNative, _FreeDart>('free');

    final sub = _toUtf16(malloc, subKey);
    final name = _toUtf16(malloc, valueName);
    final data = malloc(4).cast<Uint32>();
    final size = malloc(4).cast<Uint32>();
    try {
      size.value = 4;
      final rc = regGetValue(
        _hkcu,
        sub,
        name,
        _rrfRtRegDword,
        nullptr,
        data.cast<Void>(),
        size,
      );
      return rc == 0 ? data.value : null;
    } finally {
      free(sub.cast<Void>());
      free(name.cast<Void>());
      free(data.cast<Void>());
      free(size.cast<Void>());
    }
  }

  static const int _hkcu = 0xFFFFFFFF80000001;

  /// `RRF_RT_REG_DWORD`：只接受 `REG_DWORD`（值类型被改坏时直接失败，不猜）。
  static const int _rrfRtRegDword = 0x00000010;

  static Pointer<Uint16> _toUtf16(
    Pointer<Void> Function(int) malloc,
    String text,
  ) {
    final units = text.codeUnits;
    final p = malloc((units.length + 1) * 2).cast<Uint16>();
    for (var i = 0; i < units.length; i++) {
      p[i] = units[i];
    }
    p[units.length] = 0;
    return p;
  }
}

// -----------------------------------------------------------------------------
//  FFI 签名（Win32）
// -----------------------------------------------------------------------------

typedef _RegGetValueWNative = Int32 Function(
  IntPtr hkey,
  Pointer<Uint16> subKey,
  Pointer<Uint16> value,
  Uint32 flags,
  Pointer<Uint32> type,
  Pointer<Void> data,
  Pointer<Uint32> size,
);
typedef _RegGetValueWDart = int Function(
  int hkey,
  Pointer<Uint16> subKey,
  Pointer<Uint16> value,
  int flags,
  Pointer<Uint32> type,
  Pointer<Void> data,
  Pointer<Uint32> size,
);

typedef _MallocNative = Pointer<Void> Function(IntPtr);
typedef _MallocDart = Pointer<Void> Function(int);
typedef _FreeNative = Void Function(Pointer<Void>);
typedef _FreeDart = void Function(Pointer<Void>);
