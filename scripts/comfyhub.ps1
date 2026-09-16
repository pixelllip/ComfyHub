<#
.SYNOPSIS
    ComfyHub 整套服务的统一生命周期入口：MySQL + Kotlin 后端（+ 可选桌面 App）。

.DESCRIPTION
    之前 mysql.ps1 和 server.ps1 各管一半，会出现这些坏状态：
      · 后端在跑但 MySQL 挂了 —— 所有接口 500，而 `server.ps1 start` 看到后端已在运行就直接返回，不修
      · `mysql.ps1 stop` 之后后端还活着，静静地对着一个死库
      · 没有任何一条命令能一次看清「三个东西分别是什么状态」
    这个脚本负责把它们**当成一个整体**来起停和体检，保证启动顺序（MySQL 先就绪 → 后端才起）
    和「后端健康 = 数据库也健康」。

    MySQL 实例目录可以在项目外任意盘/目录。带了 -DataDir 就原样透传给 mysql.ps1 / server.ps1；
    不带时由它们自己按 COMFYHUB_MYSQL_DIR → <项目>\.mysql-location.json → <项目>\.mysql 解析。

.EXAMPLE
    pwsh -File scripts\comfyhub.ps1 up            # 起 MySQL + 后端
    pwsh -File scripts\comfyhub.ps1 up -WithApp   # 再顺带把桌面 App 拉起来
    pwsh -File scripts\comfyhub.ps1 status        # 三者状态一览
    pwsh -File scripts\comfyhub.ps1 down          # 全停（App → 后端 → MySQL）
    pwsh -File scripts\comfyhub.ps1 release       # 只停后端 + MySQL，不动 App（App 退出时调它）
    pwsh -File scripts\comfyhub.ps1 watch -OwnerPid <pid>   # 给"已经在跑服务"的 App 补挂退出守护
    pwsh -File scripts\comfyhub.ps1 restart
    pwsh -File scripts\comfyhub.ps1 logs
    pwsh -File scripts\comfyhub.ps1 up -DataDir 'D:\mysql\comfyhub'   # 指定 MySQL 存储位置
#>
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateSet('up', 'down', 'status', 'restart', 'logs', 'doctor', 'release', 'watch', 'unwatch')]
    [string]$Action = 'status',

    [switch]$WithApp,
    [switch]$SkipBuild,

    # MySQL 实例目录；留空则走默认解析链
    [string]$DataDir,

    # 调用方（桌面 App）的 PID。传了它 = “这些服务是 App 拉起来的”：
    # 服务起来后会挂一个守护进程盯着这个 PID，App 一退出就把**这次启动过的**服务停掉。
    # 手工敲 `up` 不带它时，服务照旧常驻（以前的行为）。见 scripts\watch-owner.ps1
    # `watch` 动作也用它：服务本来就跑着（App 探到健康就直接返回、没跑 up）时补挂守护进程。
    [int]$OwnerPid = 0
)

$ErrorActionPreference = 'Continue'
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$Scripts     = $PSScriptRoot
$RunDir      = Join-Path $ProjectRoot '.run'
$MysqlPort   = 3307
$ApiPort     = 8080

# 「关 App 自动停服务」的状态文件：里面记着当前负责的 watchdog PID
$WatchState = Join-Path $RunDir 'watch-owner.json'

# 这次 up 到底启动了哪几个服务（只停自己起的，见 watch-owner.ps1）
$Script:StartedMysql = $false
$Script:StartedApi   = $false

New-Item -ItemType Directory -Force -Path $RunDir | Out-Null

# 静默启动（无窗口 + 脱离进程树）的公共实现，见 scripts\silent-process.ps1
$SilentHelper = Join-Path $Scripts 'silent-process.ps1'
if (Test-Path $SilentHelper) { . $SilentHelper }

# 运行时依赖体检（缺 pwsh / VC++ 运行时时给出安装提示），见 scripts\runtime-deps.ps1
$RuntimeHelper = Join-Path $Scripts 'runtime-deps.ps1'
if (Test-Path $RuntimeHelper) { . $RuntimeHelper }

# ComfyUI 位置解析（用户"其他建议"第 3 条）：发布包是便携式的，ComfyUI 装在哪不能靠猜，
# 这里与后端 ComfyLocator.kt 用同一套判据，见 scripts\comfy-path.ps1
$ComfyPathHelper = Join-Path $Scripts 'comfy-path.ps1'
if (Test-Path $ComfyPathHelper) { . $ComfyPathHelper }

# ---------------------------------------------------------------------------
#  输出编码
# ---------------------------------------------------------------------------
# App（lib\core\backend_launcher.dart）是把脚本的 stdout 按 **UTF-8** 解的，
# 而 pwsh 在 stdout 被重定向时跟随控制台代码页（中文系统是 936/GBK），
# 于是 App 日志会刷 "FormatException: Missing extension byte"（第一个中文字符的位置）。
# 只在"被重定向"时改，不去动用户交互式终端的代码页。
if ([Console]::IsOutputRedirected) {
    try { [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false) } catch { }
}

