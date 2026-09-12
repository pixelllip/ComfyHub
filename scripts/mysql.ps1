<#
.SYNOPSIS
    ComfyHub 本地 MySQL 管理脚本（便携版，免安装、免管理员权限）。

.DESCRIPTION
    使用解压版的 MySQL 8.4，实例目录默认在项目内的 .mysql（数据在 .mysql\data），
    监听 127.0.0.1:3307（避开常见的 3306 以免与本机已有 MySQL 冲突）。

    实例目录可以换到别的盘/目录，解析顺序（四个脚本共用同一套规则）：
      1. 参数 -DataDir <路径>
      2. 环境变量 COMFYHUB_MYSQL_DIR
      3. 指针文件 <项目>\.mysql-location.json 里的 dataDir
      4. 默认 <项目>\.mysql
    相对路径按项目根目录展开，最终都会规范化成绝对路径。

.EXAMPLE
    pwsh -File scripts\mysql.ps1 init      # 首次初始化数据目录 + 建库建表 + 演示数据
    pwsh -File scripts\mysql.ps1 start     # 启动
    pwsh -File scripts\mysql.ps1 status    # 查看状态（会打印实际使用的实例目录）
    pwsh -File scripts\mysql.ps1 cli       # 打开 mysql 命令行
    pwsh -File scripts\mysql.ps1 stop      # 停止
    pwsh -File scripts\mysql.ps1 move -DataDir 'D:\mysql\comfyhub'   # 搬家（停库 → 复制 → 改指针 → 重启）
#>
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateSet('init', 'start', 'stop', 'restart', 'status', 'cli', 'schema', 'seed', 'reset', 'logs', 'move')]
    [string]$Action = 'status',

    [string]$MySqlHome = $env:COMFYHUB_MYSQL_HOME,
    [int]$Port = 3307,
    [string]$DbUser = 'root',
    [string]$DbPassword = '',
    [switch]$WithServer,

    # 实例目录（里面放 data\、my.ini、mysql-error.log、mysqld.pid）。
    # 只有 move 例外：此时 -DataDir 是"要搬到哪里"，源目录按 env → 指针 → 默认 解析。
    [string]$DataDir = $env:COMFYHUB_MYSQL_DIR,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$PointerFile = Join-Path $ProjectRoot '.mysql-location.json'
$DbName      = 'comfy_hub'

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
    $line = '[trace {0,6}ms] {1}' -f $Script:TraceWatch.ElapsedMilliseconds, "mysql: $Step"
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

# -DataDir 的原始值先留一份：move 时它就是目标目录
$RequestedDir = $DataDir

# 下面这些路径统一由 Set-InstancePaths 按解析结果赋值
$InstanceDir = $null
$DataRoot    = $null   # 实例目录（与 $InstanceDir 同义，保留旧名字少改代码）
$DataDir     = $null   # 物理数据目录 = <实例目录>\data
$IniFile     = $null
$ErrorLog    = $null
$PidFile     = $null

# ---------------------------------------------------------------------------
#  实例目录解析（-DataDir → COMFYHUB_MYSQL_DIR → 指针文件 → 项目内默认 .mysql）
# ---------------------------------------------------------------------------

