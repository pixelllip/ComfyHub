# Android 构建迁移到 AGP 9.4 + 内建 Kotlin（Flutter 3.47）

> 只影响 `android/` 目录（Flutter App 的 Android 构建）。
> `server/` 是另一个独立的 Gradle 8.12 工程，跟本文无关。

## 0. TL;DR

Flutter 3.47 的模板虽然已经把版本号写成 AGP 9.1.0 / Gradle 9.3.1，但默认仍**关着** AGP 9 的内建 Kotlin
（`android.builtInKotlin=false`），也就是"用 AGP 9 跑 AGP 8 时代的 Kotlin Gradle Plugin（KGP）"。
本次做两件事：把 Kotlin 编译真正交还给 AGP，并把 AGP / Gradle 顶到最新 stable。

| 组件 | 迁移前 | 迁移后 |
| --- | --- | --- |
| Flutter | 3.47.3 | 3.47.3 |
| AGP | 9.1.0 | **9.4.0**（2026-09 最新 stable；9.5.0 还是 alpha，不用） |
| Gradle（Android） | 9.3.1 | **9.6.0**（AGP 9.4.0 的最低/默认 Gradle 版本） |
| Gradle 分发包校验 | 没有 | 加了 `distributionSha256Sum`（见踩坑 P8） |
| Kotlin 编译方式 | KGP（`org.jetbrains.kotlin.android`） | **AGP 内建 Kotlin**（`android.builtInKotlin=true`） |
| KGP 版本 | 2.4.0（settings 里声明并 apply 由 Flutter 自动做） | 2.4.0（**只进 classpath，不 apply**，见踩坑 P1） |
| AGP 新 DSL | 关闭 | 关闭（Flutter 3.47 自己也没迁完，见踩坑 P5） |
| app 模块的 Kotlin 插件 | Flutter 在配置期自动 apply `kotlin-android` | 不用 apply，由 AGP 内建 Kotlin 接手 |
| Kotlin 增量编译 | 开 | 关（跨盘 bug，见踩坑 P3） |
| file_picker | 12.2.0 | 12.3.0 |

> **版本怎么选**：AGP 各版本对 Gradle 的最低要求（官方 release notes 的"最低版本"表）——
> 9.1.0 → Gradle 9.3.1、9.2.x → 9.4.1、9.3.x → 9.5.0、9.4.0 → 9.6.0。
> 升 AGP 必须同时升 Gradle，所以这里选的是"最新 stable AGP + 它要求的 Gradle"。
>
> 注意 Flutter 3.47.3 的工具链只把 **AGP ≤ 9.2、Gradle ≤ 9.3.1** 记成"已知"版本
> （`flutter_tools/lib/src/android/gradle_utils.dart` 的 `maxKnownAndSupportedAgpVersion` /
> `maxKnownAndSupportedGradleVersion`）。比这更新的版本不会被拦，只会走
> "Newer than known ..., Treating as valid configuration" 这条 trace 分支 ——
> 也就是说 **9.4.0 + 9.6.0 是我们的实测结论，不是 Flutter 官方背书的组合**。

工程配置动了 5 个文件：`android/gradle.properties`、`android/settings.gradle.kts`、
`android/app/build.gradle.kts`、`android/gradle/wrapper/gradle-wrapper.properties`、`pubspec.yaml`
（外加 `pubspec.lock` 与 `README.md` / `AGENTS.md` 的文档登记）。

## 1. 迁移前的问题是什么

`android/` 的文件与 Flutter 3.47 模板一致，但 `android/gradle.properties` 里有两行兼容开关：

```properties
android.newDsl=false
android.builtInKotlin=false
```

`android.builtInKotlin=false` 意味着：

- Flutter Gradle 插件会在配置期为**所有**应用了 AGP 的子工程（包括每个 Flutter 插件模块）
  自动 `apply("kotlin-android")`（`FlutterPluginUtils.detectApplyingKotlinGradlePlugin`）；
- AGP 9 的内建 Kotlin 被关掉，Kotlin 编译仍然由 KGP 提供。

Flutter 从 3.47 起才支持把内建 Kotlin 打开，并且计划在未来版本里**移除**对 KGP 的支持
（flutter/flutter#184837）；AGP 10 也会直接禁止 KGP。所以这一步是迟早要做的。

## 2. 改了什么

### 2.1 `android/gradle.properties`

