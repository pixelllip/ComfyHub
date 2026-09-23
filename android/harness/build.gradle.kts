plugins {
    // Kotlin 由 AGP 9 内建提供（android/gradle.properties 的 android.builtInKotlin=true），
    // 本模块**不** apply org.jetbrains.kotlin.android —— 与 :app 保持同一套约定。
    // 动这个文件之前先读 docs/android-agp9-builtin-kotlin-migration.md。
    id("com.android.library")
}

android {
    namespace = "com.comfyhub.harness"
    compileSdk = 36

    defaultConfig {
        // 与 :app 的 flutter.minSdkVersion 对齐（Flutter 3.47 的默认值是 24）。
        minSdk = 24
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    testOptions {
        unitTests.isReturnDefaultValues = true
    }
}

kotlin {
    compilerOptions {
        jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17
    }
}

dependencies {
    // ⚠ 刻意**不**引入 kotlinx-serialization 的编译期插件（@Serializable 代码生成）：
    // 那是 KGP 家族的编译器插件，与 AGP 内建 Kotlin 的组合没有成熟路径（见
    // docs/mobile-harness-design.md §2.1）。harness 只做 JsonElement 层面的读写，
    // 运行时库足够；将来若要 DTO 代码生成，再单独评估。
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.11.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.11.0")

    // ★ R14：本地库必须用 **bundled** SQLite（Android 13 的系统 SQLite 还是 3.32a、无 FTS5）。
    // sqlite-bundled 自带 native 引擎；sqlite 提供 SQLiteConnection/Statement 那层 API。
    // ⚠ 刻意**不用 Room**：Room 的注解处理要 KSP（又一个 KGP 家族插件），见上面那条注释。
    implementation("androidx.sqlite:sqlite:2.7.1")
    implementation("androidx.sqlite:sqlite-bundled:2.7.1")

    testImplementation("junit:junit:4.13.2")
    // 单元测试跑在 JVM 上，装不了 Android 的 JNI 库 → 用 xerial 的 sqlite-jdbc 跑**同一份 DDL/SQL**。
    // 它同样是"带引擎"（自带 SQLite 3.53），所以测的是真 SQL，不是假实现。
    testImplementation("org.xerial:sqlite-jdbc:3.53.4.0")
}
