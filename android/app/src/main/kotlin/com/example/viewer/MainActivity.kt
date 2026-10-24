package com.example.viewer

import android.os.Build
import com.comfyhub.harness.bridge.HarnessBridge
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

/**
 * 手机端的入口 Activity。
 *
 * 除了 Flutter 自己，它只挂一件事：**harness 桥**（M3）。
 *
 * | 通道 | 方向 | 用途 |
 * | --- | --- | --- |
 * | `comfyhub/harness`（MethodChannel） | Dart → Kotlin | 取 run-kit、起 Run、取消、答提问、读本地会话 |
 * | `comfyhub/harness/events`（EventChannel） | Kotlin → Dart | 与桌面**同一套** `RunEvent` 事件流 |
 * | `comfyhub/system_theme`（MethodChannel） | Dart → Kotlin | 系统强调色（Android 12+ 的 Material You 取色） |
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
        // 平台主题色（`lib/core/system_accent.dart`）：Android 12+ 从系统资源里读
        // Material You 的强调色；更老的版本没有壁纸取色，回 null（Dart 侧回默认紫）。
        MethodChannel(
            flutterEngine.dartExecutor.binaryMessenger,
            SYSTEM_THEME_CHANNEL,
        ).setMethodCallHandler { call, result ->
            when (call.method) {
                "accentColor" -> result.success(readSystemAccentColor())
                else -> result.notImplemented()
            }
        }
    }

    /** `android:color/system_accent1_500`（API 31+）。读不到 / 老系统一律 null。 */
    private fun readSystemAccentColor(): Int? {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return null
        return try {
            val id = resources.getIdentifier("system_accent1_500", "color", "android")
            if (id == 0) null else resources.getColor(id, theme)
        } catch (_: Throwable) {
            null
        }
    }

    override fun onDestroy() {
        bridge?.detach()
        bridge = null
        super.onDestroy()
    }

    private companion object {
        const val SYSTEM_THEME_CHANNEL = "comfyhub/system_theme"
    }
}