# ---------------------------------------------------------------------------
#  耗时打点
# ---------------------------------------------------------------------------
# 冷启动慢的时候用它定位：设 COMFYHUB_TRACE=1，每一步的相对耗时写到 stderr
# （ASCII，不受代码页影响；App 也会把 stderr 显示在启动页日志里）。
$Script:TraceWatch = [System.Diagnostics.Stopwatch]::StartNew()
function Trace([string]$Step) {
    if (-not $env:COMFYHUB_TRACE) { return }
    # 先拼好字符串再 WriteLine：直接传 ('fmt' -f a,b) 会命中
    # WriteLine(string format, object arg0) 这个重载，把占位符当参数报错。
    $line = '[trace {0,6}ms] {1}' -f $Script:TraceWatch.ElapsedMilliseconds, $Step
    [Console]::Error.WriteLine($line)
}

function Say([string]$msg, [string]$color = 'Gray') { Write-Host $msg -ForegroundColor $color }

function Test-TcpPort([string]$TargetHost, [int]$TargetPort, [int]$TimeoutMs = 200) {
    <#
      端口上"有没有人在听"的快速判断，所有"服务是否还活着"的探测都先过这一道。

      为什么不能直接去连：连一个没在监听的端口本机要等 SYN 重传，这台机器上实测
      **恒定 ~2 秒**（Invoke-RestMethod / mysqladmin / 裸 TcpClient 都一样；开着
      Clash/mihomo 这类 TUN 代理时 RST 会被吃掉，只会更明显）。冷启动时"服务还没起来"
      恰恰是最常见的状态，每次探测白等 2 秒，几次就把启动拖长了。
      BeginConnect + 200ms 超时则最多 200ms，端口开着时和正常连接一样快（实测 ~12ms）。
    #>
    $client = [System.Net.Sockets.TcpClient]::new()
    try {
        $iar = $client.BeginConnect($TargetHost, $TargetPort, $null, $null)
        if (-not $iar.AsyncWaitHandle.WaitOne($TimeoutMs)) { return $false }
        $client.EndConnect($iar)
        return $true
    } catch { return $false }
    finally { $client.Dispose() }
}

