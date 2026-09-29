<#
.SYNOPSIS
    ComfyHub 整套服务的统一生命周期入口：后端（Rust，+ 可选桌面 App）。

.DESCRIPTION
    P2（2026-09-23）起数据库是 **SQLite**（驱动自带引擎，P4 起后端是单个 Rust exe），
    MySQL 已在 2026-09-29 彻底退役：`mysql.ps1` 连同"老库一次性搬迁"这条路一起删掉了。

    这个脚本负责把**后端 + 桌面 App**当成一个整体来起停和体检，保证"后端健康 = 数据库也健康"。
    （SQLite 是单文件嵌入式库，没有独立进程，所以"数据库有没有起"这个问题本身消失了。）

.EXAMPLE
    pwsh -File scripts\comfyhub.ps1 up            # 起后端
    pwsh -File scripts\comfyhub.ps1 up -WithApp   # 再顺带把桌面 App 拉起来
    pwsh -File scripts\comfyhub.ps1 status        # 后端 / App 状态一览
    pwsh -File scripts\comfyhub.ps1 down          # 全停（App → 后端）
    pwsh -File scripts\comfyhub.ps1 release       # 只停后端，不动 App（App 退出时调它）
    pwsh -File scripts\comfyhub.ps1 watch -OwnerPid <pid>   # 给"已经在跑服务"的 App 补挂退出守护
    pwsh -File scripts\comfyhub.ps1 restart
    pwsh -File scripts\comfyhub.ps1 logs
#>
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateSet('up', 'down', 'status', 'restart', 'logs', 'doctor', 'release', 'watch', 'unwatch')]
    [string]$Action = 'status',

    [switch]$WithApp,
    [switch]$SkipBuild,

    # 后端实现：auto（默认：有 Rust 二进制就用 Rust）/ rust / kotlin。
    # 与 server.ps1 的 -Backend 同义（也认环境变量 COMFYHUB_BACKEND），并且**透传**下去 ——
    # 不透传就会出现"up 说用 Rust、server.ps1 自己去起 Kotlin"这种两端不一致。
    [ValidateSet('auto', 'rust', 'kotlin')]
    [string]$Backend = $env:COMFYHUB_BACKEND,

    # 调用方（桌面 App）的 PID。传了它 = “这些服务是 App 拉起来的”：
    # 服务起来后会挂一个守护进程盯着这个 PID，App 一退出就把**这次启动过的**服务停掉。
    # 手工敲 `up` 不带它时，服务照旧常驻（以前的行为）。见 scripts\watch-owner.ps1
    # `watch` 动作也用它：服务本来就跑着（App 探到健康就直接返回、没跑 up）时补挂守护进程。
    [int]$OwnerPid = 0,

    # API 端口。**默认 0 = 交给操作系统自动挑**（用户 2026-09-22）。
    # 解析顺序（scripts\api-port.ps1）：-Port > COMFYHUB_PORT > .run\api.json > 0。
    # 这里必须**透传给 server.ps1** —— 过去两个脚本各写各的 8080、互不知道，
    # 于是"后端在跑、脚本却说没起来"。
    [int]$Port = 0
)

$ErrorActionPreference = 'Continue'
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$Scripts     = $PSScriptRoot
$RunDir      = Join-Path $ProjectRoot '.run'

# 「关 App 自动停服务」的状态文件：里面记着当前负责的 watchdog PID
$WatchState = Join-Path $RunDir 'watch-owner.json'

# 这次 up 到底启动了哪个服务（只停自己起的，见 watch-owner.ps1）
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

# MCP 运行时（可选能力）：便携版 Python + comfy-cli + comfy-mcp，见 scripts\mcp-runtime.ps1。
# 它是**可选**的 —— 装不上不拦 up，只让 AI 少一批 ComfyUI 工具（与 Java 那种"必需"完全不同）。
$McpRuntimeHelper = Join-Path $Scripts 'mcp-runtime.ps1'
if (Test-Path $McpRuntimeHelper) { . $McpRuntimeHelper }

# API 端口解析的唯一入口（用户 2026-09-22："不再显式指定端口，启动时自动挑一个"）。
# 见 scripts\api-port.ps1：优先级 = -Port > COMFYHUB_PORT > .run\api.json > 0（自动）。
$ApiPortHelper = Join-Path $Scripts 'api-port.ps1'
if (Test-Path $ApiPortHelper) { . $ApiPortHelper }

# 这一段解析出来的端口**只是"候选"**：`-Port 0` 时后端会自己挑一个并写进
# `.run\api.json`，真实端口一律以那份文件为准（[Get-ApiPortNow] 现读）。
$Script:RequestedPort = $Port
$Script:ApiPort = (Resolve-ApiPort -ProjectRoot $ProjectRoot -Explicit $Port).Port

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
      **恒定 ~2 秒**（Invoke-RestMethod / 裸 TcpClient 都一样；开着
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

# ---------------------------------------------------------------------------
#  SQLite（P2 起的默认库；MySQL 已于 2026-09-29 退役）
# ---------------------------------------------------------------------------
$Script:SqliteDir = Join-Path $ProjectRoot '.db'
$Script:SqliteDb = Join-Path $Script:SqliteDir 'comfyhub.db'

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
#  SQLite（P2 起的默认库）
# ---------------------------------------------------------------------------
$Script:SqliteDir = Join-Path $ProjectRoot '.db'
$Script:SqliteDb = Join-Path $Script:SqliteDir 'comfyhub.db'


function Get-ApiProcess {
    # 认**两个后端**（P4 收尾后默认是 Rust；灰度期可能还有旧的 Kotlin 在跑）
    $helper = Join-Path $Scripts 'backend.ps1'
    if (Test-Path $helper) {
        . $helper
        return Get-AnyBackendProcess
    }
    Get-CimInstance Win32_Process -Filter "Name='java.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -and $_.CommandLine -like '*com.comfyhub.ApplicationKt*' }
}

