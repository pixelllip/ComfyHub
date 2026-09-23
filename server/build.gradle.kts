// ---------------------------------------------------------------------------
//  注意：本机（以及部分国内网络）无法访问 plugins.gradle.org，
//  因此这里不使用 `plugins { kotlin("jvm") version ... }` 这种需要
//  Gradle Plugin Portal 的写法，而是直接从 Maven Central 拉取 Kotlin 插件。
// ---------------------------------------------------------------------------
buildscript {
    repositories {
        mavenCentral()
    }
    dependencies {
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:2.2.20")
        classpath("org.jetbrains.kotlin:kotlin-serialization:2.2.20")
    }
}

plugins {
    application
}

apply(plugin = "org.jetbrains.kotlin.jvm")
apply(plugin = "org.jetbrains.kotlin.plugin.serialization")

group = "com.comfyhub"
version = "1.0.0"

repositories {
    mavenCentral()
}

val ktorVersion = "3.3.3"
val logbackVersion = "1.5.18"

dependencies {
    // --- Ktor server ---
    implementation("io.ktor:ktor-server-core-jvm:$ktorVersion")
    implementation("io.ktor:ktor-server-netty-jvm:$ktorVersion")
    implementation("io.ktor:ktor-server-content-negotiation-jvm:$ktorVersion")
    implementation("io.ktor:ktor-serialization-kotlinx-json-jvm:$ktorVersion")
    implementation("io.ktor:ktor-server-cors-jvm:$ktorVersion")
    implementation("io.ktor:ktor-server-call-logging-jvm:$ktorVersion")
    implementation("io.ktor:ktor-server-status-pages-jvm:$ktorVersion")
    implementation("io.ktor:ktor-server-partial-content-jvm:$ktorVersion")
    implementation("io.ktor:ktor-server-compression-jvm:$ktorVersion")
    implementation("io.ktor:ktor-server-default-headers-jvm:$ktorVersion")

    // --- JSON ---
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.9.0")

    // --- MySQL + 连接池（原生 JDBC，直接用 MySQL 方言特性） ---
    implementation("com.zaxxer:HikariCP:6.3.3")
    implementation("com.mysql:mysql-connector-j:9.4.0")

    // --- 图片解码：给 JVM 的 ImageIO 补一个 **纯 Java** 的 WebP 读取器 ---
    // 为什么需要它：JDK 自带的 ImageIO 读不了 WebP（附件与画廊里的 .webp 都是"有图没缩略图"，
    // 只能退化成文件图标）。TwelveMonkeys 这个插件是纯 Java（无本地库、无外部进程），
    // 与 MediaFiles 的"零外部二进制"前提不冲突：它只补一个 ImageReaderSpi 注册。
    // 注意：它只能**读**（缩略图仍然写 JPEG），且只有 WebP —— AVIF/HEIC 依旧走"回退原件"那条路。
    implementation("com.twelvemonkeys.imageio:imageio-webp:3.12.0")

    // --- 日志 ---
    implementation("ch.qos.logback:logback-classic:$logbackVersion")

    testImplementation(kotlin("test"))
}

java {
    sourceCompatibility = JavaVersion.VERSION_21
    targetCompatibility = JavaVersion.VERSION_21
}

tasks.withType<org.jetbrains.kotlin.gradle.tasks.KotlinCompile>().configureEach {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_21)
        freeCompilerArgs.add("-Xjsr305=strict")
    }
}

application {
    mainClass.set("com.comfyhub.ApplicationKt")
    applicationDefaultJvmArgs = listOf(
        "-Dfile.encoding=UTF-8",
        // 认本机的系统代理（Windows 的 WinINET 设置）。Java 默认**一律直连**，
        // 于是"本机要走 127.0.0.1:7890 才能出网"的机器上，所有出网工具都会连接超时
        // （用户报的 "civitai_search 反复超时"）。真正的取舍与"内网永不代理"的旁路
        // 都在 com.comfyhub.ai.OutboundProxy 里，这里只负责把系统代理**读得到**。
        "-Djava.net.useSystemProxies=true",
        // ↓ P0 内存调优（2026-09-23，实测见 docs/memory-and-storage-evaluation.md §3.2.2）：
        //   同一份代码同一个库，只换启动参数 —— 私有提交 378.6→150.5MB、线程 74→35。
        //   关键收益不是"平均更低"（Windows 本来会把空闲 JVM 的页换出去），
        //   而是**把峰值压平**：默认参数跑一次内置模型种子写入会冲到 272.6MB，调优后停 141.3MB。
        "-Xms64m",                        // 初始堆 394MB → 64MB
        "-Xmx256m",                       // 上限 6.25GB → 256MB（实测实用堆 135MB，留一倍余量）
        "-XX:MaxMetaspaceSize=192m",      // 实测 42MB
        "-XX:+UseSerialGC",               // 省掉 16 个 GC 线程 + G1 的 Remembered Set（GC 结构仅 983KB）
        "-Xss512k",                       // 线程栈减半
        "-XX:CICompilerCount=2",          // 单用户应用不需要 4 个 JIT 线程
        "-XX:ReservedCodeCacheSize=48m",  // 实测 Code Cache 用 8.2MB
        "-XX:MaxDirectMemorySize=32m",    // Netty 直接内存封顶
    )
}

tasks.test {
    useJUnitPlatform()
}

// 打包成可直接运行的 fat jar: build/libs/comfy-hub-server-1.0.0-all.jar
tasks.register<Jar>("fatJar") {
    group = "build"
    description = "Build a self-contained runnable jar"
    archiveClassifier.set("all")
    duplicatesStrategy = DuplicatesStrategy.EXCLUDE
    manifest {
        attributes["Main-Class"] = "com.comfyhub.ApplicationKt"
    }
    from(sourceSets.main.get().output)
    dependsOn(configurations.runtimeClasspath)
    from({
        configurations.runtimeClasspath.get()
            .filter { it.name.endsWith("jar") }
            .map { zipTree(it) }
    }) {
        exclude("META-INF/*.SF", "META-INF/*.DSA", "META-INF/*.RSA", "module-info.class")
    }
}