function Resolve-MysqlDataDir {
    <#
      与 mysql.ps1 保持同一套解析顺序（这里刻意重复实现，不引共享模块）：
        -DataDir → COMFYHUB_MYSQL_DIR → <项目>\.mysql-location.json → <项目>\.mysql
      相对路径按项目根目录展开，返回规范化绝对路径。
    #>
    $cand = @($DataDir, $env:COMFYHUB_MYSQL_DIR) | Where-Object { $_ } | Select-Object -First 1
    if (-not $cand) {
        $pointer = Join-Path $ProjectRoot '.mysql-location.json'
        if (Test-Path $pointer) {
            try {
                $json = Get-Content -Path $pointer -Raw -Encoding utf8 | ConvertFrom-Json
                if ($json -and $json.dataDir) { $cand = [string]$json.dataDir }
            } catch {
                Say "  警告: $pointer 解析失败，已忽略（$($_.Exception.Message)）" 'Yellow'
            }
        }
    }
    if (-not $cand) { return (Join-Path $ProjectRoot '.mysql') }
    if (-not [System.IO.Path]::IsPathRooted($cand)) { $cand = Join-Path $ProjectRoot $cand }
    return [System.IO.Path]::GetFullPath($cand).TrimEnd('\')
}

$MysqlInstanceDir = Resolve-MysqlDataDir
$MysqlDataPath    = Join-Path $MysqlInstanceDir 'data'
$MysqlErrorLog    = Join-Path $MysqlInstanceDir 'mysql-error.log'

# 透传给 mysql.ps1 / server.ps1 的 -DataDir（留空就不传，让它们自己解析）
$DataDirArgs = @()
if ($DataDir) { $DataDirArgs = @('-DataDir', $DataDir) }

# 中文是双宽字符，PowerShell 的 -f 对齐按"字符数"算会错位，这里按显示宽度手工补空格
function Get-DisplayWidth([string]$s) {
    if (-not $s) { return 0 }
    $w = 0
    foreach ($ch in $s.ToCharArray()) {
        $code = [int]$ch
        if (($code -ge 0x1100 -and $code -le 0x115F) -or
            ($code -ge 0x2E80 -and $code -le 0xA4CF) -or
            ($code -ge 0xAC00 -and $code -le 0xD7A3) -or
            ($code -ge 0xF900 -and $code -le 0xFAFF) -or
            ($code -ge 0xFE30 -and $code -le 0xFE6F) -or
            ($code -ge 0xFF00 -and $code -le 0xFF60) -or
            ($code -ge 0xFFE0 -and $code -le 0xFFE6)) { $w += 2 } else { $w += 1 }
    }
    return $w
}

function Format-Row([string]$name, [string]$state, [string]$detail) {
    $pad1 = ' ' * [Math]::Max(1, 12 - (Get-DisplayWidth $name))
    $pad2 = ' ' * [Math]::Max(1, 12 - (Get-DisplayWidth $state))
    return "  $name$pad1$state$pad2$detail"
}

# ---------------------------------------------------------------------------
#  探测
# ---------------------------------------------------------------------------

function Resolve-MysqlBin([string]$exe) {
    <#
      注意：不要写成 Get-ChildItem 'D:\tools\mysql\*\bin' -Filter xxx，
      路径里带通配符时 -Filter 不会被文件系统提供程序正确应用，会静默返回空。
    #>
    $mysqlHome = $env:COMFYHUB_MYSQL_HOME
    $cands = @()
    if ($mysqlHome) { $cands += (Join-Path $mysqlHome "bin\$exe") }

    # 发布包自带的便携版（<根>\mysql）要排在 D:\tools 前面：
    # 装配好的发布包里有自己那份 MySQL，去找开发机路径的话换台机器就找不到，
    # Test-MySqlAlive 会永远为假 → 状态页明明库在跑却报"未运行"。
    $cands += (Join-Path $ProjectRoot "mysql\bin\$exe")

    foreach ($root in @('D:\tools\mysql', 'C:\tools\mysql')) {
        if (-not (Test-Path $root)) { continue }
        $cands += (Join-Path $root "bin\$exe")
        Get-ChildItem -Path $root -Directory -ErrorAction SilentlyContinue | ForEach-Object {
            $cands += (Join-Path $_.FullName "bin\$exe")
        }
    }
    return ($cands | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1)
}

function Get-MysqldProcess {
    Get-CimInstance Win32_Process -Filter "Name='mysqld.exe'" -ErrorAction SilentlyContinue
}

function Test-MySqlAlive {
    # 先免费判断端口（死端口 ~300ms），再花 60ms 做权威的 mysqladmin ping，
    # 也省掉"库根本没起"时那次没必要的目录枚举。
    if (-not (Test-TcpPort '127.0.0.1' $MysqlPort)) { return $false }
    if (-not $Script:MysqlAdminExe) { $Script:MysqlAdminExe = Resolve-MysqlBin 'mysqladmin.exe' }
    if (-not $Script:MysqlAdminExe) { return $false }
    & $Script:MysqlAdminExe --protocol=TCP -h 127.0.0.1 -P $MysqlPort -u comfyhub --password=comfyhub ping 2>&1 | Out-Null
    return ($LASTEXITCODE -eq 0)
}

function Get-ApiProcess {
    Get-CimInstance Win32_Process -Filter "Name='java.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -and $_.CommandLine -like '*com.comfyhub.ApplicationKt*' }
}

function Get-ApiHealth {
    # 同上：端口没开就直接返回，不要让 Invoke-RestMethod 去等那 2 秒
    if (-not (Test-TcpPort '127.0.0.1' $ApiPort)) { return $null }
    try { return Invoke-RestMethod "http://127.0.0.1:$ApiPort/api/health" -TimeoutSec 3 }
    catch { return $null }
}

function Get-AppProcess { Get-Process -Name viewer -ErrorAction SilentlyContinue }

# ---------------------------------------------------------------------------
#  状态
# ---------------------------------------------------------------------------

function Get-StackStatus {
    $mysqlProc = Get-MysqldProcess
    $mysqlAlive = Test-MySqlAlive
    $apiProc = Get-ApiProcess
    $health = Get-ApiHealth
    $app = Get-AppProcess

    return [pscustomobject]@{
        MysqlProcess = @($mysqlProc).Count
        MysqlPids    = @($mysqlProc | ForEach-Object { $_.ProcessId })
        MysqlAlive   = $mysqlAlive
        ApiProcess   = @($apiProc).Count
        ApiHealth    = $health
        AppProcess   = @($app).Count
    }
}

function Show-Status {
    Trace '状态汇总'
    $s = Get-StackStatus

    Say ''
    Say '  ComfyHub 服务状态' 'White'
    Say '  ────────────────────────────────────────────────────────────'

    # MySQL
    # 注意：MySQL 8 在 Windows 上会有一个父进程 + 一个真正干活并占端口的子进程，
    # 所以这里把 PID 列表整个打出来，不要只打进程个数（以前会显示成 "PID 2"，很误导）。
    if ($s.MysqlAlive) {
        Say (Format-Row 'MySQL' '运行中' "127.0.0.1:$MysqlPort  (PID $(($s.MysqlPids) -join ','))") 'Green'
    } elseif ($s.MysqlProcess -gt 0) {
        Say (Format-Row 'MySQL' '启动中' '进程在但还连不上，稍等或看日志') 'Yellow'
    } else {
        Say (Format-Row 'MySQL' '未运行' '执行: pwsh -File scripts\comfyhub.ps1 up') 'Red'
    }

    # 后端（报的是端到端健康，包含数据库连通性）
    if ($s.ApiHealth -and $s.ApiHealth.database -eq 'ok') {
        Say (Format-Row '后端' '健康' "http://127.0.0.1:$ApiPort  v$($s.ApiHealth.version)  db=ok") 'Green'
        Say (Format-Row '' '' "存储目录 $($s.ApiHealth.storageDir)") 'DarkGray'
    } elseif ($s.ApiProcess -gt 0) {
        $db = if ($s.ApiHealth) { $s.ApiHealth.database } else { '无响应' }
        Say (Format-Row '后端' '不健康' "进程在跑但 db=$db —— 多半是 MySQL 掉了") 'Red'
        Say (Format-Row '' '' '修复: pwsh -File scripts\comfyhub.ps1 restart') 'Yellow'
    } else {
        Say (Format-Row '后端' '未运行' '修复: pwsh -File scripts\comfyhub.ps1 up') 'Red'
    }

    # 桌面 App
    if ($s.AppProcess -gt 0) {
        Say (Format-Row '桌面 App' '运行中' "$($s.AppProcess) 个进程") 'Green'
    } else {
        Say (Format-Row '桌面 App' '未运行' '启动: pwsh -File scripts\comfyhub.ps1 up -WithApp') 'DarkGray'
    }

    Say '  ────────────────────────────────────────────────────────────'
    Say ''
    return $s
}

# ---------------------------------------------------------------------------
#  启动
# ---------------------------------------------------------------------------

function Start-MySqlPart {
    Trace 'MySQL: 存活检测'
    if (Test-MySqlAlive) { Say '  MySQL 已经在跑了。' 'DarkGray'; Trace 'MySQL: 本来就在跑'; return $true }

    Say '  [1/3] 启动 MySQL…' 'Cyan'
    & pwsh -NoProfile -File (Join-Path $Scripts 'mysql.ps1') start @DataDirArgs
    Trace 'MySQL: mysql.ps1 start 已返回'

    # mysql.ps1 内部已经等过一轮，这里再兜一次底，确保"就绪"再往下走
    $deadline = (Get-Date).AddSeconds(60)
    while ((Get-Date) -lt $deadline) {
        if (Test-MySqlAlive) {
            $Script:StartedMysql = $true
            Trace 'MySQL: 就绪'
            return $true
        }
        Start-Sleep -Milliseconds 200
    }
    Say "  MySQL 没能就绪，请查看 $MysqlErrorLog" 'Red'
    return $false
}

function Start-ApiPart {
    Trace '后端: 探测健康'
    $health = Get-ApiHealth
    $apiRunning = @(Get-ApiProcess).Count -gt 0

    # 关键：后端在跑但数据库连不上时，必须重启它，而不是当作"已启动"直接跳过
    if ($apiRunning -and $health -and $health.database -eq 'ok') {
        Say '  后端已经在跑且健康。' 'DarkGray'
        Trace '后端: 本来就在跑'
        return $true
    }
    if ($apiRunning) {
        Say '  后端在跑但数据库不通，先把它停掉再重启…' 'Yellow'
        & pwsh -NoProfile -File (Join-Path $Scripts 'server.ps1') stop @DataDirArgs
        Start-Sleep 1
    }

    Say '  [2/3] 启动 Kotlin 后端…' 'Cyan'
    if ($SkipBuild) {
        & pwsh -NoProfile -File (Join-Path $Scripts 'server.ps1') start -SkipBuild @DataDirArgs
    } else {
        & pwsh -NoProfile -File (Join-Path $Scripts 'server.ps1') start @DataDirArgs
    }
    Trace '后端: server.ps1 start 已返回'

    $health = Get-ApiHealth
    if ($health -and $health.database -eq 'ok') {
        $Script:StartedApi = $true
        Trace '后端: 健康'
        return $true
    }
    Say '  后端健康检查未通过。' 'Red'
    return $false
}

function Resolve-AppExe {
    <#
      三种布局，按优先级：
        1. 发布包：<根>\viewer.exe —— scripts\pack-release.ps1 装配出来的，exe 就在根目录
        2. Release 构建产物：build\windows\...\runner\Release\viewer.exe
        3. 兜底：build\windows 下最新的 viewer.exe（debug 版，只用于热重载调试）
    #>
    $packaged = Join-Path $ProjectRoot 'viewer.exe'
    if (Test-Path -LiteralPath $packaged) { return (Get-Item -LiteralPath $packaged) }

    $exe = Get-ChildItem (Join-Path $ProjectRoot 'build\windows') -Filter 'viewer.exe' -Recurse -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -like '*\Release\*' } |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $exe) {
        $exe = Get-ChildItem (Join-Path $ProjectRoot 'build\windows') -Filter 'viewer.exe' -Recurse -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending | Select-Object -First 1
    }
    return $exe
}

