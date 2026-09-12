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
    applicationDefaultJvmArgs = listOf("-Dfile.encoding=UTF-8")
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
