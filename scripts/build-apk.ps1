<#
.SYNOPSIS
    打一份**能装进真手机**的安卓 APK：一条命令走完「编 → 报出装机要看的那些事实」。
    只出 **arm64-v8a**（真机基本都是这个架构）。

.DESCRIPTION
    为什么值得单独一个脚本（2026-09-28 用户 bug：「真手机无法安装手机端 app」）：

    ① **在这之前，装手机端是没有任何入口的**。仓库里 30 多个脚本没有一个是安卓的，
       用户只能自己敲 `flutter build apk` —— 包是出来了，但它对应哪个 ABI、versionCode 多少、
       用的哪份签名、手机上那份能不能覆盖，**一样都看不出来**；顺手加个 `--debug` 就是
       上百 MB（2026-10-29 的全量 debug 实测 212MB），而 `INSTALL_FAILED_*` 每一类要看的东西又完全不同。

    ② **装机失败时看不到任何判据**。APK 装不上无非这几类原因，而它们各自要看的东西
       完全不一样：ABI 与手机对不上（`native-code`）、系统版本低于 `minSdkVersion`、
       手机里已经装着一个**同包名但不同签名**的旧版（`INSTALL_FAILED_UPDATE_INCOMPATIBLE`）、
       或者 APK 在传输途中被改坏了（签名校验不过）。所以本脚本编完之后
       **把这些事实一次性打出来**，而不是只丢一句"编译成功"。

    ③ **只出 arm64-v8a 单架构包**（`--target-platform android-arm64`）：现在真机基本都是
       arm64，包只有以前全量包（三个 ABI）的 1/3~1/2，装机也最快。`armeabi-v7a`（老 32 位机）
       与 `x86_64`（模拟器）不再出 —— 需要它们时只能手工 `flutter build apk`。

    签名说明：`android\app\build.gradle.kts` 的 release 用的是 **debug 签名**
    （Flutter 模板的默认做法，`flutter run --release` 靠它才跑得起来）。
    debug 签名**能装**，但和别的机器上编出来的包**签名不同** ——
    手机上如果已经装过一份同包名的，必须先卸干净再装，否则必然
    `INSTALL_FAILED_UPDATE_INCOMPATIBLE`（这一条也打在报告里）。

.EXAMPLE
    pwsh -File scripts\build-apk.ps1                 # arm64-v8a release 包
    pwsh -File scripts\build-apk.ps1 -DebugBuild     # arm64-v8a debug 包（能连 flutter attach）
    pwsh -File scripts\build-apk.ps1 -Install        # 编完顺手 adb install -r（接了手机才用）
#>
[CmdletBinding()]
param(
    # 打 debug 包。⚠ 仍是 arm64-v8a 单架构；只在"要 flutter attach 调试"时才用。
    # ⚠ 名字**不能**叫 `-Debug`：那是 `[CmdletBinding()]` 的公共参数，重名会直接
    #   报 `A parameter with the name 'Debug' was defined multiple times`（本次实测踩到）。
    [switch]$DebugBuild,
    # 编完调 adb install -r（手机上会弹安装确认）。没接设备就跳过，不算失败
    [switch]$Install,
    # 跳过签名/架构体检那一趟（只想快点拿到文件时用）
    [switch]$NoReport
)

$ErrorActionPreference = 'Stop'
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }

$Scripts = $PSScriptRoot
$ProjectRoot = Split-Path -Parent $Scripts

# 装机那一段的结果：装了但没成功时要**退出码非 0**，别一路"OK"结尾
$installOk = $false
$installTried = $false

function Say([string]$Text, [string]$Color = 'Gray') { Write-Host $Text -ForegroundColor $Color }

if (-not (Test-Path (Join-Path $ProjectRoot 'pubspec.yaml'))) {
    Say "找不到 $ProjectRoot\pubspec.yaml —— 这个脚本要在源码树里跑。" 'Red'
    exit 2
}

# pub 源必须显式指到 pub.dev：国内镜像对个别包返回 424（AGENTS §4）
$env:PUB_HOSTED_URL = 'https://pub.dev'

