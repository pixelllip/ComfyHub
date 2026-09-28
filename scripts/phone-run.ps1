<#
.SYNOPSIS
    安卓真机的**日常调试入口**：装得上（绕开 flutter 那条爱坏的流式安装）+ 热重载照用。

.DESCRIPTION
    用户（2026-09-28）：「我总是需要 flutter run 进行调试，做出靠谱解决方案」。

    `flutter run` 在真机上有**两个它自己修不了的坑**，两个都实测复现过：

    ① **装机那一步在 flutter 里是黑盒**。它固定 `adb install -t -r`（流式 / 走 abb），
       而这台机器上设备侧的增量安装服务会掉：
         `* daemon still not running / failed to run abb_exec. Error: cannot connect to daemon tcp:5037`
       失败原因**一个字都不给**（看着像包坏了）。`adb install --no-streaming`（先 push 到
       `/data/local/tmp` 再 `pm install`）同一份包当场 Success —— 而 **flutter 没有这个开关**。
    ② **versionCode 降级**：分架构包的 versionCode 是 1001/2001/4001，而 `flutter run` 那份是 1，
       装过一次分架构包之后 `flutter run` **永远**报 INSTALL_FAILED_VERSION_DOWNGRADE。
       （已在 `build-apk.ps1 -Split` 里用 `-Pforce-version-code-ignoring-abi=true` 从根上修掉。）

    所以本脚本把"装机"从 flutter 手里拿回来，再用 **`flutter attach`** 接管调试循环 ——
    attach 不做安装，只连正在跑的那个 app 的 VM Service，**热重载 `r` / 热重启 `R`
    与 `flutter run` 逐字相同**。装机走我们这条（带重试与失败码翻译），调试走 flutter 那条。

.EXAMPLE
    pwsh -File scripts\phone-run.ps1
    pwsh -File scripts\phone-run.ps1 -Split        # 用 arm64 小包（28.9MB，传输更不容易断）
    pwsh -File scripts\phone-run.ps1 -ReuseBuild   # 不重编，直接装现有的 app-debug.apk
#>
[CmdletBinding()]
param(
    # 用分架构包（只装匹配本机 ABI 的那一份，比全量 debug 包小一个量级）
    [switch]$Split,
    # 不重编，直接用 build\app\outputs\flutter-apk 里现成的那一份
    [switch]$ReuseBuild,
    # 装完不 attach（只想装上去自己看）
    [switch]$NoAttach
)

$ErrorActionPreference = 'Stop'
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }

$Scripts = $PSScriptRoot
$ProjectRoot = Split-Path -Parent $Scripts
$env:PUB_HOSTED_URL = 'https://pub.dev'

function Say([string]$Text, [string]$Color = 'Gray') { Write-Host $Text -ForegroundColor $Color }

function Get-Adb {
    $cmd = Get-Command adb -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    foreach ($root in @($env:ANDROID_SDK_ROOT, $env:ANDROID_HOME, (Join-Path $env:LOCALAPPDATA 'Android\Sdk'))) {
        if (-not $root) { continue }
        $candidate = Join-Path $root 'platform-tools\adb.exe'
        if (Test-Path $candidate) { return $candidate }
    }
    return $null
}

$adb = Get-Adb
if (-not $adb) { Say '找不到 adb（装 Android platform-tools，或设 ANDROID_SDK_ROOT）。' 'Red'; exit 2 }

# --- 选设备：只要一台 online 的；多台用第一台 ---------------------------------
$online = @(& $adb devices -l | Select-Object -Skip 1 | Where-Object { $_ -match '\sdevice(\s|$)' })
if ($online.Count -eq 0) {
    Say '没有可用的设备（online）。先处理 adb 链路：' 'Red'
    Say '  · 手机解锁 → 确认那条「允许 USB 调试吗」；' 'Yellow'
    Say '  · 开发者选项里「撤销 USB 调试授权」→ 拔插数据线 → 重新授权；' 'Yellow'
    Say '  · 换一根能传数据的线 / 换一个 USB 口（hub 与 USB3 口是常见元凶）。' 'Yellow'
    & $adb devices -l
    exit 2
}
$serial = ($online[0] -split '\s+')[0]
Say "设备：$serial" 'Cyan'
$abi = (& $adb -s $serial shell getprop ro.product.cpu.abi 2>&1 | Select-Object -First 1).Trim()
Say "ABI：$abi" 'Gray'