```diff
 android.useAndroidX=true
-# This newDsl flag was added by the Flutter template
-android.newDsl=false
-# This builtInKotlin flag was added by the Flutter template
-android.builtInKotlin=false
+
+# 显式打开内建 Kotlin（不能靠"不写"来打开，见踩坑 P5）
+android.builtInKotlin=true
+
+# pub 缓存在 C:、工程在 D:，Kotlin 增量缓存跨盘会炸（见踩坑 P3）
+kotlin.incremental=false
+
+# Flutter 3.47 仍未完成新 DSL 迁移，保持关闭
+android.newDsl=false
```

### 2.2 `android/settings.gradle.kts`

`org.jetbrains.kotlin.android` 从"给 Flutter 自动 apply 用"变成"只用来把 KGP 抬到 Flutter 的最低版本之上"：

```kotlin
plugins {
    id("dev.flutter.flutter-plugin-loader") version "1.0.0"
    id("com.android.application") version "9.4.0" apply false
    // 只声明版本、不 apply：见踩坑 P1
    id("org.jetbrains.kotlin.android") version "2.4.0" apply false
}
```

### 2.3 `android/app/build.gradle.kts`

`plugins {}` 里**没有**也不需要 kotlin 插件；`kotlin.compilerOptions {}` 属于 AGP 提供的 `kotlin` 扩展
（旧的 `android.kotlinOptions {}` 写法已废弃）：

```kotlin
plugins {
    id("com.android.application")
    id("dev.flutter.flutter-gradle-plugin")
}

kotlin {
    compilerOptions {
        jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17
    }
}
```

> `jvmTarget` 在内建 Kotlin 下默认等于 `android.compileOptions.targetCompatibility`，
> 这里显式写出来只是与 Flutter 官方迁移文档保持一致。

### 2.4 `android/gradle/wrapper/gradle-wrapper.properties`

AGP 9.4.0 要求 Gradle ≥ 9.6.0，所以 wrapper 也跟着升，并顺手加上官方校验和：

```diff
-distributionUrl=https\://services.gradle.org/distributions/gradle-9.3.1-all.zip
+distributionUrl=https\://services.gradle.org/distributions/gradle-9.6.0-all.zip
+# 来自 https://services.gradle.org/distributions/gradle-9.6.0-all.zip.sha256
+distributionSha256Sum=87a2216cc1f9122192d4e0fe905ffdf1b4c72cff797e9f733b174e157cadd396
```

### 2.5 `pubspec.yaml`

`file_picker: ^12.2.0 → ^12.3.0`，并跑 `flutter pub upgrade` 更新 `pubspec.lock`
（连带 `file_picker_darwin 1.2.0` / `file_picker_platform_interface 3.4.0` / `windows_file_picker 1.3.0`）。

顺带确认了现用插件的 Android 侧状态（`audioplayers_android 5.3.0`、`shared_preferences_android 2.4.28`、
`url_launcher_android 6.3.33`、`android_file_picker 1.1.1`、`jni 1.0.3`）：**没有**任何模块在 `plugins {}` 块里
apply KGP，所以打开内建 Kotlin 后 Flutter 不会报"插件未迁移"。

## 3. 依据的官方文档

- Flutter：<https://docs.flutter.dev/release/breaking-changes/migrate-to-built-in-kotlin/for-app-developers>
  （以及总览页 `.../migrate-to-built-in-kotlin`）
- Android：<https://developer.android.com/build/migrate-to-built-in-kotlin>
  （要点：AGP 9 默认开内建 Kotlin；要 opt-out 必须同时 `android.builtInKotlin=false` + `android.newDsl=false`；
  内建 Kotlin 版本 = AGP 运行时依赖的 KGP 版本，可以用 classpath 上的更高版本覆盖）
- 版本门槛（Flutter 3.47 内建）：`DependencyVersionChecker` 的
  `warnKGPVersion=2.3.20` / `errorKGPVersion=2.2.20`，`warnAGPVersion=9.0.1` / `errorAGPVersion=8.11.1`，
  `warnGradleVersion=9.1.0` / `errorGradleVersion=8.14.0`。

## 4. 踩坑记录

### P1（最坑）删掉 KGP 声明后，Flutter 的 Kotlin 版本校验把构建拦死在配置期

**现象**

```
An exception occurred applying plugin request [id: 'dev.flutter.flutter-gradle-plugin']
> Failed to apply plugin 'dev.flutter.flutter-gradle-plugin'.
   > Error: Your project's Kotlin version (2.2.10) is lower than Flutter's minimum supported
     version of 2.2.20. Please upgrade your Kotlin version.
```

