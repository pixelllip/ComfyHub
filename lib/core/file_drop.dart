import 'dart:async';
import 'dart:io' show Platform;

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// 从资源管理器**拖进窗口**的文件 —— 与 `FileClipboard` 同一个套路：
/// 拿不到能力就**静默降级**。
///
/// # 为什么这件事必须落到原生
///
/// Flutter 框架**没有**"外部文件拖进窗口"的 API：`Draggable` / `DragTarget` 传的是
/// **应用内**的数据（画廊里那种拖动），跨进程的拖放引擎一个字都没接。原生那一半在
/// `windows/runner/file_drop_channel.cpp`：`DragAcceptFiles` + `WM_DROPFILES`。
///
/// # 方向与形状（与 `FileClipboard` 正好相反）
///
/// 这条通道是 **native → Dart**：原生收到 `WM_DROPFILES` 就**主动**调
//  * 通道名：`comfyhub/file_drop`
///  * 方法名：`droppedFiles`
///  * 参数：  `{"paths": ["D:\\a.png", "D:\\b.jpg"]}`（`List<String>`，UTF-8；
///    中文路径由原生那侧 `WideCharToMultiByte(CP_UTF8, ...)` 转好）
///
/// 因此除 `setMethodCallHandler` 的重复注册外，没有需要处理的"调用失败" ——
/// 这里只做三件事：安装接收者（一次）、把参数解成路径、推给监听方。
///
/// # 约束：这条链路是**纯增益**的
///
/// 非 Windows、旧产物（通道不存在）、参数畸形、无人监听 —— 一律按"这一次没有拖入
/// 内容"处理，**不向界面抛异常**。拖放失败的最坏结果是重新手动选择文件；抛异常会
/// 进入 AI 工作台的聊天链路，表现为"粘不上字"，且难以定位到通道。
///
/// ⚠ 界面文案"也可以把文件拖进来当附件"此前没有实现；这里补齐**能力**，监听方由
/// 页面决定（见 `stream`）。
class FileDrop {
  FileDrop._();

  static const MethodChannel _channel = MethodChannel('comfyhub/file_drop');

  /// 平台判据。默认就是 `Platform.isWindows`。
  ///
  /// 留成可替换的一格**只为一件事**：本机与 CI 都在 Windows 上，不接这一格，
  /// "非 Windows 上 `supported == false`、什么都不往流里推"这条就没有任何用例
  /// 钉得住（`test/file_drop_test.dart`）。
  @visibleForTesting
  static bool Function() platformProbe = _defaultPlatformProbe;

  /// 只有 Windows 有这份实现（`windows/runner/file_drop_channel.cpp`）。
  static bool get supported => platformProbe();

  /// 接收者**只装一次**：`setMethodCallHandler` 是"一条通道一个"，
  /// 后装的会把先装的**顶掉**（谁后调谁赢，前一个静默失效）。用这一格守住。
  static bool _handlerInstalled = false;

  static StreamController<List<String>>? _controller;
  static Stream<List<String>>? _stream;

  /// 每次拖进来一批（可能多个文件）。**广播流**：多个页面都能同时听
  /// （现在只有 AI 工作台要，以后画廊也要 —— 广播是为了那一天）。
  ///
  /// 惰性：第一次读它才建流 + 装接收者，没人听就不占着通道。
  ///
  /// ⚠ 广播流的固有语义：**没有听众时到达的那一批会被丢掉**（拖放是"此刻的
  /// 动作"，不该为后来才挂上来的页面重放一遍 —— 那会凭空多出一次附件导入）。
  static Stream<List<String>> get stream {
    final existing = _stream;
    if (existing != null) return existing;

    final controller = StreamController<List<String>>.broadcast();
    final stream = controller.stream;
    _controller = controller;
    _stream = stream;
    _installHandler();
    return stream;
  }

  static void _installHandler() {
    if (_handlerInstalled) return;
    _handlerInstalled = true;
    if (!supported) return; // 非 Windows：不装接收者，流永远不出东西
    try {
      _channel.setMethodCallHandler(_handleCall);
    } catch (_) {
      // 通道不通（老产物 / 引擎没这一侧）—— 静默降级：流还在，只是永远不响
    }
  }

  /// 原生推过来的那一批。**这个方法不许抛**：这里抛出去会变成给原生的错误回复，
  /// 而原生那边**根本不看回复**（`InvokeMethod` 不传 result），等于白响一声；
  /// 而在 Dart 这一侧，它会被报成一条没人认领的异步错误。
  static Future<Object?> _handleCall(MethodCall call) async {
    try {
      if (call.method != 'droppedFiles') return null;
      final controller = _controller;
      if (controller == null) return null;
      // 平台判据**每一批都重判**，不是只在装接收者时判一次 —— 否则"支持"这个
      // 结论会在运行期变过一次之后永远停在旧值上（测试里就是这么把它钉住的）。
      if (!supported) return null;

      final paths = _pathsFrom(call.arguments);
      if (paths.isEmpty) return null; // 空列表不推：监听方没必要白跑一趟
      controller.add(paths);
    } catch (_) {
      // 参数再畸形也只当"这一次没拖进来东西"
    }
    return null;
  }

  /// 从 `{"paths": [...]}` 里取出路径。形状不对一律回空表（**不猜、不修**）：
  /// 路径是别的进程给的，只按约定的形状取用，额外"修正"只会把错误的内容
  /// 当成附件导进来。
  static List<String> _pathsFrom(Object? arguments) {
    if (arguments is! Map) return const [];
    final raw = arguments['paths'];
    if (raw is! List) return const [];
    return raw
        .whereType<String>()
        .where((path) => path.isNotEmpty)
        .toList(growable: false);
  }
}

bool _defaultPlatformProbe() => Platform.isWindows;
