<#
.SYNOPSIS
    重编 Rust 后端二进制并重启它：一条命令走完「停后端 → cargo build → 起后端 → 等健康」。

.DESCRIPTION
    为什么值得单独一个脚本 —— 2026-09-27 的现场是两个坑叠在一起：

    ① **没有任何一条路会在"源码变新"时重编**。
       `server.ps1 start` / `comfyhub.ps1 up` 只在 exe **不存在**时才 cargo build
       （`if (-not (Test-Path $exe) -and -not $SkipBuild)`），
       而 `dev-app.ps1` 和 App 的自动启动一律带 `-SkipBuild`。
       于是"改了 server-rs\、exe 还在"时它们**直接复用旧 exe**：
       那天 20:36~22:36 有 8 个提交动了后端（schema 对齐 / 手机工具目录 / 能力探针 /
       排队提交 / 扫码配对 / 密钥上行 / 网卡地址 / 生成进度），
       跑了一整晚的却还是 19:11 编的那一份 —— 症状看着像功能 bug，其实是旧构建。
       （`doctor` 与 `up` 都会报"后端二进制是旧构建"，但没人替你跨出那一步。）

    ② **后端在跑时 `cargo build` 必然失败**：Windows 不许覆盖一个正在运行的 exe
       （实测确认文件被占：`The process cannot access the file ... because it is being
       used by another process`，cargo 侧就是"拒绝访问 / os error 5"）。
       所以重编的正确顺序只能是"先停后编"，而没人替你记住这一步。

    本脚本把顺序钉死，并且**在停服务之前**先探一次"脱离进程树启进程"这条路通不通
    （静默启动走 WMI / wscript，见 silent-process.ps1）。
    受限环境（沙箱 / 受限令牌）里那一步会被拒"拒绝访问" —— 那正是那天最坏的形态：
    **停成功了、起不来了**，健康检查红着、服务空着、还得人工去捞。
    所以探不通就**当场退出、后端一个字节都不动**。

    端口默认**沿用当前正在跑的那一个**（`COMFYHUB_PORT`，优先级见 api-port.ps1），
    这样 App 不会被换门牌号、不用重新发现端口。

.EXAMPLE
    pwsh -File scripts\rebuild-server.ps1            # 停 → 编 → 起
    pwsh -File scripts\rebuild-server.ps1 -NoStart   # 只重编，先不起
    pwsh -File scripts\rebuild-server.ps1 -Force     # 源码没变也重编一遍
#>
[CmdletBinding()]
param(
    # 只重编，不负责把后端再拉起来（你自己 `comfyhub.ps1 up`）
    [switch]$NoStart,
    # 源码没变也照样重编一遍（平时"没变就跳过"，省几分钟）
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
# 脚本之间互相调用，输出统一成 UTF-8（与其它 scripts\*.ps1 同一套，见 AGENTS §5）
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }

$Scripts = $PSScriptRoot
$ProjectRoot = Split-Path -Parent $Scripts
$CargoToml = Join-Path $ProjectRoot 'server-rs\Cargo.toml'

function Say([string]$Text, [string]$Color = 'Gray') { Write-Host $Text -ForegroundColor $Color }

if (-not (Test-Path $CargoToml)) {
    Say "找不到 $CargoToml —— 这个脚本只对源码树有用（发布包里后端是装配好的二进制）。" 'Red'
    exit 2
}

# 后端是哪一份 / 二进制在哪 / 新不新：全部走 backend.ps1 那一份真源（AGENTS §7）
. (Join-Path $Scripts 'backend.ps1')
. (Join-Path $Scripts 'api-port.ps1')

$backend = Resolve-Backend -ProjectRoot $ProjectRoot
if ($backend.Kind -ne 'rust') {
    Say "当前解析出来的后端实现是 $($backend.Kind)（不是 rust）—— 本脚本只重编 Rust。" 'Red'
    Say '  要重编的其实是别的后端，请先看清楚 scripts\backend.ps1 的 Resolve-Backend 结果。' 'DarkGray'
    exit 2
}

# ---------------------------------------------------------------------------
#  0) 停之前先探一次「能不能脱离进程树起进程」
# ---------------------------------------------------------------------------
#  这一条是整个脚本里最重要的：探不通就**什么都别动**。
#  否则就是"停成功、起不来"—— 那天的原样。
$SilentHelper = Join-Path $Scripts 'silent-process.ps1'
if (-not (Test-Path $SilentHelper)) { Say "找不到 $SilentHelper（静默启动的公共实现）。" 'Red'; exit 2 }
. $SilentHelper

Say '[0/4] 预检：能不能脱离进程树启进程（起不来就不动现有服务）…' 'Cyan'
$probeOk = $false
$probeHow = ''
try {
    # 探针本身就是真路径：跑一个立刻退出的 cmd（隐藏窗口，不留渣）
    $probeHow = Start-SilentProcess -CommandLine 'cmd.exe /c exit' -Tag 'rebuild-probe'
    $probeOk = $true
} catch {
    $probeErr = $_.Exception.Message
}
if (-not $probeOk) {
    Say '  预检失败：这台环境不允许我们创建"脱离进程树"的进程。' 'Red'
    Say "  底层报错：$probeErr" 'Red'
    Say '' 'Gray'
    Say '  → 已中止，后端保持原样（一个字节都没动）。' 'Yellow'
    Say '    常见原因：命令跑在受限环境 / 沙箱里（WMI 的 Win32_Process.Create 被拒"拒绝访问"）。' 'DarkGray'
    Say '    处理：在一个普通（非沙箱）的 pwsh 窗口里重跑本脚本，或直接让 App 点「重启」。' 'DarkGray'
    exit 3
}
Say "  可以（方式：$probeHow）" 'Green'

