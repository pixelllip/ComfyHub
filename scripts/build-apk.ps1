<#
.SYNOPSIS
    打一份**能装进真手机**的安卓 APK：一条命令走完「选 ABI → 编 → 报出装机要看的那些事实」。

.DESCRIPTION
    为什么值得单独一个脚本（2026-09-28 用户 bug：「真手机无法安装手机端 app」）：

    ① **在这之前，装手机端是没有任何入口的**。仓库里 30 多个脚本没有一个是安卓的，
       用户只能自己敲 `flutter build apk`，而且默认打出来的是 **debug** 包（实测 172MB）——
       传输慢、被网盘/聊天软件二次打包的概率高，也看不出它到底是给哪个 ABI 编的。

    ② **装机失败时看不到任何判据**。APK 装不上无非这几类原因，而它们各自要看的东西
       完全不一样：ABI 与手机对不上（`native-code`）、系统版本低于 `minSdkVersion`、
       手机里已经装着一个**同包名但不同签名**的旧版（`INSTALL_FAILED_UPDATE_INCOMPATIBLE`）、
       或者 APK 在传输途中被改坏了（签名校验不过）。所以本脚本编完之后
       **把这些事实一次性打出来**，而不是只丢一句"编译成功"。

    ③ **arm64 单架构包**（`-Split`）通常只有全量包的 1/3~1/2，装机最快。
       全量包（默认）三个 ABI 都带，随便哪台手机都能装 —— 先试全量，装不上再用 `-Split`。

    签名说明：`android\app\build.gradle.kts` 的 release 用的是 **debug 签名**
    （Flutter 模板的默认做法，`flutter run --release` 靠它才跑得起来）。
    debug 签名**能装**，但和别的机器上编出来的包**签名不同** ——
    手机上如果已经装过一份同包名的，必须先卸干净再装，否则必然
    `INSTALL_FAILED_UPDATE_INCOMPATIBLE`（这一条也打在报告里）。

.EXAMPLE
    pwsh -File scripts\build-apk.ps1                 # 全量 release 包（三个 ABI 都带）
    pwsh -File scripts\build-apk.ps1 -Split          # 只出 arm64-v8a 等分架构包（小得多）
    pwsh -File scripts\build-apk.ps1 -DebugBuild     # debug 包（大，但能连 flutter attach）
    pwsh -File scripts\build-apk.ps1 -Install        # 编完顺手 adb install -r（接了手机才用）