function Resolve-InstanceDir([string]$Path) {
    <#  相对路径按项目根目录展开，返回规范化的绝对路径（去掉结尾反斜杠）  #>
    if (-not $Path) { return $null }
    $p = $Path.Trim().Trim('"')
    if (-not $p) { return $null }
    if (-not [System.IO.Path]::IsPathRooted($p)) { $p = Join-Path $ProjectRoot $p }
    try {
        return [System.IO.Path]::GetFullPath($p).TrimEnd('\')
    } catch {
        throw "存储位置路径不合法：$p（$($_.Exception.Message)）"
    }
}

function Get-PointerFileDataDir {
    <#  读取 <项目>\.mysql-location.json 的 dataDir；文件不存在或内容坏掉都返回 $null  #>
    if (-not (Test-Path $PointerFile)) { return $null }
    try {
        $json = Get-Content -Path $PointerFile -Raw -Encoding utf8 | ConvertFrom-Json
        if ($json -and $json.dataDir) { return [string]$json.dataDir }
    } catch {
        Write-Host "警告: $PointerFile 解析失败，已忽略（$($_.Exception.Message)）" -ForegroundColor Yellow
    }
    return $null
}

function Set-PointerFileDataDir([string]$Dir) {
    <#  写下位置指针，之后别的脚本不带参数也能解析到同一个实例目录  #>
    ([pscustomobject]@{ dataDir = $Dir }) | ConvertTo-Json |
        Set-Content -Path $PointerFile -Encoding utf8
    Write-Host "==> 已记录存储位置: $PointerFile" -ForegroundColor Cyan
}

function Get-EffectiveInstanceDir([string]$Explicit) {
    $dir = Resolve-InstanceDir $Explicit
    if (-not $dir) { $dir = Resolve-InstanceDir (Get-PointerFileDataDir) }
    if (-not $dir) { $dir = Join-Path $ProjectRoot '.mysql' }
    return $dir
}

function Set-InstancePaths([string]$Dir) {
    <#  把 data\ 配置 / 日志 / pid 全部指向 $Dir  #>
    $script:InstanceDir = $Dir
    $script:DataRoot    = $Dir
    $script:DataDir     = Join-Path $Dir 'data'
    $script:IniFile     = Join-Path $Dir 'my.ini'
    $script:ErrorLog    = Join-Path $Dir 'mysql-error.log'
    $script:PidFile     = Join-Path $Dir 'mysqld.pid'
}

function Resolve-MySqlHome {
    <#
      顺序：-MySqlHome / COMFYHUB_MYSQL_HOME  →  **发布包自带的 <根>\mysql**  →  本机 D:\tools、C:\tools。

      发布包那一项必须排在 D:\tools 前面：装配好的发布包里自带一份便携版 MySQL
      （见 scripts\pack-release.ps1 / packaging\manifest.json），如果还去找开发机的
      D:\tools\mysql，换台机器就崩 —— 这正是以前"Release 目录拷给别人跑不起来"的原因之一。
    #>
    if ($MySqlHome -and (Test-Path (Join-Path $MySqlHome 'bin\mysqld.exe'))) { return $MySqlHome }
    $candidates = @(
        (Join-Path $ProjectRoot 'mysql'),
        'D:\tools\mysql\mysql-8.4.3-winx64',
        'C:\tools\mysql\mysql-8.4.3-winx64'
    ) + (Get-ChildItem -Path 'D:\tools\mysql', 'C:\tools\mysql' -Directory -ErrorAction SilentlyContinue |
            ForEach-Object { $_.FullName })
    foreach ($c in $candidates) {
        if ($c -and (Test-Path (Join-Path $c 'bin\mysqld.exe'))) { return $c }
    }
    throw "找不到 mysqld.exe。请设置环境变量 COMFYHUB_MYSQL_HOME 指向 MySQL 解压目录。"
}

$MySqlHome = Resolve-MySqlHome
$Mysqld    = Join-Path $MySqlHome 'bin\mysqld.exe'
$MysqlCli  = Join-Path $MySqlHome 'bin\mysql.exe'
$AdminCli  = Join-Path $MySqlHome 'bin\mysqladmin.exe'

function Write-Ini {
    $basedir = $MySqlHome -replace '\\', '/'
    $datadir = $DataDir   -replace '\\', '/'
    $errlog  = $ErrorLog  -replace '\\', '/'
    $content = @"
[mysqld]
basedir=$basedir
datadir=$datadir
port=$Port
bind-address=127.0.0.1
mysqlx=0
character-set-server=utf8mb4
collation-server=utf8mb4_unicode_ci
max_connections=200
max_allowed_packet=512M
innodb_buffer_pool_size=256M
innodb_flush_log_at_trx_commit=2
log-error=$errlog
local_infile=0

[client]
port=$Port
host=127.0.0.1
default-character-set=utf8mb4
"@
    New-Item -ItemType Directory -Force -Path $DataRoot | Out-Null
    Set-Content -Path $IniFile -Value $content -Encoding utf8
}

function Get-ServerProcess {
    <#
      mysqld 的 CommandLine 形如: "D:\tools\mysql\...\mysqld.exe" --defaults-file="D:\...\my.ini"
      CIM 里 Windows 路径可能带引号，所以两边都去掉引号再比，比的是解析后的 $IniFile。
    #>
    $needle = $IniFile.Trim('"')
    Get-CimInstance Win32_Process -Filter "Name='mysqld.exe'" -ErrorAction SilentlyContinue |
        Where-Object {
            if (-not $_.CommandLine) { return $false }
            ($_.CommandLine -replace '"', '') -like "*$needle*"
        }
}

function Test-Alive {
    # 先花 ≤300ms 看端口，再让 mysql 客户端去连；库没起时省掉一次 2 秒的 SYN 重传等待
    if (-not (Test-TcpPort '127.0.0.1' $Port)) { return $false }
    $out = & $MysqlCli --protocol=TCP -h 127.0.0.1 -P $Port -u $DbUser "--password=$DbPassword" `
        -N -B -e "SELECT 1;" 2>&1
    return ($LASTEXITCODE -eq 0)
}

function Invoke-Sql([string]$Sql, [string]$Database) {
    $cliArgs = @('--protocol=TCP', '-h', '127.0.0.1', '-P', "$Port", '-u', $DbUser, "--password=$DbPassword",
                 '--default-character-set=utf8mb4')
    if ($Database) { $cliArgs += $Database }
    $cliArgs += @('-e', $Sql)
    & $MysqlCli @cliArgs
    if ($LASTEXITCODE -ne 0) { throw "SQL 执行失败: $Sql" }
}

function Invoke-SqlFile([string]$File, [string]$Database) {
    if (-not (Test-Path $File)) { throw "找不到 SQL 文件: $File" }
    Write-Host "==> 导入 $File" -ForegroundColor Cyan
    $cliArgs = @('--protocol=TCP', '-h', '127.0.0.1', '-P', "$Port", '-u', $DbUser, "--password=$DbPassword",
                 '--default-character-set=utf8mb4')
    if ($Database) { $cliArgs += $Database }
    $cliArgs += @('-e', "source $($File -replace '\\','/')")
    & $MysqlCli @cliArgs
    if ($LASTEXITCODE -ne 0) { throw "导入失败: $File" }
}

function Do-EnsureAppUser {
    Write-Host "==> 创建应用账号 comfyhub" -ForegroundColor Cyan
    Invoke-Sql @"
CREATE USER IF NOT EXISTS 'comfyhub'@'localhost' IDENTIFIED BY 'comfyhub';
CREATE USER IF NOT EXISTS 'comfyhub'@'127.0.0.1' IDENTIFIED BY 'comfyhub';
ALTER USER 'comfyhub'@'localhost' IDENTIFIED BY 'comfyhub';
ALTER USER 'comfyhub'@'127.0.0.1' IDENTIFIED BY 'comfyhub';
GRANT ALL PRIVILEGES ON ``$DbName``.* TO 'comfyhub'@'localhost';
GRANT ALL PRIVILEGES ON ``$DbName``.* TO 'comfyhub'@'127.0.0.1';
FLUSH PRIVILEGES;
"@ $null
}

function Wait-Ready([int]$Seconds = 60) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    $round = 0
    while ((Get-Date) -lt $deadline) {
        $round++
        if (Test-Alive) { Trace "就绪（轮询第 $round 次）"; return $true }
        Start-Sleep -Milliseconds 200
    }
    return $false
}

function Do-Init {
    <#
      -SkipSeed：只建库建表 + 建应用账号，不灌演示数据。
      自动初始化（Do-Start 发现数据目录不存在时）走的就是这个 ——
      不能悄悄往用户库里塞演示提示词。
    #>
    param([switch]$SkipSeed)

    if (Test-Path (Join-Path $DataDir 'mysql')) {
        Write-Host "数据目录已存在，跳过 initialize：$DataDir" -ForegroundColor Yellow
    } else {
        New-Item -ItemType Directory -Force -Path $DataDir | Out-Null
        Write-Ini
        Write-Host "==> mysqld --initialize-insecure（root 空密码）" -ForegroundColor Cyan
        & $Mysqld "--defaults-file=$IniFile" --initialize-insecure --console
        if ($LASTEXITCODE -ne 0) { throw "MySQL 初始化失败，请查看 $ErrorLog" }
    }
    Write-Ini
    Do-Start
    if (-not (Wait-Ready)) { throw "MySQL 启动超时，请查看 $ErrorLog" }

    Write-Host "==> 创建数据库与表结构" -ForegroundColor Cyan
    Invoke-SqlFile (Join-Path $ProjectRoot 'db\schema.sql') $null
    Do-EnsureAppUser
    if ($SkipSeed) {
        Write-Host '==> 跳过演示数据（要演示数据执行: pwsh -File scripts\mysql.ps1 seed）' -ForegroundColor DarkGray
    } else {
        Write-Host "==> 写入演示数据" -ForegroundColor Cyan
        Invoke-SqlFile (Join-Path $ProjectRoot 'db\seed.sql') $DbName
    }
    Do-Status
}

function Start-Detached {
    <#
      用 WMI 创建进程，让它「脱离当前 PowerShell 的进程树 + 不弹窗口」。
      （命令行一结束，作为子进程的 mysqld 会被一起回收，所以必须脱离进程树；
        WMI 默认会给控制台程序分配一个新的可见窗口，所以必须显式 SW_HIDE，
        细节见 scripts\silent-process.ps1。）

      helper 缺失时退回裸 WMI：进程照样能起来，只是会闪一个控制台窗口。
    #>
    param([string]$FilePath, [string]$Arguments)
    $cmdline = "`"$FilePath`" $Arguments"
    if (Get-Command Start-SilentProcess -ErrorAction SilentlyContinue) {
        return Start-SilentProcess -CommandLine $cmdline -Tag 'mysqld'
    }
    Write-Host "  警告: 找不到 scripts\silent-process.ps1，启动时可能会弹一个控制台窗口。" -ForegroundColor Yellow
    $r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = $cmdline }
    if ($r.ReturnValue -ne 0) { throw "创建进程失败 (ReturnValue=$($r.ReturnValue))" }
    return 'wmi-plain'
}

