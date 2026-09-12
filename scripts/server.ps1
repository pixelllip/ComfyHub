<#
.SYNOPSIS
    ComfyHub Kotlin 后端：构建 / 启动 / 停止。

.DESCRIPTION
    本机 JDK 25 与 Gradle 8.12 不兼容，脚本会自动挑选一个 JDK 21~23。
    优先顺序：-JdkHome 参数 > COMFYHUB_JDK_HOME > 已安装的 JDK 21/22/23 > Android Studio 自带 JBR。

.EXAMPLE
    pwsh -File scripts\server.ps1 run       # 前台启动（Ctrl+C 停止）
    pwsh -File scripts\server.ps1 start     # 后台启动
    pwsh -File scripts\server.ps1 stop
    pwsh -File scripts\server.ps1 fatjar    # 打成一个独立 jar
#>
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateSet('run', 'start', 'stop', 'restart', 'status', 'build', 'fatjar', 'logs', 'test')]
    [string]$Action = 'run',

    [string]$JdkHome = $env:COMFYHUB_JDK_HOME,
    [int]$Port = 8080,
    [switch]$SkipBuild,

    # MySQL 实例目录。留空则按 COMFYHUB_MYSQL_DIR → <项目>\.mysql-location.json → <项目>\.mysql 解析；
    # 有值就原样 -DataDir 透传给 mysql.ps1 start。
    [string]$DataDir
)

$ErrorActionPreference = 'Stop'
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$ServerDir   = Join-Path $ProjectRoot 'server'
$LogDir      = Join-Path $ProjectRoot '.run'
$OutLog      = Join-Path $LogDir 'server.out.log'
$ErrLog      = Join-Path $LogDir 'server.err.log'
$PidFile     = Join-Path $LogDir 'server.pid'

# 静默启动（无窗口 + 脱离进程树）的公共实现，见 scripts\silent-process.ps1
$SilentHelper = Join-Path $PSScriptRoot 'silent-process.ps1'
if (Test-Path $SilentHelper) { . $SilentHelper }

# ---------------------------------------------------------------------------
#  输出编码
# ---------------------------------------------------------------------------
# 被 App / 别的脚本重定向时，pwsh 默认跟随控制台代码页（中文系统 936/GBK），
# 而调用方是按 UTF-8 解的 → 会报 "FormatException: Missing extension byte"。
if ([Console]::IsOutputRedirected) {
    try { [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false) } catch { }
}

# ---------------------------------------------------------------------------
#  耗时打点（COMFYHUB_TRACE=1 时把每步耗时写 stderr，ASCII 不受代码页影响）
# ---------------------------------------------------------------------------
$Script:TraceWatch = [System.Diagnostics.Stopwatch]::StartNew()
function Trace([string]$Step) {
    if (-not $env:COMFYHUB_TRACE) { return }
    $line = '[trace {0,6}ms] {1}' -f $Script:TraceWatch.ElapsedMilliseconds, "server: $Step"
    [Console]::Error.WriteLine($line)
}

function Test-TcpPort([string]$TargetHost, [int]$TargetPort, [int]$TimeoutMs = 200) {
    <# 端口上有没有人在听；连死端口本机要等 SYN 重传（实测 ~2s），这样最多等 200ms。 #>
    $client = [System.Net.Sockets.TcpClient]::new()
    try {
        $iar = $client.BeginConnect($TargetHost, $TargetPort, $null, $null)
        if (-not $iar.AsyncWaitHandle.WaitOne($TimeoutMs)) { return $false }
        $client.EndConnect($iar)
        return $true
    } catch { return $false }
    finally { $client.Dispose() }
}