# ---------------------------------------------------------------------------
#  1) 源码新不新（没变就别白编几分钟）
# ---------------------------------------------------------------------------
$exe = Resolve-RustExe -ProjectRoot $ProjectRoot
$fresh = Get-RustBuildFreshness -Exe $exe -ProjectRoot $ProjectRoot
if ($exe -and $fresh -and -not $fresh.Stale -and -not $Force) {
    Say '  源码没有比二进制更新，跳过重编（要强制重编加 -Force）。' 'Green'
    if (-not $NoStart) {
        Say '后台已在跑就直接用；没在跑就拉起来…' 'DarkGray'
        & pwsh -NoProfile -File (Join-Path $Scripts 'comfyhub.ps1') up -SkipBuild
    }
    exit 0
}
if ($fresh -and $fresh.Stale -and $fresh.NewestSource) {
    Say ("  源码 {0}（{1}）比二进制（{2}）新" -f `
            $fresh.NewestSource.Name, `
            $fresh.NewestSource.LastWriteTime.ToString('MM-dd HH:mm'), `
            $fresh.ExeTime.ToString('MM-dd HH:mm')) 'Yellow'
}
elseif ($Force) {
    Say '  -Force：源码没变也重编一遍。' 'DarkGray'
}

# ---------------------------------------------------------------------------
#  2) 停后端（**只停后端**，不碰 App —— App 走 comfyhub.ps1 release/down）
# ---------------------------------------------------------------------------
#  端口：先记下现在这个，重启时沿用，免得好好的 App 被换门牌号（App 不会周期性重读
#  .run\api.json，换端口就得人工点一次「修复」）。
$keepPort = 0
$state = Read-ApiPortState -ProjectRoot $ProjectRoot
if ($state -and $state.Port -gt 0 -and (Test-ApiPortListening -Port $state.Port)) {
    $keepPort = $state.Port
    Say "  当前后端在 $keepPort，重启时沿用这个端口" 'DarkGray'
}

Say '[1/4] 停后端（它占着 exe，不停就编不动）…' 'Cyan'
& pwsh -NoProfile -File (Join-Path $Scripts 'server.ps1') stop | Out-Null

# 停完还要等它真的把文件句柄放掉：cargo 紧接着就开写，抢跑就是"拒绝访问"
$deadline = (Get-Date).AddSeconds(20)
while ((Get-Date) -lt $deadline) {
    $stillRunning = @(Get-RustServerProcess).Count -gt 0
    if (-not $stillRunning -and -not (Test-ApiPortListening -Port $keepPort)) { break }
    Start-Sleep -Milliseconds 300
}
if (@(Get-RustServerProcess).Count -gt 0) {
    Say '  后端进程 20 秒内没退干净，先别编（会被文件占用挡住）。' 'Red'
    Say '  处理：pwsh -File scripts\comfyhub.ps1 down 之后再重跑本脚本。' 'DarkGray'
    exit 4
}

# ---------------------------------------------------------------------------
#  3) cargo build --release
# ---------------------------------------------------------------------------
Say '[2/4] cargo build --release …' 'Cyan'
$sw = [System.Diagnostics.Stopwatch]::StartNew()
& cargo build --release --manifest-path $CargoToml
$code = $LASTEXITCODE
$sw.Stop()
if ($code -ne 0) {
    Say "  cargo build 失败（退出码 $code）。" 'Red'
    Say '  后端现在是**停着**的；修完源码再跑一次本脚本，或先 pwsh -File scripts\comfyhub.ps1 up 起旧二进制。' 'Yellow'
    exit $code
}
$exe = Resolve-RustExe -ProjectRoot $ProjectRoot
Say ("  编译完成（{0:N1}s）→ {1}" -f $sw.Elapsed.TotalSeconds, $exe) 'Green'
Say ("  二进制时间：{0}" -f (Get-Item $exe).LastWriteTime.ToString('yyyy-MM-dd HH:mm:ss')) 'DarkGray'

if ($NoStart) {
    Say '' 'Gray'
    Say '[-NoStart] 只重编，没有起后端。要起：pwsh -File scripts\comfyhub.ps1 up -SkipBuild' 'Yellow'
    exit 0
}

# ---------------------------------------------------------------------------
#  4) 起后端 + 等健康（走统一入口，端口沿用上面记下的那个）
# ---------------------------------------------------------------------------
Say '[3/4] 起后端（沿用原端口，App 不用重连）…' 'Cyan'
# 只在"刚才真有人在听"时才钉端口；否则**不动** COMFYHUB_PORT（用户可能自己设了一个，
# 清掉它等于偷偷改了他的配置）。端口钉法见 api-port.ps1 的优先级说明。
if ($keepPort -gt 0) { $env:COMFYHUB_PORT = "$keepPort" }
# `up` 会自己等健康检查并打状态表；失败它会给非零退出码
& pwsh -NoProfile -File (Join-Path $Scripts 'comfyhub.ps1') up -SkipBuild
if ($LASTEXITCODE -ne 0) {
    Say '' 'Gray'
    Say "  up 没成功（退出码 $LASTEXITCODE）。看 .run\server.err.log，或 pwsh -File scripts\comfyhub.ps1 doctor。" 'Red'
    exit 5
}

$after = Read-ApiPortState -ProjectRoot $ProjectRoot
Say '[4/4] 完成。' 'Green'
if ($keepPort -gt 0 -and $after -and $after.Port -ne $keepPort) {
    Say "  ⚠ 端口从 $keepPort 变成了 $($after.Port)：正在跑的 App 还指着旧门牌，请点一次「修复」。" 'Yellow'
}
if ($after) { Say "  后端：$($after.Url)（pid $($after.Pid)）" 'DarkGray' }
exit 0