function Start-AppPart {
    $exe = Resolve-AppExe
    if (-not $exe) {
        Say '  找不到 viewer.exe，先执行: pwsh -File scripts\autorun-app.ps1（或打包: pwsh -File scripts\pack-release.ps1）' 'Yellow'
        return $false
    }
    if (Get-AppProcess) { Say '  App 已经在跑了。' 'DarkGray'; return $true }

    Say '  [3/3] 启动桌面 App…' 'Cyan'
    $r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{
        CommandLine      = "`"$($exe.FullName)`""
        CurrentDirectory = $exe.DirectoryName
    }
    if ($r.ReturnValue -ne 0) { Say "  启动失败 (ReturnValue=$($r.ReturnValue))" 'Red'; return $false }
    Say "  已启动，PID=$($r.ProcessId)" 'Green'
    return $true
}

# ---------------------------------------------------------------------------
#  「关 App 自动停服务」的守护进程
# ---------------------------------------------------------------------------

function Clear-OwnerWatchdog {
    <#
      让现有 watchdog 失效：它每轮都会看 <项目>\.run\watch-owner.json 里记的是不是自己，
      文件没了 / 换了人，它就静默退出，什么也不做。
      —— 手工 up/down、或新的 App 实例接管时都要先清一下，
         否则旧 watchdog 会把刚起来的服务又停掉。
    #>
    if (Test-Path $WatchState) { Remove-Item $WatchState -Force -ErrorAction SilentlyContinue }
}

function Start-OwnerWatchdog {
    <#
      挂一个「脱离进程树 + 隐藏窗口」的 watchdog 盯着 App 的 PID（见 scripts\watch-owner.ps1）。
      只是锦上添花：拉不起来也不影响启动，只是关 App 时服务继续留着。

      -StopApi / -StopMysql 显式指定要停哪几个服务；不传就沿用 up 的判定
      （这次真正启动过的那几个，即 $Script:StartedApi / $Script:StartedMysql），
      所以 up 的行为和以前完全一样。`watch` 动作会用显式参数覆盖它。
    #>
    param(
        [int]$Owner = 0,
        [bool]$StopApi = $Script:StartedApi,
        [bool]$StopMysql = $Script:StartedMysql
    )
    $watcher = Join-Path $Scripts 'watch-owner.ps1'
    if (-not (Test-Path $watcher)) {
        Say '  提示: 找不到 scripts\watch-owner.ps1，关闭 App 时不会自动停服务。' 'Yellow'
        return $false
    }
    if (-not $StopApi -and -not $StopMysql) {
        Say '  没有需要跟着 App 一起停的服务，跳过守护进程。' 'DarkGray'
        return $false
    }

    # 记下 owner 的启动时间：PID 被复用时能识破
    $ownerStart = 0
    try { $ownerStart = (Get-Process -Id $Owner -ErrorAction Stop).StartTime.Ticks } catch { }

    $exe = (Get-Process -Id $PID).Path
    $token = [guid]::NewGuid().ToString('N')
    $parts = @(
        "`"$exe`"", '-NoProfile', '-ExecutionPolicy', 'Bypass',
        '-File', "`"$watcher`"",
        '-OwnerPid', $Owner,
        '-OwnerStartTicks', $ownerStart,
        '-Token', $token
    )
    if ($StopApi) { $parts += '-StopApi' }
    if ($StopMysql) { $parts += '-StopMysql' }
    if ($DataDir) { $parts += @('-DataDir', "`"$DataDir`"") }
    $cmdline = $parts -join ' '

    # 先把「认领令牌」写进状态文件，再拉 watchdog：它自己启动要几百毫秒，
    # 这期间万一用户手工 up / 新实例接管，令牌一变它就自动失效（不做任何事）。
    Clear-OwnerWatchdog
    try {
        ([pscustomobject]@{
                token       = $token
                watchdogPid = 0
                ownerPid    = $Owner
                startedAt   = (Get-Date).ToString('o')
            } | ConvertTo-Json) | Set-Content -Path $WatchState -Encoding utf8
    } catch { }

    try {
        if (Get-Command Start-SilentProcess -ErrorAction SilentlyContinue) {
            $how = Start-SilentProcess -CommandLine $cmdline -Tag 'watch-owner'
        } else {
            $r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = $cmdline }
            if ($r.ReturnValue -ne 0) { throw "ReturnValue=$($r.ReturnValue)" }
            $how = 'wmi-plain'
        }
    } catch {
        Clear-OwnerWatchdog
        Say "  提示: 守护进程没起来（$($_.Exception.Message)），关闭 App 时不会自动停服务。" 'Yellow'
        return $false
    }

    $what = @()
    if ($StopMysql) { $what += 'MySQL' }
    if ($StopApi) { $what += '后端' }
    Say "  已挂守护进程（$how）：App(PID $Owner) 退出后自动停 $($what -join ' + ')" 'DarkGray'
    Trace "watchdog 已挂（owner=$Owner, $how）"
    return $true
}