function Resolve-MysqlInstanceDir {
    <#
      与 mysql.ps1 同一套解析顺序（这里重复实现一遍，不引共享模块）：
        -DataDir → COMFYHUB_MYSQL_DIR → <项目>\.mysql-location.json → <项目>\.mysql
      只用于把错误信息里的日志路径指对地方。
    #>
    $cand = @($DataDir, $env:COMFYHUB_MYSQL_DIR) | Where-Object { $_ } | Select-Object -First 1
    if (-not $cand) {
        $pointer = Join-Path $ProjectRoot '.mysql-location.json'
        if (Test-Path $pointer) {
            try {
                $json = Get-Content -Path $pointer -Raw -Encoding utf8 | ConvertFrom-Json
                if ($json -and $json.dataDir) { $cand = [string]$json.dataDir }
            } catch { }
        }
    }
    if (-not $cand) { return (Join-Path $ProjectRoot '.mysql') }
    if (-not [System.IO.Path]::IsPathRooted($cand)) { $cand = Join-Path $ProjectRoot $cand }
    return [System.IO.Path]::GetFullPath($cand).TrimEnd('\')
}

$MysqlInstanceDir = Resolve-MysqlInstanceDir
$MysqlErrorLog    = Join-Path $MysqlInstanceDir 'mysql-error.log'

# 透传给 mysql.ps1 的 -DataDir（留空就不传，让 mysql.ps1 自己解析）
$DataDirArgs = @()
if ($DataDir) { $DataDirArgs = @('-DataDir', $DataDir) }

# ---------------------------------------------------------------------------
#  找一个能跑 Gradle 8.12 的 JDK（需要 21~23，JDK 24/25 不支持）
# ---------------------------------------------------------------------------
function Resolve-Jdk {
    param([string]$Explicit)

    function Test-Jdk([string]$javaHome) {
        if (-not $javaHome) { return $null }
        $java = Join-Path $javaHome 'bin\java.exe'
        if (-not (Test-Path $java)) { return $null }
        $out = & $java -version 2>&1 | Out-String
        if ($out -match 'version "(\d+)') {
            $major = [int]$Matches[1]
            if ($major -ge 21 -and $major -le 23) { return $javaHome }
        }
        return $null
    }

    foreach ($cand in @(
            $Explicit,
            $env:COMFYHUB_JDK_HOME,
            'D:\tools\jdk-21',
            "$env:ProgramFiles\Eclipse Adoptium\jdk-21*",
            "$env:ProgramFiles\Java\jdk-21*",
            "$env:ProgramFiles\Java\jdk-22*",
            "$env:ProgramFiles\Java\jdk-23*",
            'D:\Program Files\Android Studio\jbr',
            "$env:LOCALAPPDATA\Programs\Android Studio\jbr"
        )) {
        if (-not $cand) { continue }
        foreach ($p in (Resolve-Path $cand -ErrorAction SilentlyContinue)) {
            $ok = Test-Jdk $p.Path
            if ($ok) { return $ok }
        }
    }
    throw @"
找不到可用的 JDK 21~23（Gradle 8.12 不支持 JDK 24/25）。
请安装 JDK 21，或用 -JdkHome / 环境变量 COMFYHUB_JDK_HOME 指定，例如：
  pwsh -File scripts\server.ps1 run -JdkHome 'C:\Program Files\Java\jdk-21'
"@
}

$Jdk = Resolve-Jdk -Explicit $JdkHome
$env:JAVA_HOME = $Jdk
Trace "JDK 已选定: $Jdk"

function Invoke-Gradle {
    param([string[]]$Tasks, [switch]$Quiet)
    Push-Location $ServerDir
    try {
        $gradleArgs = @('-p', $ServerDir, '--console=plain') + $Tasks
        if ($Quiet) { $gradleArgs += '-q' }
        & gradle @gradleArgs
        if ($LASTEXITCODE -ne 0) { throw "Gradle 执行失败: $($Tasks -join ' ')" }
    } finally { Pop-Location }
}

function Get-ServerProcess {
    Get-CimInstance Win32_Process -Filter "Name='java.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -and $_.CommandLine -like '*com.comfyhub.ApplicationKt*' }
}

function Test-Api {
    # 端口没开就直接返回 —— 否则 Invoke-RestMethod 会去等连接超时（本机实测 ~2s）
    if (-not (Test-TcpPort '127.0.0.1' $Port)) { return $null }
    try {
        $r = Invoke-RestMethod "http://127.0.0.1:$Port/api/health" -TimeoutSec 3
        return $r
    } catch { return $null }
}

function Wait-Api([int]$Seconds = 90) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    $round = 0
    while ((Get-Date) -lt $deadline) {
        $round++
        $h = Test-Api
        if ($h) { Trace "后端有响应（轮询第 $round 次）"; return $h }
        Start-Sleep -Milliseconds 200
    }
    return $null
}

