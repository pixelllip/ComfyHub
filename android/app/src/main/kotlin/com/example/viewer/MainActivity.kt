package com.example.viewer

import com.comfyhub.harness.bridge.HarnessBridge
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine

/**
 * 手机端的入口 Activity。
 *
 * 除了 Flutter 自己，它只挂一件事：**harness 桥**（M3）。
 *
 * | 通道 | 方向 | 用途 |
 * | --- | --- | --- |
 * | `comfyhub/harness`（MethodChannel） | Dart → Kotlin | 取 run-kit、起 Run、取消、答提问、读本地会话 |
 * | `comfyhub/harness/events`（EventChannel） | Kotlin → Dart | 与桌面**同一套** `RunEvent` 事件流 |
 *
 * ⚠ 桥里**没有任何规则文案**：系统提示、工具描述、安全横幅全部来自桌面下发的 run-kit
 * （红线 R13）。这里只做搬运。
 */
class MainActivity : FlutterActivity() {

    private var bridge: HarnessBridge? = null

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        bridge = HarnessBridge(applicationContext).also {
            it.attach(flutterEngine.dartExecutor.binaryMessenger)
        }
    }

    override fun onDestroy() {
        bridge?.detach()
        bridge = null
        super.onDestroy()
    }
}
