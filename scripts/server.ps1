<#
.SYNOPSIS
    ComfyHub Kotlin 后端：构建 / 启动 / 停止 / 搬迁老库。

.DESCRIPTION
    P2（2026-09-23）起后端的库是 **SQLite**（`<项目根>\.db\comfyhub.db`），
    不再需要 MySQL 先就绪 —— 只有 `migrate`（把老 MySQL 数据搬进新库）那一步才用得到它。

    本机 JDK 25 与 Gradle 8.12 不兼容，脚本会自动挑选一个 JDK 21~23。
    优先顺序：-JdkHome 参数 > COMFYHUB_JDK_HOME > 已安装的 JDK 21/22/23 > Android Studio 自带 JBR。

.EXAMPLE
    pwsh -File scripts\server.ps1 run       # 前台启动（Ctrl+C 停止）
    pwsh -File scripts\server.ps1 start     # 后台启动
    pwsh -File scripts\server.ps1 stop
    pwsh -File scripts\server.ps1 migrate   # 一次性把老 MySQL 库搬进 SQLite（P2）
    pwsh -File scripts\server.ps1 fatjar    # 打成一个独立 jar
#>
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateSet('run', 'start', 'stop', 'restart', 'status', 'build', 'fatjar', 'logs', 'test', 'migrate')]
    [string]$Action = 'run',

    [string]$JdkHome = $env:COMFYHUB_JDK_HOME,

    # 后端实现：auto（默认：有 Rust 二进制就用 Rust，否则 Kotlin）/ rust / kotlin。
    # 也可以不传参数、用环境变量 COMFYHUB_BACKEND —— 两者同义，命令行优先。
    [ValidateSet('auto', 'rust', 'kotlin')]
    [string]$Backend = $env:COMFYHUB_BACKEND,

    # API 端口。**默认 0 = 交给操作系统自动挑**（用户 2026-09-22）。
    # 解析顺序由 scripts\api-port.ps1 统一定：-Port > COMFYHUB_PORT > .run\api.json > 0。
    [int]$Port = 0,
    [switch]$SkipBuild,

    # MySQL 实例目录。留空则按 COMFYHUB_MYSQL_DIR → <项目>\.mysql-location.json → <项目>\.mysql 解析；
    # 有值就原样 -DataDir 透传给 mysql.ps1 start。
    [string]$DataDir
)

$ErrorActionPreference = 'Stop'
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$ServerDir   = Join-Path $ProjectRoot 'legacy\server'   # Kotlin 后端已退役（P4 收尾），源码留在 legacy\ 里备查
$LogDir      = Join-Path $ProjectRoot '.run'
$OutLog      = Join-Path $LogDir 'server.out.log'
$ErrLog      = Join-Path $LogDir 'server.err.log'
$PidFile     = Join-Path $LogDir 'server.pid'

# 后端启动脚本有两个可能的布局：
#   · 开发布局：Gradle installDist 的产物     <根>\server\build\install\comfy-hub-server\bin\
#   · 发布包布局：scripts\pack-release.ps1 装配的 <根>\server\bin\（只有 bin\ + lib\，没有 Gradle 源码）
# 两边都认，App / 脚本在源码树里和装配好的发布包里行为一致。
$GradleServerBat   = Join-Path $ServerDir 'build\install\comfy-hub-server\bin\comfy-hub-server.bat'
$PackagedServerBat = Join-Path $ServerDir 'bin\comfy-hub-server.bat'

# 静默启动（无窗口 + 脱离进程树）的公共实现，见 scripts\silent-process.ps1
$SilentHelper = Join-Path $PSScriptRoot 'silent-process.ps1'
if (Test-Path $SilentHelper) { . $SilentHelper }

# 端口解析的唯一真源（与后端 ApiPortFile.kt、前端 settings_store.dart、Python 扩展共读一个文件）
$ApiPortHelper = Join-Path $PSScriptRoot 'api-port.ps1'
if (Test-Path $ApiPortHelper) { . $ApiPortHelper }