function Do-Start {
    Trace 'Do-Start 开始'
    if (Get-ServerProcess) {
        Write-Host "mysqld 已在运行。" -ForegroundColor Yellow
        Trace 'mysqld 已在运行'
        return
    }

    # 数据目录还不存在就直接起 mysqld 一定失败（"Can't find data directory"）。
    # 全新解压的发布包、或刚 clone 下来还没跑过 init 的源码树都会走到这里 ——
    # 补一次初始化，让"首次双击 viewer.exe"不用先手敲一条 init 就能用。
    # （Do-Init 内部也会调 Do-Start，那时数据目录已经建好了，不会再进这个分支。）
    if (-not (Test-Path (Join-Path $DataDir 'mysql'))) {
        Write-Host '==> 数据目录还没初始化，先建库建表（首次启动会慢一点）…' -ForegroundColor Cyan
        Do-Init -SkipSeed
        return
    }

    if (-not (Test-Path $IniFile)) { Write-Ini }
    Write-Host "==> 启动 mysqld (port $Port)" -ForegroundColor Cyan
    $how = Start-Detached -FilePath $Mysqld -Arguments "--defaults-file=`"$IniFile`""
    Trace "启动方式=$how，开始等就绪"
    if (Wait-Ready) {
        # PID 在进程起来之后按命令行去找（静默启动那条路拿不到新进程的 PID）
        $proc = Get-ServerProcess
        if ($proc) { Set-Content -Path $PidFile -Value (($proc.ProcessId | Sort-Object) -join ',') }
        Write-Host "MySQL 已就绪：127.0.0.1:$Port" -ForegroundColor Green
    } else {
        Write-Host "启动超时，请查看 $ErrorLog" -ForegroundColor Red
    }
}

function Get-ApiProcess {
    Get-CimInstance Win32_Process -Filter "Name='java.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -and $_.CommandLine -like '*com.comfyhub.ApplicationKt*' }
}

function Do-Stop {
    $procs = Get-ServerProcess
    if (-not $procs) { Write-Host "mysqld 未运行。" -ForegroundColor Yellow; return }

    # 库停了后端就没法工作了，先提醒（并用 -WithServer 一起停）
    $api = Get-ApiProcess
    if ($api) {
        if ($WithServer) {
            Write-Host "==> 连同后端一起停止（数据库是后端的依赖）" -ForegroundColor Cyan
            & pwsh -NoProfile -File (Join-Path $PSScriptRoot 'server.ps1') stop
            $deadline = (Get-Date).AddSeconds(20)
            while ((Get-Date) -lt $deadline -and (Get-ApiProcess)) { Start-Sleep -Milliseconds 400 }
        } else {
            Write-Host "注意: 后端(PID $($api.ProcessId -join ',') )还在跑，停库后它会连不上数据库。" -ForegroundColor Yellow
            Write-Host "      想一起停请用: pwsh -File scripts\comfyhub.ps1 down" -ForegroundColor Yellow
        }
    }

    Write-Host "==> mysqladmin shutdown" -ForegroundColor Cyan
    & $AdminCli --protocol=TCP -h 127.0.0.1 -P $Port -u $DbUser "--password=$DbPassword" shutdown 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    if (Get-ServerProcess) {
        Get-ServerProcess | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    }
    Write-Host "已停止。" -ForegroundColor Green
}

function Do-Status {
    $proc = Get-ServerProcess
    if ($proc) {
        Write-Host "mysqld 运行中 (PID $($proc.ProcessId -join ','))" -ForegroundColor Green
        if (Test-Alive) {
            $v = & $MysqlCli --protocol=TCP -h 127.0.0.1 -P $Port -u $DbUser "--password=$DbPassword" -N -B -e "SELECT VERSION();"
            Write-Host "  版本: $v"
            $t = & $MysqlCli --protocol=TCP -h 127.0.0.1 -P $Port -u $DbUser "--password=$DbPassword" -N -B `
                -e "SELECT CONCAT('prompts=', (SELECT COUNT(*) FROM prompts), ' media=', (SELECT COUNT(*) FROM media_assets), ' tags=', (SELECT COUNT(*) FROM tags)) FROM DUAL;" $DbName 2>$null
            Write-Host "  数据: $t"
        }
    } else {
        Write-Host "mysqld 未运行。口令: pwsh -File scripts\mysql.ps1 start" -ForegroundColor Yellow
    }
    Write-Host "  实例目录: $InstanceDir"
    Write-Host "  数据目录: $DataDir"
    Write-Host "  配置文件: $IniFile"
    Write-Host "  错误日志: $ErrorLog"
}

