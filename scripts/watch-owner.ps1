<#
.SYNOPSIS
    「谁起的谁关」：App 拉起来的本地服务，等 App 退出后自动停掉。

.DESCRIPTION
    `comfyhub.ps1 up -OwnerPid <pid>` 在服务起来之后，会用「脱离进程树 + 隐藏窗口」的方式
    拉起这个脚本（见 scripts\silent-process.ps1）。它每 3 秒确认一次「App 还在不在」：

      · 进程没了（点关闭 / 崩溃 / 任务管理器强杀都一样）→ 按 -StopApi / -StopMysql
        把**这一次真正启动过**的服务停掉；
      · 只停这一次启动过的：库本来就是你在终端里起的，就不会被 App 的退出带走；
      · 没带 -OwnerPid 的手工 `up` 根本不会拉起它，服务照旧常驻。

    防串台：<项目>\.run\watch-owner.json 记着「当前由哪个 watchdog 负责」（认领令牌 token）。
    comfyhub.ps1 会**先**把新 token 写进去、再拉起本脚本；发现自己 token 不对的 watchdog
    直接静默退出，什么都不做 —— 这样「关掉 App 又马上重开」「App 起的服务之后用户又手工 up」
    都不会让旧 watchdog 误停新起来的服务。

.EXAMPLE
    # 一般不用手敲，comfyhub.ps1 up -OwnerPid 会自己拉起来
    pwsh -File scripts\watch-owner.ps1 -OwnerPid 1234 -StopApi -StopMysql
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][int]$OwnerPid,

    # owner 进程的启动时间（Ticks）。给了就能识破 PID 复用；0 表示只比 PID
    [long]$OwnerStartTicks = 0,

    [switch]$StopApi,
    [switch]$StopMysql,
    [string]$DataDir,
    [int]$PollSeconds = 3,

    # 认领令牌：comfyhub.ps1 生成并**先写进状态文件**，再拉起本脚本。
    # 这样在 watchdog 起来之前（pwsh 启动要几百毫秒）发生的"手工 up / 新实例接管"
    # 也能被识别出来 —— 状态文件里的 token 变了，我就不该动手了。
    [string]$Token = ''
)

$ErrorActionPreference = 'Continue'
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$RunDir      = Join-Path $ProjectRoot '.run'
$StateFile   = Join-Path $RunDir 'watch-owner.json'
$LogFile     = Join-Path $RunDir 'watch-owner.log'
$PwshExe     = (Get-Process -Id $PID).Path

New-Item -ItemType Directory -Force -Path $RunDir | Out-Null

function Write-Log([string]$Msg) {
    try {
        Add-Content -Path $LogFile -Value ('{0}  {1}' -f (Get-Date -Format 'MM-dd HH:mm:ss'), $Msg) -Encoding utf8
    } catch { }
}

function Test-OwnerAlive {
    $p = Get-Process -Id $OwnerPid -ErrorAction SilentlyContinue
    if (-not $p) { return $false }
    if ($OwnerStartTicks -gt 0) {
        try { if ($p.StartTime.Ticks -ne $OwnerStartTicks) { return $false } } catch { }
    }
    return $true
}

function Test-StillCurrent {
    <#  状态文件里记的 token 还是不是我的？不是（被删了 / 被新的顶替了）就什么都不做  #>
    if (-not (Test-Path $StateFile)) { return $false }
    try {
        $j = Get-Content $StateFile -Raw -Encoding utf8 | ConvertFrom-Json
        return ([string]$j.token -eq $Token)
    } catch { return $false }
}

if (-not $Token) { $Token = [guid]::NewGuid().ToString('N') }

# 认领：把状态文件补全成"现在由我负责"（token 是 comfyhub 写下的那一份）
try {
    ([pscustomobject]@{
            token       = $Token
            watchdogPid = $PID
            ownerPid    = $OwnerPid
            stopApi     = [bool]$StopApi
            stopMysql   = [bool]$StopMysql
            startedAt   = (Get-Date).ToString('o')
        } | ConvertTo-Json) | Set-Content -Path $StateFile -Encoding utf8
} catch { }

Write-Log "启动 (watchdog=$PID owner=$OwnerPid api=$StopApi mysql=$StopMysql token=$Token)"

# ---------------------------------------------------------------------------
#  等 owner 退出
# ---------------------------------------------------------------------------
while ($true) {
    Start-Sleep -Seconds $PollSeconds
    if (-not (Test-StillCurrent)) { Write-Log '已被顶替，退出'; exit 0 }
    if (-not (Test-OwnerAlive)) { break }
}

# owner 刚走 —— 关掉 App 又立刻重开的话，新实例的 up 会在这 1 秒内把状态文件顶掉
Start-Sleep -Seconds 1
if (-not (Test-StillCurrent)) { Write-Log 'owner 已退出，但已被新实例接管，退出'; exit 0 }

Write-Log 'owner 已退出，开始停服务'
$ErrorActionPreference = 'Continue'

function Get-ApiProcess {
    Get-CimInstance Win32_Process -Filter "Name='java.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -and $_.CommandLine -like '*com.comfyhub.ApplicationKt*' }
}

if ($StopApi) {
    $server = Join-Path $PSScriptRoot 'server.ps1'
    if (Test-Path $server) {
        $a = @('-NoProfile', '-File', $server, 'stop')
        if ($DataDir) { $a += @('-DataDir', $DataDir) }
        & $PwshExe @a 2>&1 | Out-Null
        Write-Log '已下发 server.ps1 stop'
        # 必须等后端真的退出再停库，否则它会一边报错一边重连
        $deadline = (Get-Date).AddSeconds(20)
        while ((Get-Date) -lt $deadline -and @(Get-ApiProcess).Count -gt 0) { Start-Sleep -Milliseconds 400 }
    } else {
        Write-Log "找不到 $server，跳过"
    }
}

if ($StopMysql) {
    $mysql = Join-Path $PSScriptRoot 'mysql.ps1'
    if (Test-Path $mysql) {
        $a = @('-NoProfile', '-File', $mysql, 'stop')
        if ($DataDir) { $a += @('-DataDir', $DataDir) }
        & $PwshExe @a 2>&1 | Out-Null
        Write-Log '已下发 mysql.ps1 stop'
    } else {
        Write-Log "找不到 $mysql，跳过"
    }
}

# 收尾：清掉自己的状态文件（还是我的话），免得下一次没人认领
if (Test-StillCurrent) { Remove-Item $StateFile -Force -ErrorAction SilentlyContinue }
Write-Log '结束'
