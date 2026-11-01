import 'dart:async';
import 'dart:io' show Platform;

import 'package:flutter/services.dart';

/// 局域网**桌面发现**（Android UDP 广播；桌面端由 Rust `mobile::discovery` 发）。
///
/// 它只解决"桌面现在在哪"：换 Wi-Fi / DHCP 换 IP / 后端换端口后，手机不用重新扫码，
/// 收到广播就用**已有配对令牌**去探一次 `/api/health`。令牌不在这里出现、也不会轮换。
///
/// 边界：广播只在同一广播域内有效；跨网段（或 Wi-Fi AP 隔离）仍要走 Tailscale /
/// 手填地址。发现到的地址也**不是信任凭据** —— 调用方必须再打一次真实请求。
class LanDiscovery {
  LanDiscovery._();

  static const MethodChannel _method = MethodChannel('comfyhub/discovery');
  static const EventChannel _events = EventChannel('comfyhub/discovery/events');

  static bool get supported {
    try {
      return Platform.isAndroid;
    } catch (_) {
      return false;
    }
  }

  /// 发现到的桌面地址流。监听开始会启动原生 UDP 监听，取消时停掉。
  ///
  /// 同一台桌面 15 秒内只报一次（原生去重），调用方不必再做节流。
  static Stream<Uri> get endpoints {
    StreamSubscription<dynamic>? sub;
    late StreamController<Uri> controller;
    controller = StreamController<Uri>(
      onListen: () async {
        try {
          await _method.invokeMethod<void>('start');
        } catch (e, st) {
          controller.addError(e, st);
          await controller.close();
          return;
        }
        sub = _events.receiveBroadcastStream().listen(
          (event) {
            final map = (event as Map?)?.cast<Object?, Object?>();
            final url = map?['url']?.toString() ?? '';
            final uri = Uri.tryParse(url);
            if (uri != null && uri.host.isNotEmpty && controller.hasListener) {
              controller.add(uri);
            }
          },
          onError: (Object e, StackTrace st) {
            if (controller.hasListener) controller.addError(e, st);
          },
        );
      },
      onCancel: () async {
        await sub?.cancel();
        sub = null;
        try {
          await _method.invokeMethod<void>('stop');
        } catch (_) {
          // 原生侧已经没了（进程重启 / Activity 销毁）：不用报错
        }
      },
    );
    return controller.stream;
  }
}