# --- 编（默认 debug，与 flutter run 一致）------------------------------------
# ⚠ `-Pforce-version-code-ignoring-abi=true` 与 build-apk.ps1 同一条纪律：
#   分架构包不许带 1000×ABI 的偏移，否则它一装上去，`flutter run` 那份（versionCode 1）
#   就再也覆盖不回来了。
$extra = @('-Pforce-version-code-ignoring-abi=true')
if (-not $ReuseBuild) {
    $args = @('build', 'apk', '--debug') + $extra
    if ($Split) { $args += '--split-per-abi' }
    Say ("→ flutter " + ($args -join ' ')) 'Cyan'
    & flutter @args
    if ($LASTEXITCODE -ne 0) { Say '✗ 编译失败' 'Red'; exit 1 }
}

$outDir = Join-Path $ProjectRoot 'build\app\outputs\flutter-apk'
$apk = $null
if ($Split) {
    $apk = Get-ChildItem $outDir -Filter "*$abi*debug*.apk" -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
}
if (-not $apk) {
    $apk = Get-ChildItem $outDir -Filter 'app-debug.apk' -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
}
if (-not $apk) { Say "✗ $outDir 里没有 debug 包（去掉 -ReuseBuild 重编一次）" 'Red'; exit 1 }
Say ("包：{0}  ({1:N1} MB)" -f $apk.Name, ($apk.Length / 1MB)) 'White'

# --- 装：robust 那条路（与 build-apk.ps1 同一套判据）--------------------------
function Install-Apk([string]$path) {
    # 顺序有讲究（2026-09-28 实测）：**先把"包本身的问题"清掉，再谈换传输方式**。
    #   ① 普通装 → 报 DOWNGRADE / 签名不一致：卸载一次，然后**还是用普通装**重来
    #      （卸载之后它就没有对手了，不需要切传输方式）；
    #   ② 只有在"没给出失败码"（= 链路半路断了）时才换 `--no-streaming`。
    #   把 ①② 混在一起做的话，172MB 的包会被白推一遍 —— 而推 172MB 正是最容易断的那一步。
    $r = & $adb -s $serial install -r $path 2>&1
    $r | ForEach-Object { Say ("    " + $_) 'Gray' }
    $text = ($r -join "`n")
    if ($text -match 'Success') { return $true }

    if ($text -match 'INSTALL_FAILED_VERSION_DOWNGRADE|Downgrade detected|INSTALL_FAILED_UPDATE_INCOMPATIBLE') {
        Say '    手机上那份 versionCode 更大 / 签名不同 → 先卸载再装（包内数据会丢，设备库在 storage\ 里）' 'Yellow'
        & $adb -s $serial uninstall com.example.viewer 2>&1 | ForEach-Object { Say ("    " + $_) 'Gray' }
        $r = & $adb -s $serial install -r $path 2>&1
        $r | ForEach-Object { Say ("    " + $_) 'Gray' }
        $text = ($r -join "`n")
        if ($text -match 'Success') { return $true }
    }

    if ($text -notmatch 'INSTALL_FAILED') {
        Say '    没给失败码（流式 / abb 半路断了）→ 换 --no-streaming 重试…' 'Yellow'
        $r2 = & $adb -s $serial install -r --no-streaming $path 2>&1
        $r2 | ForEach-Object { Say ("    " + $_) 'Gray' }
        if (($r2 -join "`n") -match 'Success') { return $true }
        # ⚠ 大包（debug 全量 164MB）走 push 也偶发 `failed to read copy response: EOF`：
        #   那还是链路抖，不是包的问题 —— 重试一次通常就过（实测 2/3 次成功）。
        Say '    push 也断了 → 再试一次（大包可以用 -Split 换成 ~29MB 的 arm64 包）' 'Yellow'
        $r3 = & $adb -s $serial install -r --no-streaming $path 2>&1
        $r3 | ForEach-Object { Say ("    " + $_) 'Gray' }
        if (($r3 -join "`n") -match 'Success') { return $true }
    }
    return $false
}

if (-not (Install-Apk $apk.FullName)) { Say '✗ 没装上（原因见上）' 'Red'; exit 1 }
Say '✓ 已装上' 'Green'

if ($NoAttach) { Say '（-NoAttach：不 attach）' 'Gray'; exit 0 }

# --- 拉起 app + attach --------------------------------------------------------
# 为什么要显式拉起：attach 只能连**正在跑**的 app。
& $adb -s $serial shell monkey -p com.example.viewer -c android.intent.category.LAUNCHER 1 2>&1 | Out-Null
Say '已拉起 app，正在 attach（热重载 r / 热重启 R，q 退出）…' 'Cyan'
& flutter attach -d $serial