function Show-RuntimeHintsIfAvailable {
    <#
      启动失败时按需给"还缺哪个运行时、怎么装"的提示。
      runtime-deps.ps1 不在（比如只拷了部分脚本）就静默跳过，不影响启动逻辑本身。
    #>
    if (-not (Get-Command Show-RuntimeFailureHints -ErrorAction SilentlyContinue)) { return }

    $binDir = $null
    try {
        $mysqld = Resolve-MysqlBin 'mysqld.exe'
        if ($mysqld) { $binDir = Split-Path -Parent $mysqld }
    } catch { }

    # Java 交给 runtime-deps.ps1 自己解析（发布包 <根>\jre → PATH → 本机常见 JDK 位置）
    Show-RuntimeFailureHints -ProjectRoot $ProjectRoot -MySqlBinDir $binDir | Out-Null
}

function Do-Up {
    $chain = if ($WithApp) { 'MySQL → 后端 → App' } else { 'MySQL → 后端' }
    Say ''
    Say "  ComfyHub 启动中（$chain）" 'White'
    Trace 'Do-Up 开始'

    # 不带 -OwnerPid 的手工 up = 用户自己接管，撤掉之前 App 留下的守护
    if ($OwnerPid -le 0) { Clear-OwnerWatchdog }

    # 后端是 JVM 程序：没有 Java 就先自动下一个便携版（免安装、免管理员）。
    # 已经有 >=21 的 java 时这里只是几次文件检查 + 一次 java -version，不会联网。
    # COMFYHUB_NO_DOWNLOAD=1 可以关掉这个行为（离线环境/不想让它自己下东西时）。
    if ((Get-Command Ensure-JavaRuntime -ErrorAction SilentlyContinue) -and -not $env:COMFYHUB_NO_DOWNLOAD) {
        try {
            Ensure-JavaRuntime -ProjectRoot $ProjectRoot -Quiet | Out-Null
        } catch {
            Say "  提示: Java 运行时自动获取失败（$($_.Exception.Message)）" 'Yellow'
        }
    }

    if (-not (Start-MySqlPart)) { Show-Status | Out-Null; Show-RuntimeHintsIfAvailable; exit 1 }
    if (-not (Start-ApiPart))   { Show-Status | Out-Null; Show-RuntimeHintsIfAvailable; exit 1 }
    if ($WithApp) { Start-AppPart | Out-Null }

    if ($OwnerPid -gt 0) {
        if ($Script:StartedMysql -or $Script:StartedApi) {
            Start-OwnerWatchdog -Owner $OwnerPid | Out-Null
        } else {
            Say '  服务本来就都在跑，这次不会跟着 App 退出而停止。' 'DarkGray'
            Trace '没启动任何服务，不挂 watchdog'
        }
    }

    Show-Status | Out-Null
    Trace 'Do-Up 结束'
}