function Do-Stop {
    $procs = Get-ServerProcess
    if (-not $procs) { Write-Host '后端未运行。' -ForegroundColor Yellow; return }
    $procs | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    Remove-Item $PidFile -Force -ErrorAction SilentlyContinue
    Write-Host '后端已停止。' -ForegroundColor Green
}

function Start-Detached {
    <#
      先用 WMI 让进程脱离当前 PowerShell 的进程树（否则命令行一结束，
      作为子进程的 java 会被一起回收），再通过一个临时 .cmd 启动脚本执行，
      避免 cmd 的引号 / set 尾空格等一堆坑。

      WMI 这一步走的是 scripts\silent-process.ps1：给它传一个
      Win32_ProcessStartup(ShowWindow = SW_HIDE)，让 cmd 和它拉起来的 java
      都在一个**隐藏**的控制台里跑 —— 否则 App 自动拉起后端时会闪出一个黑框。
    #>
    param([string]$BatchFile, [string]$WorkDir, [string]$StdOut, [string]$StdErr, [string]$StorageDir, [int]$Port)

    $launcher = Join-Path $LogDir 'start-server.cmd'
    $lines = @(
        '@echo off',
        "cd /d `"$WorkDir`"",
        "set `"COMFYHUB_STORAGE=$StorageDir`"",
        "set `"COMFYHUB_PORT=$Port`"",
        "set `"JAVA_HOME=$Jdk`"",
        "set `"PATH=$Jdk\bin;%PATH%`"",
        "`"$BatchFile`" > `"$StdOut`" 2> `"$StdErr`""
    )
    Set-Content -Path $launcher -Value $lines -Encoding ascii

    $cmdline = "cmd.exe /c `"$launcher`""
    if (Get-Command Start-SilentProcess -ErrorAction SilentlyContinue) {
        return Start-SilentProcess -CommandLine $cmdline -WorkingDirectory $WorkDir -Tag 'server'
    }

    Write-Host '  警告: 找不到 scripts\silent-process.ps1，启动时可能会弹一个控制台窗口。' -ForegroundColor Yellow
    $r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = $cmdline }
    if ($r.ReturnValue -ne 0) { throw "创建进程失败 (ReturnValue=$($r.ReturnValue))" }
    return 'wmi-plain'
}

function Resolve-MysqlAdmin {
    # 路径里带通配符时 Get-ChildItem -Filter 不生效，必须逐层解析；解析一次就缓存
    if ($Script:MysqlAdminExe -and (Test-Path $Script:MysqlAdminExe)) { return $Script:MysqlAdminExe }
    $admin = $null
    foreach ($root in @('D:\tools\mysql', 'C:\tools\mysql')) {
        if (-not (Test-Path $root)) { continue }
        $direct = Join-Path $root 'bin\mysqladmin.exe'
        if (Test-Path $direct) { $admin = $direct; break }
        $found = Get-ChildItem -Path $root -Directory -ErrorAction SilentlyContinue |
            ForEach-Object { Join-Path $_.FullName 'bin\mysqladmin.exe' } |
            Where-Object { Test-Path $_ } | Select-Object -First 1
        if ($found) { $admin = $found; break }
    }
    $Script:MysqlAdminExe = $admin
    return $admin
}