function Do-Move {
    <#
      把实例目录整体搬到新位置：
        1) 先停库（不停库复制出来的 InnoDB 文件不可靠）
        2) 目标里已有 data\ 且没加 -Force 就拒绝
        3) robocopy /E /COPY:DAT /R:1 /W:1（退出码 < 8 视为成功）
        4) 按新位置重写 my.ini，并写下指针文件 <项目>\.mysql-location.json
        5) 从新位置启动，等就绪后打印状态
      源目录不会被删除，确认新位置一切正常后自行删除即可。
    #>
    param([switch]$Force)

    if (-not $RequestedDir) {
        Write-Host '请用 -DataDir 指定新的存储位置，例如:' -ForegroundColor Yellow
        Write-Host "  pwsh -File scripts\mysql.ps1 move -DataDir 'D:\mysql\comfyhub'" -ForegroundColor Yellow
        return
    }

    $target = Resolve-InstanceDir $RequestedDir
    # -DataDir 在这里是"目标"，所以源目录只看 env → 指针 → 默认，避免自己搬给自己
    $source = Get-EffectiveInstanceDir -Explicit $env:COMFYHUB_MYSQL_DIR

    if ($target -eq $source) {
        Write-Host "目标目录与当前实例目录相同，无需移动：$source" -ForegroundColor Yellow
        return
    }
    if (-not (Test-Path $source)) {
        throw "当前实例目录不存在：$source（先执行: pwsh -File scripts\mysql.ps1 init）"
    }
    if (-not (Test-Path (Join-Path $source 'data'))) {
        throw "当前实例目录里没有 data 子目录：$source（先执行: pwsh -File scripts\mysql.ps1 init）"
    }

    Set-InstancePaths $source
    Write-Host "==> 停止 MySQL（复制数据文件前必须停库）" -ForegroundColor Cyan
    Do-Stop

    $targetData = Join-Path $target 'data'
    if (Test-Path $targetData) {
        if (-not $Force) {
            throw "目标目录已有数据：$targetData`n确认要覆盖它请加 -Force 重试（源目录不会被动）。"
        }
        Write-Host "目标已有 data 子目录，-Force 已指定，继续覆盖：$targetData" -ForegroundColor Yellow
    }
    New-Item -ItemType Directory -Force -Path $target | Out-Null

    Write-Host "==> 复制实例目录" -ForegroundColor Cyan
    Write-Host "    $source  ->  $target"
    & robocopy $source $target /E /COPY:DAT /R:1 /W:1 | Out-Null
    $rc = $LASTEXITCODE
    if ($rc -ge 8) { throw "robocopy 复制失败 (exit=$rc)，源目录未改动：$source" }

    # 复制过去的 my.ini 里还是旧路径，必须按新位置重写
    Set-InstancePaths $target
    Write-Ini
    Set-PointerFileDataDir $target

    Write-Host "==> 从新位置启动 MySQL" -ForegroundColor Cyan
    Do-Start
    if (-not (Wait-Ready)) { Write-Host "MySQL 启动超时，请查看 $ErrorLog" -ForegroundColor Red }
    Do-Status

    Write-Host "源目录仍在原处，确认新位置正常后可手动删除：$source" -ForegroundColor Yellow
    Write-Host "若新位置有问题，删掉 $PointerFile 即可回到 $source" -ForegroundColor Yellow
}

# 解析出本次实际使用的实例目录，之后所有函数都按它工作
Set-InstancePaths (Get-EffectiveInstanceDir -Explicit $RequestedDir)

switch ($Action) {
    'init'    { Do-Init }
    'start'   { Do-Start }
    'stop'    { Do-Stop }
    'restart' { Do-Stop; Do-Start }
    'status'  { Do-Status }
    'schema'  { Invoke-SqlFile (Join-Path $ProjectRoot 'db\schema.sql') $null; Write-Host '建表完成' -ForegroundColor Green }
    'seed'    { Invoke-SqlFile (Join-Path $ProjectRoot 'db\seed.sql') $DbName; Write-Host '演示数据完成' -ForegroundColor Green }
    'reset'   { Do-Stop; Remove-Item -Recurse -Force $DataDir -ErrorAction SilentlyContinue; Do-Init }
    'cli'     { & $MysqlCli --protocol=TCP -h 127.0.0.1 -P $Port -u $DbUser "--password=$DbPassword" $DbName }
    'logs'    { if (Test-Path $ErrorLog) { Get-Content $ErrorLog -Tail 60 } else { Write-Host '暂无日志' } }
    'move'    { Do-Move -Force:$Force }
}