# ---------------------------------------------------------------------------
#  停止
# ---------------------------------------------------------------------------

function Do-Down {
    Say ''
    Say '  ComfyHub 停止中（App → 后端 → MySQL）' 'White'

    # 手工停了就别让 watchdog 再补一刀
    Clear-OwnerWatchdog

    if (Get-AppProcess) {
        Say '  [1/3] 关闭桌面 App…' 'Cyan'
        Get-AppProcess | Stop-Process -Force -ErrorAction SilentlyContinue
    } else { Say '  桌面 App 未运行。' 'DarkGray' }

    Say '  [2/3] 停止后端…' 'Cyan'
    & pwsh -NoProfile -File (Join-Path $Scripts 'server.ps1') stop @DataDirArgs

    # 必须等后端真的退出再停库，否则它会一边报错一边重连
    $deadline = (Get-Date).AddSeconds(20)
    while ((Get-Date) -lt $deadline -and @(Get-ApiProcess).Count -gt 0) { Start-Sleep -Milliseconds 400 }

    Say '  [3/3] 停止 MySQL…' 'Cyan'
    & pwsh -NoProfile -File (Join-Path $Scripts 'mysql.ps1') stop @DataDirArgs

    Say ''
    Show-Status | Out-Null
}

function Do-Release {
    <#
      App 退出时**自己**调用的：只停后端 + MySQL，绝不碰 App 进程。
      和 Do-Down 的区别只有一条，但很关键 —— Do-Down 会按进程名 `viewer` 杀 App，
      在"App 自己还活着、正等我们停完服务"的时候调用它等于自杀。

      顺序和 down 一致：后端 → 等 java 真的退出 → MySQL（反过来的话后端会一直对着死库重连报错）。
    #>
    Say ''
    Say '  ComfyHub 停止中（后端 → MySQL，App 不动）' 'White'

    # 手工/App 主动停了就别让 watchdog 再补一刀（它可能和我们同时动手）
    Clear-OwnerWatchdog

    Say '  [1/2] 停止后端…' 'Cyan'
    & pwsh -NoProfile -File (Join-Path $Scripts 'server.ps1') stop @DataDirArgs

    # 必须等后端真的退出再停库，否则它会一边报错一边重连
    $deadline = (Get-Date).AddSeconds(20)
    while ((Get-Date) -lt $deadline -and @(Get-ApiProcess).Count -gt 0) { Start-Sleep -Milliseconds 400 }

    Say '  [2/2] 停止 MySQL…' 'Cyan'
    & pwsh -NoProfile -File (Join-Path $Scripts 'mysql.ps1') stop @DataDirArgs

    Say ''
    Show-Status | Out-Null
}

