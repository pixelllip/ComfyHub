pluginManagement {
    val flutterSdkPath =
        run {
            val properties = java.util.Properties()
            file("local.properties").inputStream().use { properties.load(it) }
            val flutterSdkPath = properties.getProperty("flutter.sdk")
            require(flutterSdkPath != null) { "flutter.sdk not set in local.properties" }
            flutterSdkPath
        }

    includeBuild("$flutterSdkPath/packages/flutter_tools/gradle")

    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

plugins {
    id("dev.flutter.flutter-plugin-loader") version "1.0.0"
    id("com.android.application") version "9.4.0" apply false
    // 这里只声明 KGP 版本、不 apply（apply false），目的是把 Kotlin 抬到 Flutter 的最低要求之上。
    //
    // 为什么 built-in Kotlin 还要声明 KGP：
    //   AGP 9 的内建 Kotlin 用的是 AGP 自己运行时依赖的 KGP
    //   （AGP 9.1.0 -> org.jetbrains.kotlin:kotlin-gradle-plugin:2.2.10），
    //   而 Flutter 3.47 的 DependencyVersionChecker 要求 KGP >= 2.2.20，
    //   否则直接抛错：
    //     "Error: Your project's Kotlin version (2.2.10) is lower than
    //      Flutter's minimum supported version of 2.2.20."
    //   见 flutter/flutter#192167（3.47.3 里尚未包含修复）。
    //   在 classpath 上声明更高的 KGP，同时也是 AGP 官方推荐的"内建 Kotlin 版本覆盖"方式。
    // 注意：只是 apply false，任何模块都不会真正 apply KGP，
    // 所以不会和内建 Kotlin 抢 `kotlin` 扩展名。
    id("org.jetbrains.kotlin.android") version "2.4.0" apply false
}

include(":app")