# 这次到底用哪个端口。`$Port` 为 0 时先看 .run\api.json 里上次那个"还在听"的端口；
# 都没有就还是 0（后端会让操作系统挑）。**命令行的 Port 参数在解析里永远优先**。
$Script:ApiPort = Resolve-ApiPort -ProjectRoot $ProjectRoot -Explicit $Port
$Port = $Script:ApiPort.Port
if ($Script:ApiPort.Source -eq 'state') {
    Write-Host ("==> 沿用上次的 API 端口 {0}（.run\api.json；想换端口用 -Port 指定）" -f $Port) -ForegroundColor DarkGray
} elseif ($Port -le 0) {
    Write-Host '==> API 端口自动（0 = 交给操作系统挑一个空闲端口，真实端口写进 .run\api.json）' -ForegroundColor DarkGray
}

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
            # 发布包自带的运行时（scripts\pack-release.ps1 装到 <根>\jre）。
            # 目标机器不能假设装了 JDK，所以这一项要排在开发机的那些路径前面。
            (Join-Path $ProjectRoot 'jre'),
            # 机器自己装的那个（JAVA_HOME 是"这台机器有 JDK"最常见的表示法）。
            # 检测优先：机器上有 21~23 就用机器的，别再下载/指派别的。
            $env:JAVA_HOME,
            'D:\tools\jdk-21',
            'D:\tools\jdk-2*',
            "$env:ProgramFiles\Eclipse Adoptium\jdk-2*",
            "$env:ProgramFiles\Java\jdk-2*",
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

$JdkResolved = $null
<#  懒解析 JDK：**只有真的要跑 Kotlin / Gradle 时才找它**。

    为什么不能像以前那样在脚本开头就 `Resolve-Jdk`（找不到直接 throw）：切流之后
    Rust 后端**完全不需要 JDK**，而"没装 JDK 21~23 的机器"正是发布包要支持的那一类 ——
    开头就抛等于"Rust 后端在一台没 JDK 的机器上根本起不来"。
    这台机器上唯一那份 21 就是 Android Studio 的 JBR，历史上也确实因为找不到而卡过（pitfalls 有记）。  #>
function Get-Jdk {
    if ($Script:JdkResolved) { return $Script:JdkResolved }
    $Script:JdkResolved = Resolve-Jdk -Explicit $JdkHome
    $env:JAVA_HOME = $Script:JdkResolved
    Trace "JDK 已选定: $Script:JdkResolved"
    return $Script:JdkResolved
}

# 后端选择（Rust / Kotlin）住在公共文件里：`comfyhub.ps1 doctor` 读的是同一条规则
$BackendHelper = Join-Path $PSScriptRoot 'backend.ps1'
if (Test-Path $BackendHelper) { . $BackendHelper }
$Script:BackendInfo = Resolve-Backend -Preference $Backend -ProjectRoot $ProjectRoot
$Script:BackendKind = $Script:BackendInfo.Kind
Trace ("后端: {0}（{1}）" -f $Script:BackendKind, $Script:BackendInfo.Note)

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
    # 认**当前选中的那个后端**（两个都可能在跑，见 `Get-AnyBackendProcess`）
    Get-BackendProcess -Kind $Script:BackendKind
}

function Test-Api {
    # 端口没开就直接返回 —— 否则 Invoke-RestMethod 会去等连接超时（本机实测 ~2s）
    if ($Port -le 0) { return $null }
    if (-not (Test-TcpPort '127.0.0.1' $Port)) { return $null }
    try {
        $r = Invoke-RestMethod "http://127.0.0.1:$Port/api/health" -TimeoutSec 3
        return $r
    } catch { return $null }
}

<#
    等后端可用。

    **不能只看自己那个候选端口**：restart 期间"文件里写的"和"真的在听的"会短暂对不上
    （旧进程的收尾钩子会覆盖状态文件）。所以每一轮都用 [Find-LiveApiPort] 把两个候选
    （状态文件里的、以及本次要用的）都真打一次，谁回 200 就用谁，
    并把结果**固化到 `$Script:Port`**（后面所有探测都跟着它走）。

    同时要求**连续两次 200** 才算就绪：旧进程在退出前还能应答一两次，
    只认一次会把"即将消失的后端"当成"新后端起来了"（2026-09-22 实测：就是这么误判的）。