function Do-Watch {
    <#
      「补挂守护进程」：App 启动时探到后端已经健康（比如你刚在终端里 up 过，
      或者上一次 App 的 watchdog 已经不在了）就直接返回，根本没跑 up ——
      于是 watchdog 没挂上，之后关 App / 硬杀 App 就会把 MySQL + 后端留在后台。

      App 在这种情况下会调一次 `watch -OwnerPid <自己的 PID>`，我们按**当前实际活着**的
      服务来挂守护：谁在跑就准备停谁。已经在跑的库可能是用户手工起的，但既然 App 正在
      用它、而用户又开着"关闭 App 时一并停止本地服务"，就一起带走（和 up -OwnerPid 的语义一致）。
    #>
    if ($OwnerPid -le 0) {
        Say '  watch 需要 -OwnerPid <pid>：它是给"已经在用本地服务的 App"补挂守护进程用的。' 'Yellow'
        Say '  例如: pwsh -File scripts\comfyhub.ps1 watch -OwnerPid 1234' 'DarkGray'
        exit 2
    }

    $stopApi = (@(Get-ApiProcess).Count -gt 0) -or ($null -ne (Get-ApiHealth))
    $stopMysql = Test-MySqlAlive

    if (-not $stopApi -and -not $stopMysql) {
        Say '  后端和 MySQL 都没在跑，不需要挂守护进程。' 'DarkGray'
        Clear-OwnerWatchdog
        return
    }

    $what = @()
    if ($stopMysql) { $what += 'MySQL' }
    if ($stopApi) { $what += '后端' }
    Say ''
    Say "  ComfyHub 补挂退出守护（App PID $OwnerPid 退出后停 $($what -join ' + ')）" 'White'
    Start-OwnerWatchdog -Owner $OwnerPid -StopApi $stopApi -StopMysql $stopMysql | Out-Null
    Say ''
}

function Do-Unwatch {
    <#
      「撤销守护」：用户在设置里把「关闭 App 时一并停止本地服务」**关掉**时调用。

      为什么需要它：守护进程是 App 启动时挂上的，挂上之后就只认状态文件里的令牌。
      用户中途改主意（不想让 App 退出时停服务）而 App 又已经挂了守护的话，
      光靠 release 跳过是不够的 —— 那个守护还活着，App 一死它照样把服务停掉。
      这里删掉令牌文件，守护下一轮（≤3 秒）就会发现自己"已被顶替"并静默退出。
    #>
    Say ''
    $had = Test-Path $WatchState
    Clear-OwnerWatchdog
    if ($had) {
        Say '  已撤销「关 App 自动停服务」的守护进程：本地服务会继续运行。' 'DarkGray'
    } else {
        Say '  当前没有挂守护进程，无需撤销。' 'DarkGray'
    }
    Say ''
}

# ---------------------------------------------------------------------------

function Do-Logs {
    $err = $MysqlErrorLog
    $out = Join-Path $RunDir 'server.out.log'
    Say "──── MySQL 实例目录: $MysqlInstanceDir" 'DarkGray'
    Say "──── MySQL 错误日志: $err" 'DarkGray'
    if (Test-Path $err) { Say '──── MySQL (tail 30) ────' 'Cyan'; Get-Content $err -Tail 30 }
    if (Test-Path $out) { Say '──── 后端 (tail 40) ────' 'Cyan'; Get-Content $out -Tail 40 }
    $se = Join-Path $RunDir 'server.err.log'
    if ((Test-Path $se) -and (Get-Item $se).Length -gt 0) { Say '──── 后端 stderr (tail 20) ────' 'Cyan'; Get-Content $se -Tail 20 }
    $wl = Join-Path $RunDir 'watch-owner.log'
    if (Test-Path $wl) { Say '──── 关 App 停服务守护 (tail 12) ────' 'Cyan'; Get-Content $wl -Tail 12 }
}