<#
    此刻后端**实际**在哪个端口上。

    真源永远是 `<根>\.run\api.json`（后端绑定成功后写的）。解析出来的 `$Script:ApiPort`
    只是候选（`-Port 0` 时它可能是 0），所以这里每次都现读文件：

      · 文件里的端口**真的有人在听** → 用它；
      · 没人听 / 没有文件 → 退回落候选端口（可能是显式指定的那个，也可能为 0）。
#>
function Get-ApiPortNow {
    $state = Read-ApiPortState -ProjectRoot $ProjectRoot
    if ($state -and $state.Port -gt 0 -and (Test-ApiPortListening -Port $state.Port)) {
        return $state.Port
    }
    return $Script:ApiPort
}

function Get-ApiHealth {
    # 端口没开就直接返回，不要让 Invoke-RestMethod 去等那 2 秒。
    # **必须走 [Find-LiveApiPort]**：`-Port 0` 时 `$Script:ApiPort` 是 0，
    # 而 restart 期间状态文件与"真的在听"的那个还可能短暂不一致。
    $live = Find-LiveApiPort -ProjectRoot $ProjectRoot -PreferredPort $Script:ApiPort
    if (-not $live) { return $null }
    $Script:ApiPort = $live.Port
    return $live.Health
}

function Get-AppProcess { Get-Process -Name viewer -ErrorAction SilentlyContinue }

<#
    等后端真的把端口释放掉（`restart` 里紧跟着就要重新绑同一个端口）。

    判据要**两个都过**，因为它们先后失效：
      ① 没人监听了（旧进程已经不 accept 了）；
      ② 现在**真的能 bind**（socket 完全放掉；`TIME_WAIT` 之类的尾巴散尽）。

    只判 ① 会出现：旧进程刚被杀、还没放掉 socket，新后端一上来 bind 失败
    → 回落成随机端口，白等十几秒（2026-09-22 实测）。
#>
function Wait-ApiPortReleased {
    param([int]$Port, [int]$Seconds = 30)
    if ($Port -le 0) { return $true }
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $deadline) {
        if (-not (Test-ApiPortListening -Port $Port) -and (Test-ApiPortBindable -Port $Port)) { return $true }
        Start-Sleep -Milliseconds 200
    }
    return $false
}

# ---------------------------------------------------------------------------
#  状态
# ---------------------------------------------------------------------------