$buildArgs = @('build', 'apk')
$buildArgs += if ($DebugBuild) { '--debug' } else { '--release' }
# ★ 只构建 arm64-v8a：真机以 arm64 为主；x86_64 仅用于模拟器，
#   armeabi-v7a 是 32 位老机。`--target-platform` 与 `--split-per-abi` 一起用，产物名
#   固定带架构（`app-arm64-v8a-{debug,release}.apk`），不会被误当成全量包。
$buildArgs += '--target-platform'
$buildArgs += 'android-arm64'
$buildArgs += '--split-per-abi'
# ★ 分架构包**不额外加 ABI 的 versionCode 偏移**（2026-09-28 起）。
#
# 不加这个的后果是**永久性**的：分架构包会拿到 1001 / 2001 / 4001，而
# `flutter run` 装的 debug 包是 1 —— 于是"装过一次分架构包"的手机，之后
# **`flutter run` 永远装不上去**（INSTALL_FAILED_VERSION_DOWNGRADE，`-r` 救不了，
# 只能先卸载）。这会导致真机无法安装，已复现。
#
# 加了它，本脚本的 debug / release **共用同一个 versionCode**（与 `flutter run` 装的
# debug 包一致），互相覆盖都是普通的"同版本替换"，`adb install -r` 与 `flutter run` 都能过。
# （这个开关是 Flutter 模板自己在 `android\app\build.gradle.kts` 里写明的，见那段注释。）
$buildArgs += '-Pforce-version-code-ignoring-abi=true'

# JDK：Gradle 9.6 要**完整 JDK**（带 jlink），JBR jcef 只有运行时。
# 判据不靠版本号，靠 `jlink.exe` 在不在 —— 缺了它 Gradle 会在
# `JdkImageTransform` 里报 "jlink executable ... does not exist"，而错误里不会
# 说"你给的不是 JDK"。优先 COMFYHUB_JDK_HOME（AGENTS §4），其次 JAVA_HOME。
$jdk = if ($env:COMFYHUB_JDK_HOME) { $env:COMFYHUB_JDK_HOME } else { $env:JAVA_HOME }
if ($jdk) {
    if (-not (Test-Path (Join-Path $jdk 'bin\jlink.exe'))) {
        Say "✗ $jdk 不是完整 JDK（缺 bin\jlink.exe）。JBR jcef 只有运行时，编不了 Android。" 'Red'
        Say '  换一个**完整 JDK**（要有 bin\jlink.exe，比如 Temurin / Oracle / Microsoft OpenJDK 的 JDK 发行包），或删掉环境变量让 Flutter 自己挑。' 'Yellow'
        exit 2
    }
    $env:JAVA_HOME = $jdk
    $env:PATH = (Join-Path $jdk 'bin') + ';' + $env:PATH
    Say "JDK: $jdk" 'DarkGray'
}

Say ("→ flutter " + ($buildArgs -join ' ')) 'Cyan'
Say '  （第一次编要几分钟：Gradle 要编译 Kotlin 与各插件）' 'DarkGray'
# 记下开始时间：Flutter 不会清理旧产物（见下面"只认本次构建的文件"）
$buildStarted = Get-Date
& flutter @buildArgs
if ($LASTEXITCODE -ne 0) {
    Say '' 'Red'
    Say "✗ flutter build apk 失败（退出码 $LASTEXITCODE）。" 'Red'
    Say '  常见两类：① JAVA_HOME 不是完整 JDK（缺 bin\jlink.exe）或版本过低（AGP 9 要 17+，本机 25 实测可用）；' 'Yellow'
    Say '  ② android\ 下的 AGP/Kotlin 配置被动过，' 'Yellow'
    Say '  先读 docs\android-agp9-builtin-kotlin-migration.md。' 'Yellow'
    exit 1
}