#>
function Wait-Api([int]$Seconds = 90) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    $round = 0
    $streak = 0
    while ((Get-Date) -lt $deadline) {
        $round++
        # ⚠️ $Script:ApiPort 是 Resolve-ApiPort 的**整个对象**（Port/Source/State），
        # 传给 [int]$PreferredPort 必须取 .Port —— 整个对象传过去会当场抛
        # "Cannot convert @{Port=…} to System.Int32"（2026-09-22 实测：起后端必炸）。
        $live = Find-LiveApiPort -ProjectRoot $ProjectRoot -PreferredPort $Script:ApiPort.Port
        if ($live -and $live.Health) {
            $streak++
            $Script:Port = $live.Port
            if ($streak -ge 2) { Trace "后端有响应（轮询第 $round 次，端口 $($live.Port)）"; return $live.Health }
        } else {
            $streak = 0
            # 每 20 轮（约 6 秒）报一次"为什么还没就绪"，便于下次排查（只在 -Trace 下出声）
            if ($round % 20 -eq 0) {
                $st = Read-ApiPortState -ProjectRoot $ProjectRoot
                Trace ("等待中：round=$round statePort=" + $(if ($st) { $st.Port } else { '(无)' }) + " preferred=$($Script:ApiPort.Port)")
            }
        }
        Start-Sleep -Milliseconds 300
    }
    return $null
}