function Do-Doctor {
    <#  逐项体检：路径 / 依赖 / 端口占用  #>
    Say ''
    Say '  ComfyHub 环境体检' 'White'
    Say '  ────────────────────────────────────────────────────────────'

    # 后端启动脚本 / App 的路径有两种布局（源码树 vs 装配好的发布包），这里都探一遍
    $packagedBat = Join-Path $ProjectRoot 'server\bin\comfy-hub-server.bat'
    $gradleBat   = Join-Path $ProjectRoot 'server\build\install\comfy-hub-server\bin\comfy-hub-server.bat'
    $batToShow   = if (Test-Path $packagedBat) { $packagedBat } else { $gradleBat }
    $appExe      = Resolve-AppExe
    $layout      = if (Test-Path (Join-Path $ProjectRoot 'viewer.exe')) { '发布包（便携式）' } else { '源码树' }

    # 布局是"是什么"不是"在不在"，不能塞进下面那个 Test-Path 列表里（会被报成"缺失"）
    Say ("  {0,-16} {1}" -f '运行布局', $layout) 'Gray'

    foreach ($p in @(
            @{ n = '项目根目录';    v = $ProjectRoot },
            @{ n = 'db\schema.sql'; v = (Join-Path $ProjectRoot 'db\schema.sql') },
            @{ n = 'MySQL 实例目录'; v = $MysqlInstanceDir },
            @{ n = 'MySQL 数据目录'; v = (Join-Path $MysqlDataPath 'mysql') },
            @{ n = 'MySQL 错误日志'; v = $MysqlErrorLog },
            @{ n = '后端启动脚本';   v = $batToShow },
            @{ n = '桌面 App';      v = $(if ($appExe) { $appExe.FullName } else { (Join-Path $ProjectRoot 'viewer.exe') }) }
        )) {
        $ok = Test-Path $p.v
        Say ("  {0,-16} {1}" -f $p.n, $(if ($ok) { 'OK' } else { "缺失: $($p.v)" })) $(if ($ok) { 'Green' } else { 'Yellow' })
    }
    Say ("  {0,-16} {1}" -f '位置来源', $(if ($DataDir) { "-DataDir 参数" } elseif ($env:COMFYHUB_MYSQL_DIR) { '环境变量 COMFYHUB_MYSQL_DIR' } elseif (Test-Path (Join-Path $ProjectRoot '.mysql-location.json')) { '.mysql-location.json' } else { '默认 .mysql' })) 'Gray'

    $mysqld = Resolve-MysqlBin 'mysqld.exe'
    Say ("  {0,-16} {1}" -f 'mysqld.exe', $(if ($mysqld) { $mysqld } else { '找不到，设置 COMFYHUB_MYSQL_HOME' })) $(if ($mysqld) { 'Green' } else { 'Red' })

    # ComfyUI 在哪（其他建议第 3 条）：发布包里这块最容易出问题，
    # 所以 doctor 里直接把它指出来 —— 找不到时给出该怎么填，而不是让用户自己猜。
    if (Get-Command Resolve-ComfyHome -ErrorAction SilentlyContinue) {
        $comfyHome = Resolve-ComfyHome -ProjectRoot $ProjectRoot
        $comfyOut = Resolve-ComfyOutputDir -ProjectRoot $ProjectRoot
        Say ("  {0,-16} {1}" -f 'ComfyUI 目录', $(if ($comfyHome) { $comfyHome } else { '没找到（后端会在打开 App 时探测；也可以设置 COMFYHUB_COMFY_HOME）' })) $(if ($comfyHome) { 'Green' } else { 'Yellow' })
        if ($comfyOut) {
            Say ("  {0,-16} {1}" -f 'ComfyUI 输出目录', $comfyOut) 'Gray'
        }
    }

    $flutter = (Get-Command flutter -ErrorAction SilentlyContinue)
    Say ("  {0,-16} {1}" -f 'flutter', $(if ($flutter) { $flutter.Source } else { 'PATH 里没有' })) $(if ($flutter) { 'Green' } else { 'Red' })

    foreach ($port in @($MysqlPort, $ApiPort)) {
        $listen = Get-NetTCPConnection -State Listen -LocalPort $port -ErrorAction SilentlyContinue
        Say ("  {0,-16} {1}" -f "端口 $port", $(if ($listen) { "被 PID $($listen[0].OwningProcess) 占用" } else { '空闲' })) 'Gray'
    }

    # 运行时依赖：发布包带不走的那几样（pwsh / VC++ 运行时）在这里现形
    Say ''
    Say '  运行时依赖' 'White'
    Say '  ────────────────────────────────────────────────────────────'
    if (Get-Command Show-RuntimeReport -ErrorAction SilentlyContinue) {
        $mysqldForCheck = Resolve-MysqlBin 'mysqld.exe'
        $binDirForCheck = if ($mysqldForCheck) { Split-Path -Parent $mysqldForCheck } else { $null }
        Show-RuntimeReport -ProjectRoot $ProjectRoot -MySqlBinDir $binDirForCheck | Out-Null
    } else {
        Say '  （找不到 scripts\runtime-deps.ps1，跳过运行时体检）' 'Yellow'
    }
    Say ''
}

switch ($Action) {
    'up'      { Do-Up }
    'down'    { Do-Down }
    'release' { Do-Release }
    'watch'   { Do-Watch }
    'unwatch' { Do-Unwatch }
    'status'  { Show-Status | Out-Null }
    'restart' { Do-Down; Do-Up }
    'logs'    { Do-Logs }
    'doctor'  { Do-Doctor }
}