$outDir = Join-Path $ProjectRoot 'build\app\outputs\flutter-apk'
# ★ 只认**这一次**构建出来的文件。
#
# 为什么不能 `Get-ChildItem *.apk`：Flutter 不清理输出目录，所以编完 release 之后，
# 目录里还躺着上次的 debug / 全量 / 分架构包（实测：一次 `--release` 之后目录里有 8 个包，
# 其中三个还是 0.2.0 的旧构建）。那样做有三个后果，全是"看着成功、其实拿错包"：
#   ① 「拿去装机」输出旧包与旧 sha256，导致复制错误的包；
#   ② `-Install` 会把旧包也装一遍（`-DebugBuild -Install` 最后装上的反而是旧 release）；
#   ③ 体检报的是旧包的 minSdk / 签名。
# 产物名由 Flutter 固定：本脚本只出 app-arm64-v8a-{debug,release}.apk。
$mode = if ($DebugBuild) { 'debug' } else { 'release' }
$expected = @("app-arm64-v8a-$mode.apk")
$allApks = @(Get-ChildItem $outDir -Filter '*.apk' -ErrorAction SilentlyContinue | Sort-Object Name)
$apks = @($allApks | Where-Object { $expected -contains $_.Name })
if ($apks.Count -eq 0) {
    # 兜底：万一哪天 Flutter 改了产物命名，按"本次构建之后写过"筛，总比把旧包当结果好。
    $apks = @($allApks | Where-Object { $_.LastWriteTime -ge $buildStarted })
}
if ($apks.Count -eq 0) {
    Say "✗ 编译说成功了，但 $outDir 里找不到本次构建的产物（预期 $($expected -join ' / ')）。" 'Red'
    exit 1
}

Say ''
Say '==================== 拿去装机 ====================' 'Green'
foreach ($apk in $apks) {
    Say ("  {0}  ({1:N1} MB)" -f $apk.FullName, ($apk.Length / 1MB)) 'White'
}
$stale = @($allApks | Where-Object { $apks -notcontains $_ })
if ($stale.Count -gt 0) {
    Say ''
    Say ("⚠ 目录里还有 $($stale.Count) 个**不是这次构建**的旧包（别拿错，也建议删掉）：") 'Yellow'
    foreach ($s in $stale) {
        Say ("    {0}  {1:yyyy-MM-dd HH:mm}  {2:N1} MB" -f $s.Name, $s.LastWriteTime, ($s.Length / 1MB)) 'DarkGray'
    }
}

# adb 在哪：优先 PATH，其次 ANDROID_SDK_ROOT / LOCALAPPDATA 下的 platform-tools
function Get-Adb {
    $cmd = Get-Command adb -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $roots = @($env:ANDROID_SDK_ROOT, $env:ANDROID_HOME, (Join-Path $env:LOCALAPPDATA 'Android\Sdk'))
    foreach ($root in $roots) {
        if (-not $root) { continue }
        $candidate = Join-Path $root 'platform-tools\adb.exe'
        if (Test-Path $candidate) { return $candidate }
    }
    return $null
}