function Do-Stop {
    # ⚠ **两个后端都要停**（切流期间尤其）：只停"当前选中的那个"会让旧的另一个继续占着端口，
    # 表现是"新后端起来了、健康检查却打到旧进程上"或干脆 bind 失败。
    $procs = Get-AnyBackendProcess
    if (-not $procs) { Write-Host '后端未运行。' -ForegroundColor Yellow; return }
    Trace "停止：找到 $(@($procs).Count) 个后端进程（Rust+Kotlin 一起停）"
    $procs | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }

    # ⚠️ **必须等它真的死透并放开端口**，否则 restart 会撞进一个空窗：
    # 旧进程已不再应答、新进程还没起来，中间的 2~5 秒里任何健康检查都失败 ——
    # 表现是"重启偶尔报『后端不健康』，但过几秒又好了"（2026-09-22 实测，restart 三次错两次）。
    # 这里等两件事：进程消失 + 端口释放。都是轮询，最多 20 秒。
    $deadline = (Get-Date).AddSeconds(20)
    while ((Get-Date) -lt $deadline -and @(Get-AnyBackendProcess).Count -gt 0) { Start-Sleep -Milliseconds 200 }
    Trace "停止：进程已消失（剩余 $(@(Get-AnyBackendProcess).Count)）"
    if ($Port -gt 0) {
        $deadline = (Get-Date).AddSeconds(20)
        while ((Get-Date) -lt $deadline -and (Test-TcpPort '127.0.0.1' $Port -TimeoutMs 200)) {
            Start-Sleep -Milliseconds 200
        }
        Trace "停止：端口 $Port 已释放"
    }

    Remove-Item $PidFile -Force -ErrorAction SilentlyContinue
    # 端口状态文件在进程被强杀时不会被后端的收尾钩子清掉（Stop-Process -Force = 没有优雅退出）。
    # 主动删一次：留着它只会让下一次启动去探一个空端口（虽然有"还在听才复用"兜着，但没意义）。
    Remove-ApiPortState -ProjectRoot $ProjectRoot
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

    $jdk = Get-Jdk
    $launcher = Join-Path $LogDir 'start-server.cmd'
    $lines = @(
        '@echo off',
        "cd /d `"$WorkDir`"",
        "set `"COMFYHUB_STORAGE=$StorageDir`"",
        "set `"COMFYHUB_PORT=$Port`"",
        "set `"JAVA_HOME=$jdk`"",
        "set `"PATH=$jdk\bin;%PATH%`"",
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

function Start-DetachedRust {
    <#
      启动 **Rust 后端**：与 [Start-Detached] 同一条纪律（WMI 脱离进程树 + SW_HIDE 不留黑框），
      只是把 `.bat` 换成 exe + 参数。

      参数与 Kotlin 那边**逐个对应**，好让两条路的行为一样：
        · `--db` / `--storage`：与 Kotlin 同一个库文件与产物目录；
        · `--port`：`0` 表示让后端自己挑（真实端口由**后端**写进 `.run\api.json`，见 `api_port_file.rs`）；
        · `COMFYHUB_PROJECT_ROOT`：显式给根目录 —— 端口文件必须落在项目根，
          绝不能靠"以子目录为 cwd 跑"时的兜底（pitfalls 里那条"搬迁工具把数据写到 server\.db"就是它）。
    #>
    param([string]$Exe, [string]$WorkDir, [string]$StdOut, [string]$StdErr, [string]$StorageDir, [int]$Port)

    # 监听地址：默认回环；`COMFYHUB_ALLOW_REMOTE`（App 的「允许手机 / 局域网设备连接」
    # 开关会带上它）时听 0.0.0.0 —— 否则真机连桌面永远超时（2026-09-28 用户 bug）。
    # 远程访问由后端的配对闸门把关（回环放行、局域网要配对令牌），这里只管"听哪个网卡"。
    $bindHost = if ($env:COMFYHUB_ALLOW_REMOTE -and $env:COMFYHUB_ALLOW_REMOTE.Trim() -match '^(?i:1|true|yes|on)$') {
        Write-Host '==> 允许局域网访问（COMFYHUB_ALLOW_REMOTE）：后端将监听所有网卡' -ForegroundColor DarkGray
        '0.0.0.0'
    } else {
        '127.0.0.1'
    }

    $launcher = Join-Path $LogDir 'start-server.cmd'
    $dbFile = Join-Path $ProjectRoot '.db\comfyhub.db'
    $lines = @(
        '@echo off',
        "cd /d `"$WorkDir`"",
        "set `"COMFYHUB_PROJECT_ROOT=$ProjectRoot`"",
        "set `"COMFYHUB_STORAGE=$StorageDir`"",
        "`"$Exe`" --db `"$dbFile`" --storage `"$StorageDir`" --host $bindHost --port $Port > `"$StdOut`" 2> `"$StdErr`""
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

    # 发布包自带的便携版优先（<根>\mysql），否则换台机器就找不到 mysqladmin.exe，
    # 于是 Test-MySqlAlive 永远返回 $false —— 库明明在跑却报"数据库不可用"。
    $packaged = Join-Path $ProjectRoot 'mysql\bin\mysqladmin.exe'
    if (Test-Path $packaged) { $Script:MysqlAdminExe = $packaged; return $packaged }

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

    # ⚠ P2（2026-09-23）起**不再**等 MySQL：后端用的是 SQLite 文件（`<根>\.db\comfyhub.db`），
    # 由后端自己按需创建 / 迁移（`Migrate.run`）。老 MySQL 只在 `migrate` 那一步当只读来源。
    # 以前这里"库没起来就抛"的闸门保留在 `migrate` 动作里。

    if (Get-AnyBackendProcess) {
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

    $storageDir = Join-Path $ProjectRoot 'storage'
    New-Item -ItemType Directory -Force -Path $storageDir | Out-Null

    # ---- 起哪一个后端（Rust / Kotlin，见 scripts\backend.ps1 的规则）------------
    if ($Script:BackendKind -eq 'rust') {
        Write-Host ("==> 后端实现: Rust（{0}）" -f $Script:BackendInfo.Note) -ForegroundColor Cyan
        $exe = $Script:BackendInfo.Exe
        if (-not (Test-Path $exe) -and -not $SkipBuild) {
            Write-Host '==> cargo build --release（第一次会比较久）' -ForegroundColor Cyan
            & cargo build --release --manifest-path (Join-Path $ProjectRoot 'server-rs\Cargo.toml')
            $exe = Resolve-RustExe -ProjectRoot $ProjectRoot
        }
        if (-not $exe -or -not (Test-Path $exe)) {
            throw @"
找不到 Rust 后端二进制（也不在发布包里）。
  构建：cargo build --release --manifest-path server-rs\Cargo.toml
  或者用旧后端：pwsh -File scripts\server.ps1 start -Backend kotlin
"@
        }
        Write-Host "==> 后台启动后端 (port $Port)  exe=$exe" -ForegroundColor Cyan
        # SQLite 不替我们建目录（`<根>\.db` 不存在时驱动的报错完全指不到这里）
        New-Item -ItemType Directory -Force -Path (Join-Path $ProjectRoot '.db') | Out-Null
        $how = Start-DetachedRust -Exe $exe -WorkDir $ProjectRoot -StdOut $OutLog -StdErr $ErrLog -StorageDir $storageDir -Port $Port
        Trace "启动方式=$how，开始等后端健康"
    } else {
        Write-Host ("==> 后端实现: Kotlin（{0}）" -f $Script:BackendInfo.Note) -ForegroundColor DarkGray
        # ⚠ P4 收尾（2026-09-26）：Kotlin 后端**已退役** —— 源码搬到 `legacy\server\`，
        # 发布包也不再装它。这条路只剩"灰度期回退"一个用途，而且要先自己把它构建出来。
        if (-not (Test-Path (Join-Path $ServerDir 'build.gradle.kts'))) {
            throw @"
Kotlin 后端已退役（P4 收尾）：legacy\server 里没有源码。
  现在默认就是 Rust 后端：pwsh -File scripts\comfyhub.ps1 up
  真要用旧后端：把 legacy\server 放回原位、pwsh -File scripts\server.ps1 build，再 -Backend kotlin。
"@
        }
        Write-Host "==> 使用 JDK: $(Get-Jdk)" -ForegroundColor DarkGray
        if (Test-Path $PackagedServerBat) {
            # 发布包布局：没有 Gradle 源码也没有 gradle 可跑，别去 attempt 构建
            $bat = $PackagedServerBat
            Write-Host '==> 发布包布局：直接用自带的 server\bin，跳过 Gradle 构建' -ForegroundColor DarkGray
        } else {
            if ($SkipBuild) {
                Write-Host '==> 跳过构建（-SkipBuild）' -ForegroundColor DarkGray
            } else {
                Write-Host '==> gradle installDist' -ForegroundColor Cyan
                Invoke-Gradle @('installDist')
            }
            $bat = $GradleServerBat
        }
        Trace '构建阶段结束（SkipBuild 则跳过）'

        if (-not (Test-Path $bat)) {
            throw "找不到后端启动脚本: $bat`n（开发布局去掉 -SkipBuild 重新构建；发布包布局请重新执行 scripts\pack-release.ps1）"
        }

        Write-Host "==> 后台启动后端 (port $Port)" -ForegroundColor Cyan
        $how = Start-Detached -BatchFile $bat -WorkDir $ServerDir -StdOut $OutLog -StdErr $ErrLog -StorageDir $storageDir -Port $Port
        Trace "启动方式=$how，开始等后端健康"
    }

    # `-Port 0` 时脚本事先不知道后端会绑到哪：先给它一点时间把真实端口写进
    # `.run\api.json`（正常 4~10 秒），再开始探健康 —— 否则头几轮探测根本没有候选端口，
    # 白白多等（实测能多等几十秒）。
    if ($Port -le 0) {
        $state = Wait-ApiPortState -ProjectRoot $ProjectRoot -Seconds 30
        if ($state) {
            $Script:Port = $state.Port
            if ($state.FellBack -and $state.RequestedPort -gt 0) {
                Write-Host ("==> 注意：候选端口 {0} 绑不上，后端已改用随机端口 {1}" -f $state.RequestedPort, $state.Port) -ForegroundColor Yellow
            }
        } else {
            Trace '后端 30 秒内没写出端口状态文件，继续按候选端口探'
        }
    }

    $h = Wait-Api
    if ($h -and $h.database -eq 'ok') {
        # PID 在进程起来之后按命令行去找（静默启动那条路拿不到新进程的 PID）
        $proc = Get-ServerProcess
        if ($proc) { Set-Content -Path $PidFile -Value (($proc.ProcessId | Sort-Object) -join ',') }
        if ($Port -le 0) { Write-Host ("后端实际端口 {0}（.run\api.json）" -f $Script:Port) -ForegroundColor DarkGray }
        Write-Host "后端就绪: http://127.0.0.1:$Script:Port  (db=$($h.database))" -ForegroundColor Green
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
    # 库是 SQLite 文件（P2）：直接看文件在不在 + 大小，比"进程活没活"更贴近事实
    $dbFile = Join-Path $ProjectRoot '.db\comfyhub.db'
    if (Test-Path $dbFile) {
        $sizeMb = [math]::Round((Get-Item $dbFile).Length / 1MB, 2)
        Write-Host "数据库:    SQLite $dbFile（$sizeMb MB）" -ForegroundColor Green
    } else {
        Write-Host "数据库:    SQLite 还没建（$dbFile；后端首次启动会建）" -ForegroundColor Yellow
    }
    # 老 MySQL 只在搬迁时有用；它在跑就顺手报一句，不在跑也**不算异常**
    if (Test-MySqlAlive) {
        Write-Host "旧库:      MySQL 运行中 127.0.0.1:3307（只有 migrate 会读它）" -ForegroundColor DarkGray
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
        Write-Host "API 地址:  http://127.0.0.1:$Port"
        Write-Host "存储目录:  $($h.storageDir)"
    } elseif ($h) {
        Write-Host "健康检查:  异常  db=$($h.database)" -ForegroundColor Red
    } else {
        # 端口自动时先把真实端口读出来再报，否则用户只会看到一句"无法连接"，
        # 而不知道后端其实正跑在哪个端口上
        $state = Read-ApiPortState -ProjectRoot $ProjectRoot
        if ($state -and $state.Port -gt 0) {
            Write-Host ("端口记录:  {0}（.run\api.json{1}）" -f $state.Port, $(if ($state.FellBack) { '，候选端口被占已回落' } else { '' }))
        }
        Write-Host '健康检查:  无法连接' -ForegroundColor Red
    }
}

switch ($Action) {
    'build'  { Write-Host "==> JDK: $(Get-Jdk)" -ForegroundColor DarkGray; Invoke-Gradle @('build'); Write-Host '构建完成' -ForegroundColor Green }
    'test'   { Write-Host "==> JDK: $(Get-Jdk)" -ForegroundColor DarkGray; Invoke-Gradle @('test'); Write-Host '测试完成' -ForegroundColor Green }
    'fatjar' { Write-Host "==> JDK: $(Get-Jdk)" -ForegroundColor DarkGray; Invoke-Gradle @('fatJar'); Get-ChildItem (Join-Path $ServerDir 'build\libs') | Select-Object Name, Length }
    'migrate' {
        # 一次性数据搬迁（P2）：老 MySQL（只读来源）→ SQLite 目标库。
        # 这一步是**唯一**还需要 MySQL 的地方；来源地址走 COMFYHUB_MIGRATE_SOURCE，
        # 目标库走 COMFYHUB_JDBC_URL（缺省 <根>\.db\comfyhub.db）。
        Write-Host '==> 一次性数据搬迁：MySQL → SQLite（旧 Kotlin 后端的能力；已退役，源码在 legacy\\server）' -ForegroundColor Cyan
        if (-not (Ensure-MySql)) {
            throw "老库（MySQL）不可用，没法搬迁。先执行: pwsh -File scripts\mysql.ps1 start $($DataDirArgs -join ' ')".TrimEnd()
        }
        if (-not $env:COMFYHUB_MIGRATE_SOURCE) {
            $env:COMFYHUB_MIGRATE_SOURCE = 'jdbc:mysql://127.0.0.1:3307/comfy_hub?useUnicode=true&characterEncoding=UTF-8&allowPublicKeyRetrieval=true&useSSL=false'
        }
        # ⚠ 必须显式给出项目根 / storage：Gradle 是以 `server\` 为工作目录跑的，
        # 后端 Config 的兜底会把 `<server>\storage` 的父目录当成项目根 —— 那样搬迁会
        # 写进 `server\.db\comfyhub.db`（看上去"成功了"，其实新库还是空的，2026-09-23 实测踩到）。
        $env:COMFYHUB_PROJECT_ROOT = $ProjectRoot
        $env:COMFYHUB_STORAGE = Join-Path $ProjectRoot 'storage'
        # 控制台代码页钉成 UTF-8：Java 是按 UTF-8 输出的（file.encoding=UTF-8），
        # 不管的话中文报告在中文系统上会渲染成乱码。
        try { [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false) } catch { }
        Write-Host "==> 来源: $env:COMFYHUB_MIGRATE_SOURCE" -ForegroundColor DarkGray
        Write-Host "==> 目标: $(if ($env:COMFYHUB_JDBC_URL) { $env:COMFYHUB_JDBC_URL } else { (Join-Path $ProjectRoot '.db\comfyhub.db') })" -ForegroundColor DarkGray
        # 不给 -q：搬迁报告（逐表行数 + 核对结果）就是这一步的产出，不能被安静掉
        Invoke-Gradle @('migrateData')
        Write-Host '搬迁完成（逐表行数与行哈希已核对）' -ForegroundColor Green
    }
    'start'  { Do-Start }
    'stop'   { Do-Stop }
    'restart'{ Do-Stop; Start-Sleep 1; Do-Start }
    'status' { Do-Status }
    'logs'   { if (Test-Path $OutLog) { Get-Content $OutLog -Tail 80 }; if (Test-Path $ErrLog) { Write-Host '--- stderr ---'; Get-Content $ErrLog -Tail 40 } }
    'run' {
        Write-Host "==> 使用 JDK: $(Get-Jdk)" -ForegroundColor DarkGray
        Write-Host "==> 前台运行（Ctrl+C 停止），监听 $Port" -ForegroundColor Cyan
        Push-Location $ServerDir
        try { & gradle -p $ServerDir --console=plain run } finally { Pop-Location }
    }
}