function Test-MySqlAlive {
    if (-not (Test-TcpPort '127.0.0.1' 3307)) { return $false }
    $admin = Resolve-MysqlAdmin
    if (-not $admin) { return $false }
    & $admin --protocol=TCP -h 127.0.0.1 -P 3307 -u comfyhub --password=comfyhub ping 2>&1 | Out-Null
    return ($LASTEXITCODE -eq 0)
}

function Ensure-MySql {
    <#  返回 $true 表示数据库确实可用（而不是"尝试过了"）  #>
    Trace 'MySQL 存活检测'
    if (Test-MySqlAlive) { Trace 'MySQL 已在跑'; return $true }

    Write-Host '==> MySQL 未运行，尝试启动...' -ForegroundColor Yellow
    $mysqlScript = Join-Path $PSScriptRoot 'mysql.ps1'
    if (-not (Test-Path $mysqlScript)) {
        Write-Host '找不到 scripts\mysql.ps1' -ForegroundColor Red
        return $false
    }
    # 注意：这里的输出必须吞掉。否则 mysql.ps1 的 stdout 会被当成返回值的一部分，
    # 让 Ensure-MySql 返回"数组"而不是布尔值，下面 Do-Start 里的 -not $mysqlOk 就永远为假，
    # 数据库起不来时后端还是会被硬拉起来（然后一直连不上库）。
    & pwsh -NoProfile -File $mysqlScript start @DataDirArgs | Out-Null
    Trace '已调用 mysql.ps1 start'

    $deadline = (Get-Date).AddSeconds(60)
    while ((Get-Date) -lt $deadline) {
        if (Test-MySqlAlive) { Trace 'MySQL 就绪'; return $true }
        Start-Sleep -Milliseconds 200
    }
    Write-Host "MySQL 启动后仍无法连接，请看 $MysqlErrorLog" -ForegroundColor Red
    return $false
}

function Do-Start {
    Trace 'Do-Start 开始'
    New-Item -ItemType Directory -Force -Path $LogDir | Out-Null

    # 先保证数据库在跑 —— 即使后端进程已经存在也要检查，
    # 否则会出现"后端活着但数据库死了"的假健康状态，`start` 却什么都不做。
    $mysqlOk = Ensure-MySql

    if (Get-ServerProcess) {
        Trace 'CIM 查到后端进程'
        $h = Test-Api
        if ($h -and $h.database -eq 'ok') {
            Write-Host '后端已在运行且健康。' -ForegroundColor Yellow
            Do-Status
            return
        }
        Write-Host '后端在跑但数据库不通，重启它…' -ForegroundColor Yellow
        Do-Stop
        Start-Sleep 1
    }

    if (-not $mysqlOk) {
        throw "数据库不可用，后端起来也用不了。请先执行: pwsh -File scripts\mysql.ps1 start $($DataDirArgs -join ' ')".TrimEnd()
    }

    Write-Host "==> 使用 JDK: $Jdk" -ForegroundColor DarkGray
    if ($SkipBuild) {
        Write-Host '==> 跳过构建（-SkipBuild）' -ForegroundColor DarkGray
    } else {
        Write-Host '==> gradle installDist' -ForegroundColor Cyan
        Invoke-Gradle @('installDist')
    }
    Trace '构建阶段结束（SkipBuild 则跳过）'

    $bat = Join-Path $ServerDir 'build\install\comfy-hub-server\bin\comfy-hub-server.bat'
    if (-not (Test-Path $bat)) { throw "找不到启动脚本: $bat（去掉 -SkipBuild 重新构建）" }

    Write-Host "==> 后台启动后端 (port $Port)" -ForegroundColor Cyan
    $storageDir = Join-Path $ProjectRoot 'storage'
    New-Item -ItemType Directory -Force -Path $storageDir | Out-Null
    $how = Start-Detached -BatchFile $bat -WorkDir $ServerDir -StdOut $OutLog -StdErr $ErrLog -StorageDir $storageDir -Port $Port
    Trace "启动方式=$how，开始等后端健康"

    $h = Wait-Api
    if ($h -and $h.database -eq 'ok') {
        # PID 在进程起来之后按命令行去找（静默启动那条路拿不到新进程的 PID）
        $proc = Get-ServerProcess
        if ($proc) { Set-Content -Path $PidFile -Value (($proc.ProcessId | Sort-Object) -join ',') }
        Write-Host "后端就绪: http://127.0.0.1:$Port  (db=$($h.database))" -ForegroundColor Green
    } elseif ($h) {
        Write-Host "后端起来了但数据库不通 (db=$($h.database))，日志尾部：" -ForegroundColor Red
        if (Test-Path $OutLog) { Get-Content $OutLog -Tail 30 }
        throw '后端数据库连接异常'
    } else {
        Write-Host '启动超时，日志尾部：' -ForegroundColor Red
        if (Test-Path $OutLog) { Get-Content $OutLog -Tail 30 }
        if (Test-Path $ErrLog) { Get-Content $ErrLog -Tail 30 }
        throw '后端启动失败'
    }
}