if (-not $NoReport) {
    # 体检：把"装不上时该看的那几个事实"一次打全。
    # 用 build-tools 里的 aapt2 / apksigner —— 它们本来就在，不用另外装东西。
    $buildTools = $null
    foreach ($root in @($env:ANDROID_SDK_ROOT, $env:ANDROID_HOME, (Join-Path $env:LOCALAPPDATA 'Android\Sdk'))) {
        if (-not $root) { continue }
        $toolsDir = Join-Path $root 'build-tools'
        if (Test-Path $toolsDir) {
            $buildTools = (Get-ChildItem $toolsDir -Directory | Sort-Object Name -Descending | Select-Object -First 1).FullName
            if ($buildTools) { break }
        }
    }

    Say ''
    Say '-------------------- 装机体检 --------------------' 'Cyan'
    foreach ($apk in $apks) {
        Say ("· " + $apk.Name) 'White'
        if ($buildTools) {
            $aapt = Join-Path $buildTools 'aapt2.exe'
            if (Test-Path $aapt) {
                $badging = & $aapt dump badging $apk.FullName 2>&1
                # ⚠ aapt2 37 打的是 `minSdkVersion:'24'`，不是 `sdkVersion:` —— 只写
                # `^sdkVersion` 时这一行会**静默不出现**（下面第 1 条还拿它当判据）。
                foreach ($pattern in @('^package:', '^minSdkVersion', '^sdkVersion', '^targetSdkVersion', '^native-code')) {
                    $line = $badging | Where-Object { $_ -match $pattern } | Select-Object -First 1
                    if ($line) { Say ("    " + $line.Trim()) 'Gray' }
                }
            }
            $signer = Join-Path $buildTools 'apksigner.bat'
            if (Test-Path $signer) {
                $verify = & $signer verify --print-certs $apk.FullName 2>&1
                $ok = ($LASTEXITCODE -eq 0)
                Say ("    签名校验：" + $(if ($ok) { '通过' } else { '**不通过**（这个包传坏了，重新拷一份）' })) $(if ($ok) { 'Gray' } else { 'Red' })
                $cn = $verify | Where-Object { $_ -match 'signer #1 certificate DN' } | Select-Object -First 1
                if ($cn) { Say ("    " + $cn.Trim()) 'DarkGray' }
            }
        }
        $sha = (Get-FileHash $apk.FullName -Algorithm SHA256).Hash.ToLower()
        Say ("    sha256：" + $sha) 'DarkGray'
    }

    Say ''
    Say '装不上时按这个顺序对：' 'Cyan'
    Say '  1. 手机系统版本 < 上面那个 minSdkVersion → 装不了（minSdk 目前是 24 = Android 7）。' 'Gray'
    Say '  2. 这个脚本只出 **arm64-v8a**；`armeabi-v7a` / `x86_64` 设备装不了（本项目不再出全量包）。' 'Gray'
    Say '  3. 「应用未安装 / 与已有应用签名不一致」→ 手机上先**卸载**旧的那份再装' 'Gray'
    Say '     （release 用的是 debug 签名，换一台机器编出来签名就不同）。' 'Gray'
    Say '  4. 「解析软件包时出现问题」→ APK 在传输途中被改坏了：重新拷（别走会二次打包的渠道），' 'Gray'
    Say '     并核对上面的 sha256。' 'Gray'
    Say '  5. 手机要在「设置」里允许这个来源安装应用（未知来源 / 外部来源应用）。' 'Gray'
    Say '  6. ★ **失败原因一个字都没有**（`failed to install …:` 后面是空的）→ 几乎总是' 'Gray'
    Say '     **adb 链路半路断了**，不是包坏了。先 `adb devices -l` 看状态：offline / unauthorized' 'Gray'
    Say '     就按上面「adb 链路」那一节处理（解锁屏幕点允许 → 撤销授权重插 → 换线换口）。' 'Gray'
    Say '  7. ★ `INSTALL_FAILED_VERSION_DOWNGRADE`：手机上那份的 versionCode 比这份大。' 'Gray'
    Say '     本脚本出的 debug / release 都带统一开关，versionCode 一律是 1；' 'Gray'
    Say '     但如果手机上装过**更早**（2026-09-28 之前）的分架构包，它可能是 1001 / 2001 / 4001 ——' 'Gray'
    Say '     那种情况 `-r` 救不了，先卸载：' 'Gray'
    Say '       adb uninstall com.example.viewer' 'DarkGray'
}

