plugins {
    id("com.android.application")
    // Kotlin 由 AGP 9 内建提供（android.builtInKotlin=true），
    // 这里不再 apply org.jetbrains.kotlin.android / kotlin-android。
    // The Flutter Gradle Plugin must be applied after the Android Gradle plugin.
    id("dev.flutter.flutter-gradle-plugin")
}

android {
    namespace = "com.example.viewer"
    compileSdk = flutter.compileSdkVersion
    ndkVersion = flutter.ndkVersion

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    defaultConfig {
        // TODO: Specify your own unique Application ID (https://developer.android.com/studio/build/application-id.html).
        applicationId = "com.example.viewer"
        // You can update the following values to match your application needs.
        // For more information, see: https://flutter.dev/to/review-gradle-config.
        minSdk = flutter.minSdkVersion
        targetSdk = flutter.targetSdkVersion
        // Uses the version code from pubspec.yaml. When using split APKs, 1000 * ABI_VERSION
        // is added automatically by Flutter. (https://developer.android.com/studio/build/configure-apk-splits#configure-APK-versions)
        // You can force using the value of versionCode by specifying the `-P force-version-code-ignoring-abi=true`
        // flag during build.
        versionCode = flutter.versionCode
        versionName = flutter.versionName
    }

    buildTypes {
        release {
            // TODO: Add your own signing config for the release build.
            // Signing with the debug keys for now, so `flutter run --release` works.
            signingConfig = signingConfigs.getByName("debug")
        }
    }
}

// 内建 Kotlin 下的编译选项 DSL（旧写法 android.kotlinOptions{} 已废弃）。
// 说明：jvmTarget 默认就等于 android.compileOptions.targetCompatibility(=17)，
// 这里显式写出来只为和 Flutter 官方迁移文档保持一致。
kotlin {
    compilerOptions {
        jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17
    }
}

flutter {
    source = "../.."
}

dependencies {
    // 手机端 harness（M3）：纯逻辑 + 本地 bundled SQLite 都在 :harness 里（可跑 JVM 单测）。
    // 平台胶水（platform channel / Android Keystore）留在 app 模块 —— 它们要 Flutter 的
    // embedding，而 :harness 刻意**不依赖 Flutter**（`docs/mobile-harness-design.md` §2.1
    // 的"为将来的 KMP 留门"：纯逻辑不许碰 Android/Flutter API）。
    implementation(project(":harness"))
    // :harness 把它们当 implementation（不外泄），app 的桥自己要用就显式声明：
    // JSON 对象在公开 API 里出现（run-kit / 事件 payload），协程用于事件泵。
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.11.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.11.0")
}