function Get-StackStatus {
    $apiProc = Get-ApiProcess
    $health = Get-ApiHealth
    $app = Get-AppProcess

    return [pscustomobject]@{
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

    # 数据库：P2 起是 SQLite 文件（后端自己建），没有独立进程
    if (Test-Path $Script:SqliteDb) {
        $sizeMb = [math]::Round((Get-Item $Script:SqliteDb).Length / 1MB, 2)
        Say (Format-Row '数据库' 'SQLite' "$Script:SqliteDb  ($sizeMb MB)") 'Green'
    } else {
        Say (Format-Row '数据库' '待建' "$Script:SqliteDb（后端首次启动会建）") 'Yellow'
    }

    # 后端（报的是端到端健康，包含数据库连通性）
    # 实现（Rust / Kotlin）由 `scripts\backend.ps1` 的**同一条规则**判定 —— 与 server.ps1 共读一份，
    # 免得"doctor 说 Kotlin 在跑、server.ps1 去停 Rust"这种自相矛盾（切流期间两个都可能在场）。
    $backendHelper = Join-Path $Scripts 'backend.ps1'
    if (Test-Path $backendHelper) {
        . $backendHelper
        $backend = Resolve-Backend -Preference $Backend -ProjectRoot $ProjectRoot
        $running = Get-AnyBackendProcess
        $runningKind = if (@(Get-RustServerProcess).Count -gt 0) { 'Rust' }
            elseif (@(Get-KotlinServerProcess).Count -gt 0) { 'Kotlin' }
            else { 'none' }
        $implNote = "实现 {0}（{1}）" -f $backend.Kind, $backend.Note
        if ($runningKind -ne 'none') { $implNote += "；正在跑的是 $runningKind" }
        Say (Format-Row '后端实现' $backend.Kind $implNote) $(if ($runningKind -eq 'none' -or $runningKind -eq $backend.Kind) { 'Green' } else { 'Yellow' })
        if ($backend.Kind -eq 'rust') {
            $fresh = Get-RustBuildFreshness -Exe $backend.Exe -ProjectRoot $ProjectRoot
            if ($fresh) {
                if ($fresh.Stale) {
                    Say (Format-Row '后端构建' '已过期' ("$($backend.Exe) 早于源码 $($fresh.NewestSource.FullName) —— 重新构建：cargo build --release --manifest-path server-rs\Cargo.toml")) 'Yellow'
                } else {
                    Say (Format-Row '后端构建' '最新' ("$($backend.Exe)  $($fresh.ExeTime)") ) 'DarkGray'
                }
            }
        }
    }
    if ($s.ApiHealth -and $s.ApiHealth.database -eq 'ok') {
        # 端口从状态文件现读：`-Port 0` 时 `$ApiPort` 是 0，报它等于报了个假地址
        $livePort = Get-ApiPortNow
        Say (Format-Row '后端' '健康' "http://127.0.0.1:$livePort  v$($s.ApiHealth.version)  db=ok") 'Green'
        Say (Format-Row '' '' "存储目录 $($s.ApiHealth.storageDir)") 'DarkGray'
        $state = Read-ApiPortState -ProjectRoot $ProjectRoot
        if ($state -and $state.FellBack -and $state.RequestedPort -gt 0) {
            Say (Format-Row '' '' "注意：候选端口 $($state.RequestedPort) 绑不上（被占或落在 Windows 保留段），已用随机端口 $($state.Port)") 'Yellow'
        }
    } elseif ($s.ApiProcess -gt 0) {
        $db = if ($s.ApiHealth) { $s.ApiHealth.database } else { '无响应' }
        Say (Format-Row '后端' '不健康' "进程在跑但 db=$db —— 看 .run\server.out.log，多半是库文件不可写/被占") 'Red'
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
        & pwsh -NoProfile -File (Join-Path $Scripts 'server.ps1') stop -Port $Script:ApiPort
        Start-Sleep 1
    }

    $backendLabel = if ($Backend -and $Backend -ne 'auto') { $Backend } else { 'auto（有 Rust 二进制就用 Rust）' }
    Say "  [1/2] 启动后端（-Backend $backendLabel）…" 'Cyan'
    # ⚠️ 端口必须**透传**给 server.ps1：过去两个脚本各写各的 8080、互不知道，
    # 于是"后端在跑、脚本却说没起来"。`-Port 0` 表示"自动"，由后端挑好写进 .run\api.json。
    # `-Backend` 要**透传**给 server.ps1：不透传就会出现"up 说用 Rust、server.ps1 自己起 Kotlin"
    $backendArgs = if ($Backend) { @('-Backend', $Backend) } else { @() }
    $apiArgs = @('-Port', "$Script:ApiPort") + $backendArgs
    if ($SkipBuild) {
        & pwsh -NoProfile -File (Join-Path $Scripts 'server.ps1') start -SkipBuild @apiArgs
    } else {
        & pwsh -NoProfile -File (Join-Path $Scripts 'server.ps1') start @apiArgs
    }
    Trace '后端: server.ps1 start 已返回'

    $health = Get-ApiHealth
    if ($health -and $health.database -eq 'ok') {
        $Script:StartedApi = $true
        Trace '后端: 健康'
        return $true
    }

    # 一次探不到不等于失败：`server.ps1 start` 返回时后端刚绑上端口，第一次受管请求
    # 可能还没铺好（MCP 子进程 / 迁移都在启动尾部跑）。用户要的是"起来就行"，
    # 这里给它 30 秒的确认窗口 —— 而不是让他看到一句"健康检查未通过"然后自己再试一次。
    Trace '后端: 首次探测未通过，重试 30 秒'
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 500
        $health = Get-ApiHealth
        if ($health -and $health.database -eq 'ok') {
            $Script:StartedApi = $true
            Trace '后端: 健康（重试后）'
            return $true
        }
    }

    # 复用的端口仍然绑不上（旧进程还在、或那个端口落在 Windows 保留段里）：
    # **不再空手收场**，把 `-Port 0` 明确给一次、让后端自己挑一个随机端口再来一遍。
    # 这正是用户定的兜底规则："失败了回到随机端口"。
    if ($Script:ApiPort -gt 0) {
        Say "  端口 $Script:ApiPort 起不来（旧进程没退干净 / 端口被保留）——改用随机端口重试一次…" 'Yellow'
        $script:ApiPort = 0
        $retryArgs = @('-Port', '0') + $backendArgs
        if ($SkipBuild) {
            & pwsh -NoProfile -File (Join-Path $Scripts 'server.ps1') start -SkipBuild @retryArgs
        } else {
            & pwsh -NoProfile -File (Join-Path $Scripts 'server.ps1') start @retryArgs
        }
        $health = Get-ApiHealth
        if ($health -and $health.database -eq 'ok') {
            $Script:StartedApi = $true
            Trace '后端: 健康（随机端口重试后）'
            return $true
        }
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

function Get-AppDartPayload {
    <#
      这个 exe 里"装着 Dart 代码"的那份产物：
        · Release / 发布包 → data\app.so（AOT）
        · debug            → data\flutter_assets\kernel_blob.bin（JIT 内核）
      找不到就返回 $null（判断不了新旧，别乱报警 —— 比如原生 exe 的时间戳
      除非动过 windows\ 否则一直很旧，拿它比会永远报"过期"）。
    #>
    param([System.IO.FileInfo]$Exe)
    if (-not $Exe) { return $null }
    foreach ($rel in @('data\app.so', 'data\flutter_assets\kernel_blob.bin')) {
        $p = Join-Path $Exe.DirectoryName $rel
        if (Test-Path -LiteralPath $p) { return (Get-Item -LiteralPath $p) }
    }
    return $null
}

function Get-AppBuildStaleness {
    <#
      比较 App 的 Dart 产物与源码，判断"跑的到底是新代码还是旧代码"。

      为什么要查这个：改完 lib\ 只跑 flutter test / analyze 是**看不见界面**的，
      而 App 又常常是直接双击/Start-Process 拉起旧产物 —— 于是用户看到的还是旧界面，
      反馈回来就是"明明修了却还是老样子"。2026-09-17 的"视频没有预览图"就是这么丢的：
      源码 20:11 改好，跑起来的却是 18:50 编的内核。
      返回 $null（判断不了）或 @{ Payload; Source; Newer }。
    #>
    param([System.IO.FileInfo]$Exe)
    $payload = Get-AppDartPayload -Exe $Exe
    if (-not $payload) { return $null }

    $items = @()
    $lib = Join-Path $ProjectRoot 'lib'
    if (Test-Path -LiteralPath $lib) {
        $items += Get-ChildItem -LiteralPath $lib -Recurse -Filter '*.dart' -File -ErrorAction SilentlyContinue
    }
    # 依赖或资源变了同样得重编，所以 pubspec 两个文件一起比
    foreach ($f in @('pubspec.yaml', 'pubspec.lock')) {
        $p = Join-Path $ProjectRoot $f
        if (Test-Path -LiteralPath $p) { $items += Get-Item -LiteralPath $p }
    }
    if ($items.Count -eq 0) { return $null }   # 发布包/别人机器上没有源码，静默跳过

    return [pscustomobject]@{
        Payload = $payload
        Source  = ($items | Sort-Object LastWriteTime -Descending | Select-Object -First 1)
        Newer   = (($items | Sort-Object LastWriteTime -Descending | Select-Object -First 1).LastWriteTime -gt $payload.LastWriteTime)
    }
}

function Format-AppFreshness {
    param([System.IO.FileInfo]$Exe)
    if (-not $Exe) { return '没有产物' }
    $s = Get-AppBuildStaleness -Exe $Exe
    if (-not $s) { return '无法判断（找不到 app.so / kernel_blob.bin）' }
    $built = $s.Payload.LastWriteTime.ToString('MM-dd HH:mm')
    if ($s.Newer) {
        return "旧构建：$($s.Payload.Name) 编于 $built，源码已改到 $($s.Source.LastWriteTime.ToString('MM-dd HH:mm'))"
    }
    return "最新（$($s.Payload.Name) 编于 $built）"
}

function Show-AppStaleWarning {
    param([string]$Label, $Stale)
    Say "  ⚠ $Label 是旧构建：$($Stale.Payload.Name) 编于 $($Stale.Payload.LastWriteTime.ToString('MM-dd HH:mm'))，" 'Yellow'
    Say "    而源码（$($Stale.Source.Name)）已改到 $($Stale.Source.LastWriteTime.ToString('MM-dd HH:mm')) —— 界面上看到的是旧代码。" 'Yellow'
    Say '    重新构建：pwsh -File scripts\autorun-app.ps1（Release）或 pwsh -File scripts\dev-app.ps1（debug 热重载）' 'DarkGray'
}

function Get-ServerBuildStaleness {
    <#
      后端二进制的"旧构建"体检（P4 收尾：从 Kotlin jar 换成 **Rust exe** —— 现在默认就是它）。

      为什么要查：dev-app.ps1 / up -SkipBuild 都是"二进制存在就直接用"，
      于是"昨晚改了 server-rs\、今早起的是昨天下班前编的 exe"完全无声。
      2026-09-23 现场（当时还是 Kotlin jar）：P0 的补挂 shared / comfy_env_report / 提交前预检
      全都没在跑，AI 对着一台没挂共享模型的 ComfyUI 烧掉半小时 —— 症状看着像功能 bug，
      其实是旧构建。**这个体检与 doctor 里那条是同一个判据。**

      返回 $null（发布包没有 server-rs\target、或没有源码 → 判断不了，静默跳过）或
      @{ Jar; Source; Newer }。按 LastWriteTime 比（git 换分支会把源码 mtime 刷新，
      可能误报一次"旧构建"—— 重新 cargo build 即消）。
    #>
    $jar = Join-Path $ProjectRoot 'server-rs\target\release\comfyhub-server-rs.exe'
    if (-not (Test-Path $jar)) { return $null }

    $items = @()
    $src = Join-Path $ProjectRoot 'server-rs\src'
    if (Test-Path $src) {
        $items += Get-ChildItem $src -Recurse -File -ErrorAction SilentlyContinue
    }
    $cargoToml = Join-Path $ProjectRoot 'server-rs\Cargo.toml'
    if (Test-Path $cargoToml) { $items += Get-Item $cargoToml }
    if ($items.Count -eq 0) { return $null }   # 没有源码可比（发布包），静默跳过

    $newest = $items | Sort-Object LastWriteTime -Descending | Select-Object -First 1
    return [pscustomobject]@{
        Jar    = (Get-Item $jar)
        Source = $newest
        Newer  = ($newest.LastWriteTime -gt (Get-Item $jar).LastWriteTime)
    }
}

function Get-RunningServerProcess {
    <# 正在跑的后端进程（Rust 或旧 Kotlin 都认；拿不到返回 $null） #>
    $helper = Join-Path $Scripts 'backend.ps1'
    if (Test-Path $helper) {
        . $helper
        return (Get-AnyBackendProcess | Sort-Object CreationDate | Select-Object -First 1)
    }
    Get-CimInstance Win32_Process -Filter "Name='java.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -match 'comfy-hub-server' } |
        Sort-Object CreationDate | Select-Object -First 1
}

function Show-ServerStaleWarning {
    param($Stale, $Running)
    Say "  ⚠ 后端二进制是旧构建：$($Stale.Jar.Name) 编于 $($Stale.Jar.LastWriteTime.ToString('MM-dd HH:mm'))，" 'Yellow'
    Say "    而后端源码（$($Stale.Source.Name)）已改到 $($Stale.Source.LastWriteTime.ToString('MM-dd HH:mm')) —— 后端跑的是旧代码。" 'Yellow'
    Say '    重建 + 重启：cargo build --release --manifest-path server-rs\Cargo.toml 然后 pwsh -File scripts\comfyhub.ps1 restart' 'DarkGray'
    if ($Running) {
        Say "    ⚠ 另外正在跑的后端进程启动于 $($Running.CreationDate.ToString('MM-dd HH:mm'))——即使现在重建 jar，它也吃不到，必须重启。" 'Yellow'
    }
}

function Get-RunningAppExe {
    <# 正在跑的那个 viewer.exe（拿不到路径就返回 $null，不抛错） #>
    $running = Get-AppProcess | Select-Object -First 1
    if (-not $running) { return $null }
    try { $p = $running.Path } catch { return $null }
    if (-not $p -or -not (Test-Path -LiteralPath $p)) { return $null }
    return (Get-Item -LiteralPath $p)
}

function Start-AppPart {
    $exe = Resolve-AppExe
    if (-not $exe) {
        Say '  找不到 viewer.exe，先执行: pwsh -File scripts\autorun-app.ps1（或打包: pwsh -File scripts\pack-release.ps1）' 'Yellow'
        return $false
    }

    # 已经在跑的这个也可能是旧构建 —— 那正是"改了却看不到效果"的另一种形态，
    # 所以这里也查一遍（不然一句"App 已经在跑了"就把线索盖掉了）。
    $runningExe = Get-RunningAppExe
    if ($runningExe) {
        Say '  App 已经在跑了。' 'DarkGray'
        $rs = Get-AppBuildStaleness -Exe $runningExe
        if ($rs -and $rs.Newer) { Show-AppStaleWarning -Label '正在运行的 App' -Stale $rs }
        return $true
    }

    # 旧构建 = 界面还是老样子。这时候把 App 拉起来只会让人以为"改了没用"，
    # 所以先明说，再给重编的命令（照旧启动，不拦着 —— 也许他就是想看旧版对照）。
    $stale = Get-AppBuildStaleness -Exe $exe
    if ($stale -and $stale.Newer) { Show-AppStaleWarning -Label '选中的 App' -Stale $stale }

    Say '  [2/2] 启动桌面 App…' 'Cyan'
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

      -StopApi 显式指定要不要跟着停后端；不传就沿用 up 的判定
      （这次真正启动过的那个，即 $Script:StartedApi），
      所以 up 的行为和以前完全一样。`watch` 动作会用显式参数覆盖它。
    #>
    param(
        [int]$Owner = 0,
        [bool]$StopApi = $Script:StartedApi
    )
    $watcher = Join-Path $Scripts 'watch-owner.ps1'
    if (-not (Test-Path $watcher)) {
        Say '  提示: 找不到 scripts\watch-owner.ps1，关闭 App 时不会自动停服务。' 'Yellow'
        return $false
    }
    if (-not $StopApi) {
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

    # Java 交给 runtime-deps.ps1 自己解析（发布包 <根>\jre → PATH → 本机常见 JDK 位置）
    Show-RuntimeFailureHints -ProjectRoot $ProjectRoot | Out-Null
}

function Do-Up {
    $chain = if ($WithApp) { '数据库 → 后端 → App' } else { '数据库 → 后端' }
    Say ''
    Say "  ComfyHub 启动中（$chain）" 'White'
    Trace 'Do-Up 开始'

    # 不带 -OwnerPid 的手工 up = 用户自己接管，撤掉之前 App 留下的守护
    if ($OwnerPid -le 0) { Clear-OwnerWatchdog }

    # Java：**只有跑旧 Kotlin 后端时才需要**（P4 收尾起默认是 Rust 单文件二进制，不需要 JVM）。
    # 所以这里先问后端选择器：Rust 就完全跳过（连"检查一下 java 在不在"都不做）——
    # 否则没装 JDK 的机器会在 up 时白下一个 40MB 的便携 JRE（谁也不用它）。
    $backendKind = 'rust'
    $backendHelper = Join-Path $Scripts 'backend.ps1'
    if (Test-Path $backendHelper) {
        . $backendHelper
        $backendKind = (Resolve-Backend -Preference $Backend -ProjectRoot $ProjectRoot).Kind
    }
    if ($backendKind -eq 'kotlin') {
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
    }

    # MCP 运行时（便携版 Python + comfy-cli + comfy-mcp）：AI 的 ComfyUI 工具靠它。
    # **这是可选能力**：已就绪时这里只是几次文件检查（不联网）；没装才会去下载（约 300MB）。
    # 失败**不拦 up**（与 Java 完全不同）—— 装不上只是 AI 少一批工具，App 照常能用。
    # COMFYHUB_NO_MCP=1 可以整个关掉；COMFYHUB_NO_DOWNLOAD=1 时也不自动装（只提示）。
    if ((Get-Command Ensure-McpRuntime -ErrorAction SilentlyContinue) -and -not $env:COMFYHUB_NO_MCP) {
        if ($env:COMFYHUB_NO_DOWNLOAD) {
            $mcpStatus = Get-McpRuntimeStatus -ProjectRoot $ProjectRoot
            if (-not $mcpStatus.Ok) {
                Say '  提示: MCP 运行时还没装配（COMFYHUB_NO_DOWNLOAD=1，已跳过自动安装）' 'Yellow'
                Say '        要装: pwsh -File scripts\mcp-runtime.ps1 install' 'DarkGray'
            }
        } else {
            try {
                Ensure-McpRuntime -ProjectRoot $ProjectRoot -Quiet | Out-Null
            } catch {
                Say "  提示: MCP 运行时自动装配失败（$($_.Exception.Message)）" 'Yellow'
            }
        }
    }

    # 数据库：SQLite 单文件，没有独立进程，**由后端自己建**（`db::open_or_create`）
    Say "  数据库: SQLite $Script:SqliteDb" 'DarkGray'

    if (-not (Start-ApiPart))   { Show-Status | Out-Null; Show-RuntimeHintsIfAvailable; exit 1 }
    if ($WithApp) { Start-AppPart | Out-Null }

    if ($OwnerPid -gt 0) {
        if ($Script:StartedApi) {
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
    Say '  ComfyHub 停止中（App → 后端）' 'White'

    # 手工停了就别让 watchdog 再补一刀
    Clear-OwnerWatchdog

    if (Get-AppProcess) {
        Say '  [1/2] 关闭桌面 App…' 'Cyan'
        Get-AppProcess | Stop-Process -Force -ErrorAction SilentlyContinue
    } else { Say '  桌面 App 未运行。' 'DarkGray' }

    Say '  [2/2] 停止后端…' 'Cyan'
    & pwsh -NoProfile -File (Join-Path $Scripts 'server.ps1') stop -Port $Script:ApiPort

    # 必须等后端真的退出，否则 App 一关它就被半路掐断
    $deadline = (Get-Date).AddSeconds(20)
    while ((Get-Date) -lt $deadline -and @(Get-ApiProcess).Count -gt 0) { Start-Sleep -Milliseconds 400 }

    Say ''
    Show-Status | Out-Null
}

function Do-Release {
    <#
      App 退出时**自己**调用的：只停后端，绝不碰 App 进程。
      和 Do-Down 的区别只有一条，但很关键 —— Do-Down 会按进程名 `viewer` 杀 App，
      在"App 自己还活着、正等我们停完服务"的时候调用它等于自杀。
    #>
    Say ''
    Say '  ComfyHub 停止中（后端，App 不动）' 'White'

    # 手工/App 主动停了就别让 watchdog 再补一刀（它可能和我们同时动手）
    Clear-OwnerWatchdog

    Say '  [1/1] 停止后端…' 'Cyan'
    & pwsh -NoProfile -File (Join-Path $Scripts 'server.ps1') stop -Port $Script:ApiPort

    # 必须等后端真的退出
    $deadline = (Get-Date).AddSeconds(20)
    while ((Get-Date) -lt $deadline -and @(Get-ApiProcess).Count -gt 0) { Start-Sleep -Milliseconds 400 }

    Say ''
    Show-Status | Out-Null
}

function Do-Watch {
    <#
      「补挂守护进程」：App 启动时探到后端已经健康（比如你刚在终端里 up 过，
      或者上一次 App 的 watchdog 已经不在了）就直接返回，根本没跑 up ——
      于是 watchdog 没挂上，之后关 App / 硬杀 App 就会把后端留在后台。

      App 在这种情况下会调一次 `watch -OwnerPid <自己的 PID>`，我们按**当前实际活着的**
      服务来挂守护：谁在跑就准备停谁。已经在跑的可能是用户手工起的，但既然 App 正在
      用它、而用户又开着"关闭 App 时一并停止本地服务"，就一起带走（和 up -OwnerPid 的语义一致）。
    #>
    if ($OwnerPid -le 0) {
        Say '  watch 需要 -OwnerPid <pid>：它是给"已经在用本地服务的 App"补挂守护进程用的。' 'Yellow'
        Say '  例如: pwsh -File scripts\comfyhub.ps1 watch -OwnerPid 1234' 'DarkGray'
        exit 2
    }

    $stopApi = (@(Get-ApiProcess).Count -gt 0) -or ($null -ne (Get-ApiHealth))

    if (-not $stopApi) {
        Say '  后端没在跑，不需要挂守护进程。' 'DarkGray'
        Clear-OwnerWatchdog
        return
    }

    Say ''
    Say "  ComfyHub 补挂退出守护（App PID $OwnerPid 退出后停后端）" 'White'
    Start-OwnerWatchdog -Owner $OwnerPid -StopApi $stopApi | Out-Null
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
    # MySQL 已于 2026-09-29 退役 —— 这里只看后端与守护进程的日志
    $out = Join-Path $RunDir 'server.out.log'
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

    # 后端二进制 / App 的路径有两种布局（源码树 vs 装配好的发布包），这里都探一遍。
    # P4 收尾（2026-09-26）起后端是**一个 Rust exe**；旧的 Kotlin bat 只在 legacy\ 里留档。
    $packagedExe = Join-Path $ProjectRoot 'server\comfyhub-server-rs.exe'
    $gradleExe   = Join-Path $ProjectRoot 'server-rs\target\release\comfyhub-server-rs.exe'
    $batToShow   = if (Test-Path $packagedExe) { $packagedExe } else { $gradleExe }
    $appExe      = Resolve-AppExe
    $layout      = if (Test-Path (Join-Path $ProjectRoot 'viewer.exe')) { '发布包（便携式）' } else { '源码树' }

    # 布局是"是什么"不是"在不在"，不能塞进下面那个 Test-Path 列表里（会被报成"缺失"）
    Say ("  {0,-16} {1}" -f '运行布局', $layout) 'Gray'

    foreach ($p in @(
            @{ n = '项目根目录';    v = $ProjectRoot },
            @{ n = 'db\schema.sql'; v = (Join-Path $ProjectRoot 'db\schema.sql') },
            @{ n = 'db\schema.sqlite.sql'; v = (Join-Path $ProjectRoot 'db\schema.sqlite.sql') },
            @{ n = '后端二进制';     v = $batToShow },
            @{ n = '桌面 App';      v = $(if ($appExe) { $appExe.FullName } else { (Join-Path $ProjectRoot 'viewer.exe') }) }
        )) {
        $ok = Test-Path $p.v
        Say ("  {0,-16} {1}" -f $p.n, $(if ($ok) { 'OK' } else { "缺失: $($p.v)" })) $(if ($ok) { 'Green' } else { 'Yellow' })
    }

    # 库：SQLite 单文件（后端自建）。MySQL 已于 2026-09-29 退役，这里不再有任何老库判断。
    if (Test-Path $Script:SqliteDb) {
        $sizeMb = [math]::Round((Get-Item $Script:SqliteDb).Length / 1MB, 2)
        $wal = Join-Path $Script:SqliteDir 'comfyhub.db-wal'
        $walText = if (Test-Path $wal) { "，WAL $([math]::Round((Get-Item $wal).Length / 1KB, 1)) KB" } else { '' }
        Say ("  {0,-16} {1}" -f 'SQLite 库', "$Script:SqliteDb  ($sizeMb MB$walText)") 'Green'
    } else {
        Say ("  {0,-16} {1}" -f 'SQLite 库', "还没建：$Script:SqliteDb（后端首次启动会建）") 'Gray'
    }

    # 构建新鲜度：App 的产物比 lib\ 旧时，界面上看到的是旧代码。
    # 这类"改了没生效"最容易被误报成功能 bug（2026-09-17 视频预览图踩过一次），
    # 所以 doctor 里把"该启动的 App"和"正在跑的 App"分别报一遍（两者常常不是同一个）。
    $appStale = Get-AppBuildStaleness -Exe $appExe
    if ($appExe) {
        Say ("  {0,-16} {1}" -f 'App 构建', (Format-AppFreshness -Exe $appExe)) $(if ($appStale -and $appStale.Newer) { 'Yellow' } else { 'Gray' })
    }
    $runningExe = Get-RunningAppExe
    if ($runningExe) {
        $rs = Get-AppBuildStaleness -Exe $runningExe
        Say ("  {0,-16} {1}" -f '运行中的 App', "$($runningExe.FullName) — $(Format-AppFreshness -Exe $runningExe)") $(if ($rs -and $rs.Newer) { 'Yellow' } else { 'Gray' })
    }

    # 后端 jar 的构建新鲜度：dev-app / up -SkipBuild 都是"有产物就直接用"，
    # 旧 jar 上场完全无声（2026-09-23 现场：昨晚的 P0 修复全没参与运行）。
    $srvStale = Get-ServerBuildStaleness
    if ($srvStale) {
        $runningSrv = Get-RunningServerProcess
        $built = $srvStale.Jar.LastWriteTime.ToString('MM-dd HH:mm')
        $text = if ($srvStale.Newer) {
            "旧构建：$($srvStale.Jar.Name) 编于 $built，源码已改到 $($srvStale.Source.LastWriteTime.ToString('MM-dd HH:mm'))"
        } else {
            "最新（$($srvStale.Jar.Name) 编于 $built）"
        }
        Say ("  {0,-16} {1}" -f '后端构建', $text) $(if ($srvStale.Newer) { 'Yellow' } else { 'Gray' })
        if ($srvStale.Newer) { Show-ServerStaleWarning -Stale $srvStale -Running $runningSrv }
        elseif ($runningSrv -and $runningSrv.CreationDate -lt $srvStale.Jar.LastWriteTime) {
            # 第二种旧法：jar 是新的，但跑着的进程是在这次构建**之前**拉起来的。
            Say ("  {0,-16} {1}" -f '运行中的后端', "进程启动于 $($runningSrv.CreationDate.ToString('MM-dd HH:mm'))，早于 jar 的 $built —— 跑的还是旧类，restart 才能吃上") 'Yellow'
        }
    }

    # 模型能力目录（模态/思考档位自动更新链）：内置表随版本发，在线目录由后端
    # 每 7 天拉一次 models.dev（或设置页手动）。过期太久 = 新模型预填不到，黄牌提醒。
    $capFile = Join-Path $ProjectRoot 'storage\ai\capabilities\remote.json'
    if (Test-Path $capFile) {
        try {
            $cap = Get-Content $capFile -Raw -Encoding utf8 | ConvertFrom-Json
            $entryCount = ($cap.entries.PSObject.Properties | Measure-Object).Count
            $ageDays = $null
            if ($cap.fetchedAt) {
                $ageDays = [int]([datetimeoffset]::Now - [datetimeoffset]::Parse($cap.fetchedAt)).TotalDays
            }
            $ageText = if ($null -ne $ageDays) { "，$ageDays 天前" } else { '' }
            $stale = ($null -ne $ageDays) -and ($ageDays -gt 7)
            Say ("  {0,-16} {1}" -f '能力目录', "在线 $($cap.source) 更新于 $($cap.fetchedAt)$ageText（$entryCount 条模型声明）") $(if ($stale) { 'Yellow' } else { 'Gray' })
        } catch {
            Say ("  {0,-16} {1}" -f '能力目录', "remote.json 解析失败：$($_.Exception.Message)") 'Yellow'
        }
    } else {
        Say ("  {0,-16} {1}" -f '能力目录', '还没有在线目录（后端每 7 天自动拉取 models.dev；也可在设置页点「立即更新」）') 'Gray'
    }

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

    # 后端自己是不是"被关在项目里了"（2026-09-29）。
    #
    # 后端若是从**受限会话**（AI 助手的文件沙箱）里拉起来的，那份限制跟着进程走、
    # 子进程原样继承 —— 表现是 comfy_start 写不进 ComfyUI 的 temp/、comfy_use_media
    # 写不进 input…，而上面这些行**一条都看不出来**（这里只探端口/健康/构建新鲜度）。
    # 所以后端启动时自己探一次并留证（`server-rs\src\confinement.rs`），这里读出来报。
    # ⚠ 标记带 pid：**换了后端就重新判**，旧标记无权代表现状（那正是本条最常见的假红）。
    $confFile = Join-Path (Join-Path $ProjectRoot '.run') 'confinement.json'
    if (Test-Path $confFile) {
        $conf = $null
        try { $conf = Get-Content $confFile -Raw -Encoding utf8 | ConvertFrom-Json } catch { $conf = $null }
        if ($conf -and $conf.restricted) {
            $apiState0 = Read-ApiPortState -ProjectRoot $ProjectRoot
            $isCurrent = ($null -ne $apiState0) -and ([int64]$conf.pid -eq [int64]$apiState0.Pid)
            if ($isCurrent) {
                Say ("  {0,-16} {1}" -f '后端受限', "pid $($conf.pid)（当前在跑的这个）写不进项目外：$(($conf.blockedPaths -join '、'))") 'Red'
                # 修法要跟"内容一律在自身文件夹内"这条对齐 —— **不要**建议往 %LOCALAPPDATA% 之类拷
                $backendExe = $null
                # ⚠ 必须**先 dot-source 再调**（本文件另外两处 389/849 都是这个写法）：
                # `Resolve-Backend` 住在 backend.ps1 里，不是本文件自带的。
                $backendHelper2 = Join-Path $Scripts 'backend.ps1'
                if (Test-Path $backendHelper2) {
                    . $backendHelper2
                    $backendExe = (Resolve-Backend -ProjectRoot $ProjectRoot).Exe
                }
                if ($backendExe) {
                    Say ("  {0,-16} {1}" -f '', "后端映像：$backendExe") 'Red'
                    Say ("  {0,-16} {1}" -f '', '限制按**映像位置**判：映像在工作区内就写不了工作区外（与路径/ACL/用户/终端都无关）。') 'Red'
                    Say ("  {0,-16} {1}" -f '', '这是**源码树布局**的固有结果（项目根就是工作区）。要跑 ComfyUI 集成就用发布包：') 'Red'
                    Say ("  {0,-16} {1}" -f '', '  pwsh -File scripts\pack-release.ps1 -OutDir D:\dist\ComfyHub') 'Red'
                    Say ("  {0,-16} {1}" -f '', '  pwsh -File D:\dist\ComfyHub\scripts\comfyhub.ps1 up') 'Red'
                    Say ("  {0,-16} {1}" -f '', '发布包的后端在 <包根>\server\、数据在 <包根>\.db / storage / .run —— 内容照旧全在它自己的文件夹里。') 'Gray'
                    Say ("  {0,-16} {1}" -f '', '已经有工作区外的构建产物时，一句 $env:COMFYHUB_SERVER_EXE = "<完整路径>" 就够。') 'Gray'
                }
            } else {
                Say ("  {0,-16} {1}" -f '后端受限', "有旧标记（pid $($conf.pid)，不是当前在跑的后端）—— 重起后端会自动重新判定，可忽略") 'Gray'
            }
        }
    }

    $flutter = (Get-Command flutter -ErrorAction SilentlyContinue)
    Say ("  {0,-16} {1}" -f 'flutter', $(if ($flutter) { $flutter.Source } else { 'PATH 里没有' })) $(if ($flutter) { 'Green' } else { 'Red' })

    # API 端口单独报：它现在是"自动"的（默认 0），所以要看**后端记录在案的**那个端口。
    # 再顺手把 Windows 保留端口段点出来 —— 落在保留段里的端口"谁都绑不上、netstat 也看不见"，
    # 而错误信息只会说 "Address already in use"，非常难查（2026-09-22 本机 8080 就是这个）。
    $apiPortNow = Get-ApiPortNow
    if ($apiPortNow -gt 0) {
        $apiListen = Get-NetTCPConnection -State Listen -LocalPort $apiPortNow -ErrorAction SilentlyContinue
        Say ("  {0,-16} {1}" -f "API 端口 $apiPortNow", $(if ($apiListen) { "被 PID $($apiListen[0].OwningProcess) 占用（在跑）" } else { '空闲' })) 'Gray'
        $state = Read-ApiPortState -ProjectRoot $ProjectRoot
        if ($state -and $state.FellBack -and $state.RequestedPort -gt 0) {
            Say ("  {0,-16} {1}" -f '端口回落', "候选 $($state.RequestedPort) 绑不上，已改用 $apiPortNow（要固定端口请指定一个确实空闲的）") 'Yellow'
        }
    } else {
        Say ("  {0,-16} {1}" -f 'API 端口', '自动（0 = 由操作系统挑；真实端口写在 .run\api.json）') 'Gray'
    }

    $excluded = Get-ExcludedTcpPortRange -Port $apiPortNow
    if ($excluded) {
        Say ("  {0,-16} {1}" -f '端口保留段', "端口落在 Windows 保留段 $excluded 里（Hyper-V/WSL 预留）——换一个端口，或管理员执行 net stop winnat / net start winnat") 'Yellow'
    }

    # 运行时依赖：发布包带不走的那几样（pwsh / Java）在这里现形
    Say ''
    Say '  运行时依赖' 'White'
    Say '  ────────────────────────────────────────────────────────────'
    if (Get-Command Show-RuntimeReport -ErrorAction SilentlyContinue) {
        Show-RuntimeReport -ProjectRoot $ProjectRoot | Out-Null
    } else {
        Say '  （找不到 scripts\runtime-deps.ps1，跳过运行时体检）' 'Yellow'
    }

    # MCP 运行时（可选能力）：AI 的 ComfyUI 工具全靠它，所以 doctor 里单独报一段。
    # 这一项**不影响 App 能不能启动**，所以未就绪只是黄色，不是红色。
    # doctor 走 -Deep（真去问 comfy-cli，约 6 秒）：体检就是要看真相，不是看记录。
    if (Get-Command Show-McpRuntimeReport -ErrorAction SilentlyContinue) {
        Show-McpRuntimeReport -ProjectRoot $ProjectRoot -Deep
    } else {
        Say '  （找不到 scripts\mcp-runtime.ps1，跳过 MCP 运行时体检）' 'Yellow'
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
    'restart' {
        # restart = down + up。这里**故意不复用重启前的端口**，`Do-Up` 会用 `-Port 0` 要一个新的：
        #
        # 为什么（2026-09-22 实测，值得记住）：restart 期间会**先后有两个后端**存在几秒
        # （旧进程收到强杀信号还没退干净、新进程已经在启动），而端口状态文件是"后写的赢"。
        # 于是"文件里写的"和"真的在听的"会对不上：新后端可能读到旧记录、或旧进程的收尾钩子
        # 覆盖掉新记录 → bind 失败 → 回落随机端口。表现是**每次重启白等 1~2 分钟**
        # （两次启动尝试）并且中途报一句"不健康"，而端口最后还是变了。
        # 与其在竞态里挣扎，不如让 restart 每次都用一个**全新端口**：
        # 端口从来不是用户要记的东西（前端/脚本/捕获扩展都读 .run\api.json）。
        Do-Down
        $Script:ApiPort = 0
        Do-Up
    }
    'logs'    { Do-Logs }
    'doctor'  { Do-Doctor }
}