if ($Install) {
    $adb = Get-Adb
    if (-not $adb) {
        Say ''
        Say '（没找到 adb，跳过 -Install。装了 Android platform-tools 再试，或手动拷 APK 到手机。）' 'Yellow'
    } else {
        # ------------------------------------------------------------------
        # 先看链路：`flutter run` 报的那句
        # `Error: ADB exited with exit code 1 / adb.exe: failed to install …:`
        # **后面什么都没有**，绝大多数情况就是**传输在半路断了**，而不是包本身有问题
        # （2026-09-28 真机现场：设备列表里那台是 `offline`）。
        # 所以第一件事永远是报出每个设备的状态、并把状态翻成人话。
        # ------------------------------------------------------------------
        $listed = @(& $adb devices -l | Select-Object -Skip 1 | Where-Object { $_.Trim() })
        Say ''
        Say '-------------------- adb 链路 --------------------' 'Cyan'
        if ($listed.Count -eq 0) {
            Say '  adb 一个设备都没看到。' 'Yellow'
            Say '  · 手机要开「开发者选项 → USB 调试」，插线后手机上会弹一次「允许 USB 调试吗」——要点允许；' 'Gray'
            Say '  · 数据线要能传数据（有些线只供电）；USB 模式选「文件传输 / MTP」。' 'Gray'
        }
        $online = @()
        foreach ($line in $listed) {
            Say ("  " + $line.Trim()) 'Gray'
        }
        $online = @($listed | Where-Object { $_ -match '\sdevice(\s|$)' })
        if ($online.Count -eq 0 -and $listed.Count -gt 0) {
            Say ''
            if ($listed -match '\soffline(\s|$)') {
                Say '  ⚠ 有设备，但状态是 **offline**：adb 握不上手。这就是那句"失败原因空白"的典型来源 ——' 'Yellow'
                Say '    安装包传到一半链路断了，adb 只能回一个空的失败原因（不是包坏了）。按顺序做：' 'Yellow'
                Say '      ① 手机上确认那条「允许 USB 调试吗」还挂着（锁屏会挡住它）→ 点允许；' 'Gray'
                Say '      ② 开发者选项里「撤销 USB 调试授权」→ 拔插一次数据线 → 重新授权；' 'Gray'
                Say '      ③ 还是 offline：换一根线 / 换一个 USB 口（USB3 口和 hub 是常见元凶）；' 'Gray'
                Say '      ④ 然后重跑本脚本，或 `adb kill-server; adb start-server`。' 'Gray'
            }
            if ($listed -match '\sunauthorized(\s|$)') {
                Say '  ⚠ 状态 **unauthorized**：手机还没认这台电脑。解锁屏幕 → 点那条「允许 USB 调试」' 'Yellow'
                Say '    （勾上"一直允许"），或者开发者选项里撤销授权后重新插线。' 'Yellow'
            }
        }

        if ($online.Count -eq 0) {
            Say ''
            Say '（链路没通，跳过 -Install。上面那几条做完再跑一次。）' 'Yellow'
        } else {
            # 存在多个设备时需要选择，不默认安装
            $serial = ($online[0] -split '\s+')[0]
            $installTried = $true
            if ($online.Count -gt 1) {
                Say ''
                Say "  看到 $($online.Count) 台设备，用第一台 $serial（要用别的就加 ANDROID_SERIAL=…）。" 'Yellow'
            }
            # 现在只有一份 arm64-v8a 包：直接装它（多架构包的 ABI 挑选逻辑已随全量包一起删掉）。
            $targets = $apks

            foreach ($apk in $targets) {
                Say ''
                Say ("→ adb -s $serial install -r " + $apk.Name) 'Cyan'
                # ⚠ 判据用输出里的 `Success`，**不能**只看 `$LASTEXITCODE`：
                #   失败之后我们会在下面换一条路重试，而重试是脚本自己发的命令 ——
                #   用退出码判断就会出现"重试成功了、脚本却报失败"（本次实测踩到）。
                $succeeded = $false
                $result = & $adb -s $serial install -r $apk.FullName 2>&1
                $result | ForEach-Object { Say ("    " + $_) 'Gray' }
                if (($result -join "`n") -match 'Success') { $succeeded = $true }

                if ($LASTEXITCODE -ne 0) {
                    # ① 空原因 / 只说 Streamed Install：**换非流式**再试一次。
                    #    `--no-streaming` 走"先 push 到 /data/local/tmp 再 pm install"，
                    #    链路抖动时更稳，而且 pm 会把真正的失败码打出来（流式那条经常什么都不给）。
                    #
                    #    ★ 2026-09-28 真机实测（KB2000 / OnePlus 8T，同一台机器同一条命令）：
                    #    流式那条回的是
                    #      `Performing Streamed Install / * daemon still not running /
                    #       adb: failed to run abb_exec. Error: cannot connect to daemon at tcp:5037`
                    #    —— **设备侧的增量安装服务 abb 掉了**，adb 于是只回一句没有原因的失败，
                    #    看着像"包坏了"，其实是链路。紧接着 push 安装同一份包：Success。
                    $text = ($result -join "`n")
                    if ($text -notmatch 'INSTALL_FAILED') {
                        Say '    没给出失败码（大概率是流式/增量安装半路断了）→ 换 --no-streaming 再试一次…' 'Yellow'
                        $retry = & $adb -s $serial install -r --no-streaming $apk.FullName 2>&1
                        $retry | ForEach-Object { Say ("    " + $_) 'Gray' }
                        if (($retry -join "`n") -match 'Success') { $succeeded = $true }
                        $text = ($retry -join "`n")
                    }
                    # ② 手机里已经有一份**同包名不同签名**的：必须先把旧的卸掉
                    if ($text -match 'INSTALL_FAILED_UPDATE_INCOMPATIBLE|signatures do not match') {
                        Say '    手机上那份签名和这份不一样（release 用的是 debug 签名，换机器编就不同）。' 'Yellow'
                        Say '    → 自动执行 adb uninstall com.example.viewer 再装一次…' 'Yellow'
                        & $adb -s $serial uninstall com.example.viewer 2>&1 | ForEach-Object { Say ("    " + $_) 'Gray' }
                        $again = & $adb -s $serial install -r --no-streaming $apk.FullName 2>&1
                        $again | ForEach-Object { Say ("    " + $_) 'Gray' }
                        if (($again -join "`n") -match 'Success') { $succeeded = $true }
                    }
                    # ③ 国产 ROM 的"USB 安装"开关：这一档的失败原因**常常是空白的**，
                    #    现场（OnePlus / ColorOS 的 KB2000）要对的是手机上的开关，不是包。
                    if ($text -match 'INSTALL_FAILED_USER_RESTRICTED|install_switch_flag|用户限制') {
                        Say '    手机把这个来源的安装**限制住了**（不是包的问题）。去手机上开：' 'Yellow'
                        Say '      · 开发者选项 →「USB 安装」/「通过 USB 安装应用」（ColorOS / MIUI 都有这一项，' 'Gray'
                        Say '        有的版本要求登录厂商账号 + 插 SIM 卡）；' 'Gray'
                        Say '      · 顺手关掉「应用安装管控 / 纯净模式 / 安全守护」里的拦截。' 'Gray'
                    }
                    if ($text -match 'INSTALL_FAILED_INSUFFICIENT_STORAGE') {
                        Say '    手机存储不够了。本脚本出的 arm64 release 包约 30MB，安装时还要再占一份临时空间 ——' 'Yellow'
                        Say '    先清掉手机上的旧包再装。' 'Gray'
                    }
                    # ④ 降级：手机里那份 versionCode 更大。本脚本的包都带统一开关（一律 1），
                    #    只有"2026-09-28 之前装过的分架构包"才会是 1001/2001/4001。
                    #    这条**必须卸载再装**，`-r` 救不了。
                    if ($text -match 'INSTALL_FAILED_VERSION_DOWNGRADE|Downgrade detected') {
                        Say '    手机上那份的 versionCode 比这份大，覆盖不了（不是签名问题）。' 'Yellow'
                        Say '    → 自动执行 adb uninstall com.example.viewer 再装一次…' 'Yellow'
                        & $adb -s $serial uninstall com.example.viewer 2>&1 | ForEach-Object { Say ("    " + $_) 'Gray' }
                        $again = & $adb -s $serial install -r --no-streaming $apk.FullName 2>&1
                        $again | ForEach-Object { Say ("    " + $_) 'Gray' }
                        if (($again -join "`n") -match 'Success') { $succeeded = $true }
                    }
                }
                if ($succeeded) { $installOk = $true }
            }
        }
    }
}

Say ''
if ($Install -and $installTried -and -not $installOk) {
    Say '没装上（原因见上面）。APK 本身是编好了的，路径在上面的「拿去装机」那一节。' 'Red'
    exit 1
}
Say 'OK' 'Green'