#>
[CmdletBinding()]
param(
    # 出**分架构**包（arm64-v8a / armeabi-v7a / x86_64 各一个）而不是一个全量包
    [switch]$Split,
    # 打 debug 包。⚠ 实测 172MB 起步：只在"要 flutter attach 调试"时才用。
    # ⚠ 名字**不能**叫 `-Debug`：那是 `[CmdletBinding()]` 的公共参数，重名会直接
    #   报 `A parameter with the name 'Debug' was defined multiple times`（本次实测踩到）。
    [switch]$DebugBuild,
    # 编完调 adb install -r（手机上会弹安装确认）。没接设备就跳过，不算失败
    [switch]$Install,
    # 跳过签名/ABI 体检那一趟（只想快点拿到文件时用）
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
if ($Split) {
    $buildArgs += '--split-per-abi'
    # ★ 分架构包**不额外加 ABI 的 versionCode 偏移**（2026-09-28 起）。
    #
    # 不加这个的后果是**永久性**的：分架构包会拿到 1001 / 2001 / 4001，而全量包与
    # `flutter run` 装的 debug 包是 1 —— 于是"装过一次分架构包"的手机，之后
    # **`flutter run` 永远装不上去**（INSTALL_FAILED_VERSION_DOWNGRADE，`-r` 救不了，
    # 只能先卸载）。这正是用户报"真手机无法安装"的第三个原因，实测复现过。
    #
    # 加了它，四种包（debug / 全量 release / 三个分架构）**共用同一个 versionCode**，
    # 互相覆盖都是普通的"同版本替换"，`adb install -r` 与 `flutter run` 都能过。
    # （这个开关是 Flutter 模板自己在 `android\app\build.gradle.kts` 里写明的，见那段注释。）
    $buildArgs += '-Pforce-version-code-ignoring-abi=true'
}

Say ("→ flutter " + ($buildArgs -join ' ')) 'Cyan'
Say '  （第一次编要几分钟：Gradle 要编译 Kotlin 与各插件）' 'DarkGray'
& flutter @buildArgs
if ($LASTEXITCODE -ne 0) {
    Say '' 'Red'
    Say "✗ flutter build apk 失败（退出码 $LASTEXITCODE）。" 'Red'
    Say '  常见两类：① JDK 不在 21~23（AGENTS §4）；② android\ 下的 AGP/Kotlin 配置被动过，' 'Yellow'
    Say '  先读 docs\android-agp9-builtin-kotlin-migration.md。' 'Yellow'
    exit 1
}

$outDir = Join-Path $ProjectRoot 'build\app\outputs\flutter-apk'
$apks = @(Get-ChildItem $outDir -Filter '*.apk' -ErrorAction SilentlyContinue | Sort-Object Name)
if ($apks.Count -eq 0) {
    Say "✗ 编译说成功了，但 $outDir 里一个 apk 都没有。" 'Red'
    exit 1
}

Say ''
Say '==================== 拿去装机 ====================' 'Green'
foreach ($apk in $apks) {
    Say ("  {0}  ({1:N1} MB)" -f $apk.FullName, ($apk.Length / 1MB)) 'White'
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
                foreach ($pattern in @('^package:', '^sdkVersion', '^targetSdkVersion', '^native-code')) {
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
    Say '  1. 手机系统版本 < 上面那个 sdkVersion → 装不了（minSdk 目前是 24 = Android 7）。' 'Gray'
    Say '  2. ABI 对不上 → 用全量包（不加 -Split），它三个 ABI 都带。' 'Gray'
    Say '  3. 「应用未安装 / 与已有应用签名不一致」→ 手机上先**卸载**旧的那份再装' 'Gray'
    Say '     （release 用的是 debug 签名，换一台机器编出来签名就不同）。' 'Gray'
    Say '  4. 「解析软件包时出现问题」→ APK 在传输途中被改坏了：重新拷（别走会二次打包的渠道），' 'Gray'
    Say '     并核对上面的 sha256。' 'Gray'
    Say '  5. 手机要在「设置」里允许这个来源安装应用（未知来源 / 外部来源应用）。' 'Gray'
    Say '  6. ★ **失败原因一个字都没有**（`failed to install …:` 后面是空的）→ 几乎总是' 'Gray'
    Say '     **adb 链路半路断了**，不是包坏了。先 `adb devices -l` 看状态：offline / unauthorized' 'Gray'
    Say '     就按上面「adb 链路」那一节处理（解锁屏幕点允许 → 撤销授权重插 → 换线换口）。' 'Gray'
    Say '  7. ★ `INSTALL_FAILED_VERSION_DOWNGRADE`：手机上那份的 versionCode 比这份大。' 'Gray'
    Say '     实测：分架构包是 1001 / 2001 / 4001，而全量包（含 flutter run 装的 debug 包）是 1 ——' 'Gray'
    Say '     装过一次分架构包之后，`flutter run` 就再也覆盖不回去了。先卸载：' 'Gray'
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
            # 有多个设备时让用户选，别乱装
            $serial = ($online[0] -split '\s+')[0]
            $installTried = $true
            if ($online.Count -gt 1) {
                Say ''
                Say "  看到 $($online.Count) 台设备，用第一台 $serial（要用别的就加 ANDROID_SERIAL=…）。" 'Yellow'
            }
            # 装哪个：分架构包优先挑 **这台手机自己的 ABI**（小得多，传输更不容易断）
            $abi = (& $adb -s $serial shell getprop ro.product.cpu.abi 2>&1 | Select-Object -First 1).Trim()
            if ($abi) { Say ("  手机 ABI：" + $abi) 'Gray' }
            $targets = $apks
            if ($apks.Count -gt 1 -and $abi -match '^(arm64-v8a|armeabi-v7a|x86_64)$') {
                $matched = @($apks | Where-Object { $_.Name -like "*$abi*" })
                if ($matched.Count -gt 0) {
                    Say ("  → 只装匹配 $abi 的那一份（" + $matched[0].Name + "）") 'Gray'
                    $targets = $matched
                }
            }

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
                        Say '    手机存储不够了。debug 包实测 172MB，安装时还要再占一份临时空间 ——' 'Yellow'
                        Say '    用 -Split 出的 arm64 包（~29MB）能省一大截。' 'Gray'
                    }
                    # ④ 降级：手机里那份 versionCode 更大（分架构包是 1001/2001/4001，
                    #    全量包与 flutter run 的 debug 包都是 1）。这条**必须卸载再装**，
                    #    `-r` 救不了 —— 实测现场就是这样（装过一次分架构包之后 flutter run 全红）。
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