**原因**

内建 Kotlin 的编译器版本来自 **AGP 运行时依赖的 KGP**，而不是你声明的插件版本：
`com.android.tools.build:gradle:9.1.0`（以及 **9.4.0**，实测两版一样）的 POM 里写着
`org.jetbrains.kotlin:kotlin-gradle-plugin:2.2.10`。
把 `id("org.jetbrains.kotlin.android")` 从 settings 的 `plugins {}` 里删掉之后，classpath 上最高的 KGP
就只剩 2.2.10，低于 Flutter 3.47 要求的 2.2.20 → 直接抛错。
这是 Flutter 侧已知问题 [flutter/flutter#192167](https://github.com/flutter/flutter/issues/192167)
（3.47.3 里还没有那个"`builtInKotlin=true` 时跳过 KGP 校验"的修复）。

> 别把两个"Kotlin 版本"搞混：
> - **Gradle 自带的 Embedded Kotlin**（跑 `.gradle.kts` 脚本用的），和 Android 编译无关；
> - **AGP 内建 Kotlin 用的 KGP**，就是上面这个。
> 看到 Gradle 兼容性表里写着 Kotlin 2.3.x，不代表内建 Kotlin 也是 2.3.x。

**处理**

在 settings 的 `plugins {}` 里**保留** `id("org.jetbrains.kotlin.android") version "2.4.0" apply false`：

- Gradle 会用冲突解析取 classpath 上最高的 KGP（2.4.0 > AGP 的 2.2.10），
  Flutter 的校验和 AGP 的内建 Kotlin 就都拿到 2.4.0；
- `apply false` 表示**没有任何模块真正 apply 它**，所以不会和内建 Kotlin 抢 `kotlin` 扩展名
  （同时 apply 才会报 `Cannot add extension with name 'kotlin'`）；
- 这也正是 AGP 官方推荐的"覆盖内建 Kotlin 版本"的方式。

验证：`android\gradlew.bat :app:kgpVersion` → `KGP Version: 2.4.0`。

等 Flutter 带上 #192167 的修复、或 AGP 自带的 KGP ≥ 2.2.20 之后，这行可以删掉。

### P2 Flutter 打印的 "opt out of android.newDsl" 提示是**误报**

**现象**：构建失败时 Flutter 额外弹了一个框：

```
[!] Starting AGP 9+, only the new DSL interface will be read.
This results in a build failure when applying the Flutter Gradle plugin at .../app/build.gradle.kts.
To resolve this update flutter or opt out of `android.newDsl`.
```

但 `android.newDsl=false` 明明已经写在 `gradle.properties` 里。

**原因**：这个框来自 `flutter_tools/lib/src/android/gradle_errors.dart` 的 `useNewAgpDslErrorHandler`，
它的匹配条件只是这一行文本：

```
> Failed to apply plugin 'dev.flutter.flutter-gradle-plugin'
```

**任何**在 apply 这个插件时抛出的异常（包括 P1 的 Kotlin 版本校验）都会命中它。

**处理**：别被它带跑偏，往上看真正的 `* What went wrong:`。本次真正的错因是 P1。

### P3 `Could not close incremental caches` / `this and base files have different roots`

**现象**（`flutter build apk --debug`）：

```
Execution failed for task ':shared_preferences_android:compileDebugKotlin'.
> java.lang.Exception: Could not close incremental caches in ...\build\shared_preferences_android\kotlin\
  compileDebugKotlin\cacheable\caches-jvm\jvm\kotlin: class-fq-name-to-source.tab, ...
```

被 `Suppressed` 掉的真正异常是：

```
java.lang.IllegalArgumentException: this and base files have different roots:
  C:\Users\Administrator\AppData\Local\Pub\Cache\hosted\pub.dev\shared_preferences_android-2.4.28\android\src\main\kotlin\...kt
  and D:\myProject\FlutterProject\viewer\android.
  at kotlin.io.FilesKt__UtilsKt.toRelativeString(Utils.kt:119)
  at org.jetbrains.kotlin.incremental.storage.RelocatableFileToPathConverter.toPath(...)
```

**原因**：本机 pub 缓存在 `C:`、工程在 `D:`。Kotlin 增量编译的"可重定位缓存"
（`RelocatableFileToPathConverter`）要把源文件路径相对于工程目录转成相对路径，跨盘时 `relativeTo()`
直接抛异常；随后关闭缓存时又把它包成 `Could not close incremental caches` ——
**报错文案指的是缓存，根因却是跨盘**。（同一台机器上，Windows 上 `C:`/`D:` 是两个 root。）

上游 [flutter/flutter#187225](https://github.com/flutter/flutter/issues/187225) 遇到的正是这个报错，
最后没定位到根因就关了（怀疑过杀软 / OneDrive / `flutter clean`，都不是）。

**处理**：`android/gradle.properties` 里加 `kotlin.incremental=false`（对所有 Kotlin 模块生效，
包括 pub 缓存里的插件模块）。代价是 Kotlin 编译不再增量；想彻底解决可以把 `PUB_CACHE` 挪到 `D:` 盘
（和工程同一个 root），然后删掉这行。

### P4 本机 Gradle 代理把 dl.google.com / Maven Central 打破

**现象**：`ERROR: Could not resolve ... / Could not GET 'https://dl.google.com/...'`，
伴随 `The server may not support the client's requested TLS protocol versions` /
`Remote host terminated the handshake`。

**原因**：`~/.gradle/gradle.properties` 里配了本机代理 `127.0.0.1:7890`。实测（curl）：

| 主机 | 直连 | 走 7890 代理 |
| --- | --- | --- |
| `dl.google.com`（Google Maven） | 200 | **TLS 握手失败** |
| `repo.maven.apache.org`（Maven Central） | 200 | **TLS 握手失败** |
| `plugins.gradle.org` / `services.gradle.org` / `pub.dev` | 200 | 200 |
| `github.com` | **不通** | 200 |

所以代理不能整个关掉（GitHub 还得靠它），只能让两个大仓库绕过代理。

**处理**：在 `~/.gradle/gradle.properties` 里加一行（Java 只有 `http.nonProxyHosts` 这一个属性，
**https 也读它**，没有 `https.nonProxyHosts`）：

```properties
systemProp.http.nonProxyHosts=dl.google.com|*.google.com|*.googleapis.com|*.gstatic.com|repo.maven.apache.org|repo1.maven.org|*.maven.apache.org|*.maven.org|localhost|127.0.0.1
```

改完先 `android\gradlew.bat --stop`（系统属性是启动 daemon 时读的，不换 daemon 不生效）。

### P5 `android.builtInKotlin` 想打开必须**显式写 true**

Flutter 的 `DisableBuiltInKotlinMigration`（`flutter_tools/lib/src/android/migrations/`）
只有在属性**完全缺失**时才补 `android.builtInKotlin=false`；写成 `true` 它就跳过。
所以：

- 不能靠"把 `android.builtInKotlin=false` 删掉"来打开内建 Kotlin —— 下次 `flutter run/build`
  会被自动补回 `false`；
- 属性名**大小写敏感**，`android.builtinKotlin` / `android.builtInKotlin = true` 之类都算没配；
- `android.newDsl` 同理（Flutter 也会自动补 `false`）。这里保持 `false` **不是"漏迁移"**：
  Flutter 3.47 官方仍默认关闭它，因为大量 Flutter 插件的 `android/build.gradle` 还在用旧 DSL 类型
  （flutter/flutter#180137、#184838），等 #184839 落地后再打开。

### P6 插件模块会带自己的旧 buildscript classpath

这些不是你写的，但每次配置都要解析，网络问题和版本问题会先从这里冒出来：

| 插件 | 自己的 buildscript 里固定了 |
| --- | --- |
| `audioplayers_android 5.3.0` | `com.android.tools.build:gradle:7.3.1` + `kotlin-gradle-plugin:1.7.10` + `de.mannodermaus.android-junit5:1.7.1.1` |
| `shared_preferences_android 2.4.28` | `com.android.tools.build:gradle:8.13.1` + `kotlin-gradle-plugin:2.3.0` |
| `url_launcher_android 6.3.33` | 同 shared_preferences |
| `android_file_picker 1.1.1` | `com.android.tools.build:gradle:8.5.2` + `kotlin-gradle-plugin:1.8.22`（注释里写明是为 `android.newDsl=false` 打的临时补丁） |

结论：**本机的 Google Maven 必须通**（P4 就是为了这个），否则配置期直接失败。

### P7 "没有告警" ≠ "插件已经迁移到内建 Kotlin"

Flutter 判断某模块是否 apply 了 KGP，用的是**正则扫 `<module>/build.gradle(.kts)` 里 `plugins {}` 块**
（`FlutterPluginUtils.kgpRegexKotlin/kgpRegexGroovy`）。所以：

- `android_file_picker` 在 `plugins {}` 块外用 `apply(plugin = "org.jetbrains.kotlin.android")`
  （还带 `shouldApplyKotlinAndroidPlugin` 条件判断）→ 扫不到，不告警，但它确实还在用 KGP 兜底；
- `audioplayers_android` 是 Groovy 文件、压根没写 kotlin 插件 —— 迁移前是靠 Flutter 的
  `detectApplyingKotlinGradlePlugin` 自动 `apply("kotlin-android")` 才编得过；打开内建 Kotlin 后
  这条路不再走（`isBuiltInKotlinEnabled` 为真时 Flutter 不 apply），改由 AGP 编译，
  它自己的 `kotlin { compilerOptions { ... } }` 由 AGP 提供的 `kotlin` 扩展解析。

**想确认某个模块到底走哪条路，看产物目录最可靠**：

```
build/<module>/intermediates/built_in_kotlinc/debug/compileDebugKotlin/classes/...   # AGP 内建 Kotlin
build/<module>/kotlin/compileDebugKotlin/...                                        # KGP
```

### P8 升 Gradle 时：wrapper 下载会 307 跳到 GitHub，而本机 GitHub 直连不通

**现象**：`flutter build apk` 刚起头就炸。注意栈里是 **Gradle wrapper 自己**（`org.gradle.wrapper.*`），
不是 Gradle 构建过程：

```
Exception in thread "main" javax.net.ssl.SSLHandshakeException: Remote host terminated the handshake
	at java.base/sun.net.www.protocol.http.HttpURLConnection.followRedirect0(HttpURLConnection.java:...)
	at org.gradle.wrapper.Download.downloadInternal(Download.java:58)
	at org.gradle.wrapper.Install.createDist(Install.java:48)
```

**原因**：`services.gradle.org/distributions/gradle-9.6.0-all.zip` 会 **307 跳到
`github.com/gradle/gradle-distributions/releases/...`**，再 302 到
`release-assets.githubusercontent.com`。本机 github.com 直连不通（必须走代理），
而 **wrapper 跑在 Gradle 之前，不会读 `~/.gradle/gradle.properties` 里的 `systemProp.*` 代理设置**，
于是它直连 GitHub → 握手失败。

**处理**（一次性，之后 wrapper 缓存一直有效）：

```powershell
# 1) 官方校验和（services.gradle.org 直连可达）
curl.exe -sSL https://services.gradle.org/distributions/gradle-9.6.0-all.zip.sha256

# 2) 从一个不走 GitHub 的镜像拉包（腾讯/华为镜像都行），并核对校验和
curl.exe -sSL -o "$env:TEMP\gradle-9.6.0-all.zip" `
  https://mirrors.cloud.tencent.com/gradle/gradle-9.6.0-all.zip
(Get-FileHash "$env:TEMP\gradle-9.6.0-all.zip" -Algorithm SHA256).Hash   # 必须等于第 1 步的值

# 3) 塞进 wrapper 的缓存目录（目录名是 wrapper 自己按 URL 算出来的 hash，
#    第一次失败的下载已经帮你建好了，里面是 .part/.lck）
$dst = "$env:USERPROFILE\.gradle\wrapper\dists\gradle-9.6.0-all\*\"
Remove-Item "$dst\*.part","$dst\*.lck" -Force
Copy-Item "$env:TEMP\gradle-9.6.0-all.zip" "${dst}gradle-9.6.0-all.zip"

# 4) 验证：不再联网下载，直接解压启动
cd android; .\gradlew.bat --version      # 应该打印 Gradle 9.6.0
```

配了 `distributionSha256Sum` 之后，wrapper 会校验下到的包，用镜像也不怕被掉包。
（另一条路是给 wrapper 传 `GRADLE_OPTS=-Dhttps.proxyHost=127.0.0.1 -Dhttps.proxyPort=7890` 让它走代理；
本次没走这条路，因为环境变量是机器级改动、比塞缓存更"重"。）

## 5. 怎么验证这次迁移是成功的

```powershell
$env:PUB_HOSTED_URL='https://pub.dev'

# 1) 配置期 + 编译期全绿
flutter build apk --debug
#    √ Built build\app\outputs\flutter-apk\app-debug.apk

# 2) 输出里不应出现下面这行（出现就是还有模块在用 KGP）
#    WARNING: Your app uses the following plugins that apply Kotlin Gradle Plugin (KGP): ...

# 3) app 自己的 Kotlin 是内建 Kotlin 编的（路径里有 built_in_kotlinc）
#    build\app\intermediates\built_in_kotlinc\debug\compileDebugKotlin\classes\com\example\viewer\MainActivity.class

# 4) 内建 Kotlin 实际用的 KGP 版本
cd android; .\gradlew.bat :app:kgpVersion     # KGP Version: 2.4.0

# 5) Dart 侧回归
cd ..; flutter analyze; flutter test
```

本次实测结果（2026-09-12）：

- **AGP 9.4.0 + Gradle 9.6.0**：`app-debug.apk` 构建成功（`√ Built build\app\outputs\flutter-apk\app-debug.apk`），
  构建日志里没有 KGP 告警、也没有 AGP 的废弃/移除类警告；
- `app` / `android_file_picker` / `audioplayers_android` / `shared_preferences_android` 的 Kotlin 产物
  都在 `built_in_kotlinc` 下（说明确实走的是 AGP 内建 Kotlin）；
- `gradlew :app:kgpVersion` → `KGP Version: 2.4.0`，`:app:printBuildVariants` → debug / release / profile；
- `flutter analyze` 无问题；`flutter test` 32 个用例全过；`flutter build windows --release` 也通过
  （覆盖 `file_picker 12.3.0` 的依赖升级）。

> 之前 AGP 9.1.0 + Gradle 9.3.1 的组合也完整验证过一次（同一套命令、同样全绿），
> 所以这次只是把版本顶到最新 stable，迁移方式没有变。

## 6. 后续可以再往前走的地方

| 什么时候 | 可以做什么 |
| --- | --- |
| Flutter 带上 flutter#192167 的修复（或 AGP 自带 KGP ≥ 2.2.20） | 删掉 `settings.gradle.kts` 里的 `org.jetbrains.kotlin.android ... apply false`（AGP 9.4.0 实测仍带 2.2.10，所以现在还得留着） |
| Flutter 完成新 AGP DSL 迁移（flutter#180137 / #184839） | 把 `android.newDsl` 改成 `true`（或删掉，让它跟随上游默认值）。AGP 9.4 已经给了按模块过渡的 `android.newDsl.optOut=:模块名`，AGP 10 起强制新 DSL |
| Flutter 把 AGP/Gradle 的"已知版本"往上抬（现在只到 AGP 9.2 / Gradle 9.3.1） | 之后 `flutter analyze --suggestions` 之类的版本判断才会覆盖 9.4.0 / 9.6.0；在那之前我们这套组合属于"Flutter 未背书但实测可用" |
| 把 `PUB_CACHE` 挪到 D 盘（与工程同一个 root） | 删掉 `kotlin.incremental=false`，恢复增量编译 |
| 官方 Gradle 分发包改成不经 GitHub（或本机网络能直连 GitHub） | 那时不再需要 P8 的手工塞缓存步骤 |
| `audioplayers_android` / `android_file_picker` 上游发布内建 Kotlin 版本 | 插件模块里那些旧 AGP/KGP buildscript classpath 会自然消失 |

## 7. 参考链接

- [Flutter：Built-in Kotlin migration for app developers](https://docs.flutter.dev/release/breaking-changes/migrate-to-built-in-kotlin/for-app-developers)
- [Flutter：Migrating Flutter Android projects to built-in Kotlin（总览）](https://docs.flutter.dev/release/breaking-changes/migrate-to-built-in-kotlin)
- [Android：Migrate to built-in Kotlin](https://developer.android.com/build/migrate-to-built-in-kotlin)
- AGP release notes（每页顶部有"最低版本"表，写明了要求的 Gradle）：
  [9.1](https://developer.android.com/build/releases/agp-9-1-0-release-notes) /
  [9.2](https://developer.android.com/build/releases/agp-9-2-0-release-notes) /
  [9.3](https://developer.android.com/build/releases/agp-9-3-0-release-notes) /
  [9.4](https://developer.android.com/build/releases/agp-9-4-0-release-notes)
- [AGP 9.0 release notes（含对 KGP 的运行时依赖）](https://developer.android.com/build/releases/agp-9-0-0-release-notes)
- flutter/flutter#192167（内建 Kotlin 的 KGP 版本校验）、#187225（跨盘增量缓存）、
  #180137 / #184838 / #184839（新 AGP DSL 迁移）
