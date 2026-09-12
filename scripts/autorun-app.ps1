<#
.SYNOPSIS
    ComfyHub 一键自动流程：确保 MySQL / 后端在跑 → 构建 Windows 桌面版 → 启动 App。

.DESCRIPTION
    设计成可以被「任务计划程序」在指定时间无人值守调用，
    全过程写入日志文件（默认 .run\autorun.log），失败也会留下清晰的原因。

.EXAMPLE
    pwsh -NoProfile -ExecutionPolicy Bypass -File scripts\autorun-app.ps1
    pwsh -NoProfile -File scripts\autorun-app.ps1 -SkipLaunch
#>
[CmdletBinding()]
param(
    [string]$LogFile,
    [switch]$SkipLaunch,
    [switch]$Release
)

$ErrorActionPreference = 'Continue'
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$RunDir      = Join-Path $ProjectRoot '.run'
if (-not $LogFile) { $LogFile = Join-Path $RunDir 'autorun.log' }
New-Item -ItemType Directory -Force -Path $RunDir | Out-Null

$script:failed = $false

function Log {
    param([string]$Message, [string]$Color = 'Gray')
    $line = '[{0}] {1}' -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $Message
    Write-Host $line -ForegroundColor $Color
    Add-Content -Path $LogFile -Value $line -Encoding utf8
}

function Run-Step {
    param([string]$Title, [scriptblock]$Action)
    Log "---- $Title ----" Cyan
    try {
        & $Action 2>&1 | ForEach-Object { Add-Content -Path $LogFile -Value $_ -Encoding utf8 }
    } catch {
        Log "  !! 步骤失败: $($_.Exception.Message)" Red
        $script:failed = $true
    }
}

# 让日志文件每次都是新的
Set-Content -Path $LogFile -Value "ComfyHub autorun @ $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')" -Encoding utf8

Log '================ ComfyHub 自动流程开始 ================' Green

# ---------------------------------------------------------------------------
# 1 + 2. 数据库 + 后端
#   走 scripts\comfyhub.ps1 这个统一入口，保证「MySQL 先就绪 → 后端才起」，
#   并且后端在跑但数据库不通时会自动重启后端，而不是留下一个假健康状态。
# ---------------------------------------------------------------------------
Run-Step '确保 MySQL + Kotlin 后端在运行' {
    & pwsh -NoProfile -File (Join-Path $PSScriptRoot 'comfyhub.ps1') up
}

$health = $null
try {
    $health = Invoke-RestMethod 'http://127.0.0.1:8080/api/health' -TimeoutSec 5
    if ($health.database -ne 'ok') { throw "数据库状态异常: $($health.database)" }
    Log "后端健康检查: OK (db=$($health.database), storage=$($health.storageDir))" Green
} catch {
    Log "后端健康检查失败: $($_.Exception.Message)" Red
    $script:failed = $true
}

# ---------------------------------------------------------------------------
# 3. Flutter 依赖 / 静态分析
# ---------------------------------------------------------------------------
$env:PUB_HOSTED_URL = 'https://pub.dev'   # 国内镜像对个别包会返回 424，直连 pub.dev 更稳
$env:FLUTTER_SUPPRESS_ANALYTICS = 'true'
$env:PATH = "C:\Users\$env:USERNAME\flutter\bin;$env:PATH"

if (-not (Get-Command flutter -ErrorAction SilentlyContinue)) {
    Log 'PATH 里找不到 flutter，请检查 Flutter SDK 安装位置。' Red
    $script:failed = $true
}

Run-Step 'flutter pub get' {
    Push-Location $ProjectRoot
    try { & flutter pub get } finally { Pop-Location }
}

Run-Step 'flutter analyze' {
    Push-Location $ProjectRoot
    try { & flutter analyze --no-pub } finally { Pop-Location }
}

# ---------------------------------------------------------------------------
# 4. 构建 Windows 桌面版
# ---------------------------------------------------------------------------
$buildArgs = @('build', 'windows')
if ($Release) { $buildArgs += '--release' }

Run-Step ("flutter " + ($buildArgs -join ' ')) {
    Push-Location $ProjectRoot
    try { & flutter @buildArgs } finally { Pop-Location }
}

# ---------------------------------------------------------------------------
# 5. 启动
# ---------------------------------------------------------------------------
$exe = Get-ChildItem (Join-Path $ProjectRoot 'build\windows') -Filter 'viewer.exe' -Recurse -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -like '*\Release\*' } |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $exe) {
    # 没有 Release 产物（比如只跑过 scripts\dev-app.ps1 的 debug 版）就退回最新的那个
    $exe = Get-ChildItem (Join-Path $ProjectRoot 'build\windows') -Filter 'viewer.exe' -Recurse -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
}

if (-not $exe) {
    Log '没有找到构建产物 viewer.exe，跳过启动。' Red
    $script:failed = $true
} elseif ($SkipLaunch) {
    Log "构建产物: $($exe.FullName)（按要求不自动启动）" Green
} else {
    Log "启动 App: $($exe.FullName)" Green
    # 用 WMI 创建，脱离本进程的进程树
    $r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{
        CommandLine      = "`"$($exe.FullName)`""
        CurrentDirectory = $exe.DirectoryName
    }
    if ($r.ReturnValue -ne 0) {
        Log "启动失败 (ReturnValue=$($r.ReturnValue))" Red
        $script:failed = $true
    } else {
        Log "已启动，PID=$($r.ProcessId)" Green
    }
}

if ($script:failed) {
    Log '================ 流程结束（有步骤失败，请看上面的日志）================' Red
    exit 1
}
Log '================ 流程结束，全部成功 ================' Green
exit 0