function Do-Status {
    # 一体式状态：数据库 + 后端（因为后端的"可用"本来就依赖数据库）
    $mysqlAlive = Test-MySqlAlive
    if ($mysqlAlive) {
        Write-Host "MySQL:     运行中 127.0.0.1:3307" -ForegroundColor Green
    } else {
        Write-Host 'MySQL:     未运行（后端将无法工作）' -ForegroundColor Red
    }

    $procs = Get-ServerProcess
    if ($procs) {
        Write-Host "后端进程:  PID $($procs.ProcessId -join ',')" -ForegroundColor Green
    } else {
        Write-Host '后端进程:  未运行' -ForegroundColor Yellow
    }

    $h = Test-Api
    if ($h -and $h.database -eq 'ok') {
        Write-Host "健康检查:  OK  version=$($h.version)  db=ok" -ForegroundColor Green
        Write-Host "存储目录:  $($h.storageDir)"
    } elseif ($h) {
        Write-Host "健康检查:  异常  db=$($h.database)" -ForegroundColor Red
    } else {
        Write-Host '健康检查:  无法连接' -ForegroundColor Red
    }
}

switch ($Action) {
    'build'  { Write-Host "==> JDK: $Jdk" -ForegroundColor DarkGray; Invoke-Gradle @('build'); Write-Host '构建完成' -ForegroundColor Green }
    'test'   { Write-Host "==> JDK: $Jdk" -ForegroundColor DarkGray; Invoke-Gradle @('test'); Write-Host '测试完成' -ForegroundColor Green }
    'fatjar' { Write-Host "==> JDK: $Jdk" -ForegroundColor DarkGray; Invoke-Gradle @('fatJar'); Get-ChildItem (Join-Path $ServerDir 'build\libs') | Select-Object Name, Length }
    'start'  { Do-Start }
    'stop'   { Do-Stop }
    'restart'{ Do-Stop; Start-Sleep 1; Do-Start }
    'status' { Do-Status }
    'logs'   { if (Test-Path $OutLog) { Get-Content $OutLog -Tail 80 }; if (Test-Path $ErrLog) { Write-Host '--- stderr ---'; Get-Content $ErrLog -Tail 40 } }
    'run' {
        Write-Host "==> 使用 JDK: $Jdk" -ForegroundColor DarkGray
        Write-Host "==> 前台运行（Ctrl+C 停止），监听 $Port" -ForegroundColor Cyan
        Push-Location $ServerDir
        try { & gradle -p $ServerDir --console=plain run } finally { Pop-Location }
    }
}
