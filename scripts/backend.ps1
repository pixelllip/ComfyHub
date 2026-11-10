<#
.SYNOPSIS
    Rust 后端的小工具：**找它 / 编它 / 起它 / 停它 / 看它**（给 e2e 与 dev 脚本用）。

.DESCRIPTION
    ⚠ **运行期的真源是 App 自己**，不是这个脚本：

      · 找 exe    → `lib\core\backend_paths.dart` 的 `serverExeCandidates()`
      · 起 / 停   → `lib\core\backend_launcher.dart`（Dart 直接 CreateProcess，
                    带 `--watch-stdin`；强杀 App 时后端自己退）

    本文件只服务于"没有 App 的场合"：`scripts\e2e-*-test.ps1`（需要一个后端来打接口）
    与 `scripts\dev-app.ps1`（它只需要保证后端**存在且是新的**，起还是 App 自己起）。
    **候选顺序与 Dart 那份必须一致** —— 改一边就要改另一边（AGENTS §7 的纪律）。

    2026-09-30 之前这条路是 `comfyhub.ps1 up → server.ps1 → silent-process.ps1(WMI)
    → cmd → exe`，App 改成直接起 exe 之后那几层连同 `watch-owner.ps1` 一起删了。
    这里起后端用 `Start-Process -WindowStyle Hidden`（不弹窗口），**不带 `--watch-stdin`**：
    Start-Process 给的不是管道，带了后端会当场退出（见 `server-rs/src/main.rs`）。

.EXAMPLE
    pwsh -File scripts\backend.ps1 exe                 # 后端程序在哪
    pwsh -File scripts\backend.ps1 build               # 只 cargo build --release
    pwsh -File scripts\backend.ps1 start               # 起来并等到健康（缺/旧会自动编）
    pwsh -File scripts\backend.ps1 start -SkipBuild    # 直接用现有产物
    pwsh -File scripts\backend.ps1 stop
    pwsh -File scripts\backend.ps1 status
    pwsh -File scripts\backend.ps1 wait -Seconds 30
#>
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateSet('exe', 'build', 'start', 'stop', 'restart', 'status', 'wait')]
    [string]$Action = 'status',

    # 起后端时听哪个网卡（`0.0.0.0` = 允许手机 / 局域网设备连接）
    [ValidateSet('127.0.0.1', '0.0.0.0')]
    [string]$BindHost = '127.0.0.1',

    # 起之前不检查/不跑 cargo build（e2e 里常用）
    [switch]$SkipBuild,

    # wait 的上限
    [int]$Seconds = 60
)

$ErrorActionPreference = 'Stop'

# 被 App / 别的脚本重定向时钉成 UTF-8（中文串在 936 代码页下会乱码）
if ([Console]::IsOutputRedirected) {
    try { [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false) } catch { }
}

function Resolve-ProjectRootForBackend {
    param([string]$Start)
    $dir = if ($Start) { (Resolve-Path $Start).Path } else { $PSScriptRoot }
    while ($dir) {
        # 与 lib\core\backend_paths.dart 的 looksLikeRoot 同一组判据
        if ((Test-Path (Join-Path $dir 'server-rs\Cargo.toml')) -or
            (Test-Path (Join-Path $dir 'db\schema.sqlite.sql')) -or
            (Test-Path (Join-Path $dir 'backend\comfyhub-server-rs.exe'))) {
            return $dir
        }
        $parent = Split-Path $dir -Parent
        if (-not $parent -or $parent -eq $dir) { break }
        $dir = $parent
    }
    return (Split-Path $PSScriptRoot -Parent)
}

$ProjectRoot = Resolve-ProjectRootForBackend
$ExeName = 'comfyhub-server-rs.exe'

<#  数据目录（库 + storage）在哪。**与 lib\core\backend_paths.dart 的 `resolveDataDir` 同一份约定**：
    `<根>\data-dir.json`（`{"dataDir":"D:\..."}`）存在就用它，否则就是运行根目录。
    `.run\` / `backups\` / `comfyui\` 永远按运行根目录算，不跟着数据走。

    判定顺序（2026-11-10 起，与 Dart 侧逐字一致）：
      ① 指针指的目录不在了 → 回运行根目录（**不许**照着不存在的路径建一个空库）；
      ② 偏好里有"上次用的是哪一份库"的记录（`app-support\shared_preferences.json` 的
         `flutter.comfyhub.library.last`）→ **先按身份认**：指针那一级就是它就用它；
         指针那一级没有库 / 那份库没身份（老数据）→ 用记录里的路径，或在
         `<指针>\storage` 那几层里按身份找回来；
      ③ 还没有记录（老库没有身份）→ 一次性过渡：`<指针>\storage\` 里那份比指针
         那一级**大 4 倍以上**时才换过去（残留那份通常只有几 MB）。
    库的身份就在库旁边：`<数据目录>\.db\library.json` 的 `id`。 #>
function Test-DataDb {
    param([string]$Path)
    if (-not $Path) { return $false }
    return (Test-Path -LiteralPath (Join-Path $Path '.db\comfyhub.db') -PathType Leaf)
}

function Get-DataDbBytes {
    param([string]$Path)
    $file = Join-Path $Path '.db\comfyhub.db'
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { return 0L }
    try { return (Get-Item -LiteralPath $file).Length } catch { return 0L }
}

<#  读一份库的身份（`<数据目录>\.db\library.json` 的 id；没有 / 读不懂回 $null）。 #>
function Get-LibraryId {
    param([string]$DataDir)
    if (-not $DataDir) { return $null }
    $f = Join-Path $DataDir '.db\library.json'
    if (-not (Test-Path -LiteralPath $f -PathType Leaf)) { return $null }
    try {
        $j = Get-Content -LiteralPath $f -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($j.id) { return "$($j.id)" }
    } catch {
        Write-Host "身份文件读不了（$f），这次当没有身份" -ForegroundColor Yellow
    }
    return $null
}

<#  读"上次用的是哪一份库"（偏好里的 `flutter.comfyhub.library.last`，JSON 字符串）。
    返回 @{ id; path } 或 $null。这是"复制出去的构建照样打开你的库"的依据。 #>
function Get-LastLibrary {
    param([string]$ProjectRoot = $ProjectRoot)
    $f = Join-Path $ProjectRoot 'app-support\shared_preferences.json'
    if (-not (Test-Path -LiteralPath $f -PathType Leaf)) { return $null }
    try {
        $prefs = Get-Content -LiteralPath $f -Raw -Encoding UTF8 | ConvertFrom-Json
        $raw = $prefs.'flutter.comfyhub.library.last'
        if (-not $raw) { return $null }
        $ref = "$raw" | ConvertFrom-Json
        if ($ref.id -and $ref.path) { return $ref }
    } catch {
        Write-Host "上次用的库记录读不懂（$f），当没记录" -ForegroundColor Yellow
    }
    return $null
}

function Resolve-DataDir {
    param([string]$ProjectRoot = $ProjectRoot)
    $chosen = $ProjectRoot
    $file = Join-Path $ProjectRoot 'data-dir.json'
    if (Test-Path -LiteralPath $file) {
        try {
            $raw = Get-Content -LiteralPath $file -Raw -Encoding UTF8 | ConvertFrom-Json
            $dir = if ($raw.dataDir) { "$($raw.dataDir)".Trim() } else { '' }
            if ($dir) {
                if (Test-Path -LiteralPath $dir -PathType Container) {
                    $chosen = $dir
                } else {
                    Write-Host "data-dir.json 指的 $dir 不在了，这次用运行根目录 $ProjectRoot" -ForegroundColor Yellow
                }
            }
        } catch {
            Write-Host "data-dir.json 读不了（$file），按运行根目录走" -ForegroundColor Yellow
        }
    }
    # ① 先听"上次用的是哪一份库"的记录（与 Dart 侧 resolveDataDir 同一判据）
    $known = Get-LastLibrary -ProjectRoot $ProjectRoot
    if ($known) {
        if ((Get-LibraryId $chosen) -eq $known.id) { return $chosen }
        $chosenId = Get-LibraryId $chosen
        if (-not $chosenId) {
            # 指针这一级没有库、或者那份库没有身份（老数据）→ 听记录的
            if ($known.path -and (Test-DataDb $known.path) -and ((Get-LibraryId $known.path) -eq $known.id)) {
                Write-Host "按上次用的那份库打开：$($known.path)（指针写的是 $chosen）" -ForegroundColor DarkGray
                return $known.path
            }
            foreach ($c in @((Join-Path $chosen 'storage'), (Join-Path $chosen 'storage\storage'))) {
                if ((Test-DataDb $c) -and ((Get-LibraryId $c) -eq $known.id)) {
                    Write-Host "在 $c 找到上次用的那份库（身份对得上）" -ForegroundColor DarkGray
                    return $c
                }
            }
        }
        return $chosen
    }
    # ② 没有记录时**就用指针这一级**（或运行根目录）。
    #    ⚠ 不在此处按大小 / 修改时间推测"哪份是目标库"：该规则只适用于单台机器。
    #    同一棵树中发现其他库时，App 会列出并由界面选择；脚本只认指针 + 记录，
    #    无法判断时不推测。
    return $chosen
}

<#  产物目录**实际**在哪。**与 lib\core\backend_paths.dart 的 `resolvedStorageDirOf` 同一份约定**：

      标准：`<数据目录>\storage`
      双层：`<数据目录>\storage\storage`（旧版把 storage 文件夹选成了数据目录）
      扁平：`<数据目录>` 本身（用户把里层 storage 的内容挪到了上一级）

    判据是"哪个目录里真的有内容"，空目录不算 —— 启动代码会建空壳。 #>
function Test-StorageContent {
    param([string]$Path)
    if (-not $Path) { return $false }
    foreach ($name in @('media', 'thumbs', 'ai', 'ai-attachments', 'ai-thumbs')) {
        $dir = Join-Path $Path $name
        if (-not (Test-Path -LiteralPath $dir -PathType Container)) { continue }
        $items = @(Get-ChildItem -LiteralPath $dir -Force -ErrorAction SilentlyContinue)
        if ($items.Count -gt 0) { return $true }
    }
    return $false
}

function Resolve-StorageDir {
    param([string]$DataDir)
    $canonical = Join-Path $DataDir 'storage'
    if (Test-StorageContent -Path $canonical) { return $canonical }
    $nested = Join-Path $canonical 'storage'
    if (Test-StorageContent -Path $nested) { return $nested }
    if (Test-StorageContent -Path $DataDir) { return $DataDir }
    return $canonical
}

<#  后端 exe 在哪。**顺序即优先级**，与 `lib\core\backend_paths.dart` 一字不差：
      ① $env:COMFYHUB_SERVER_EXE  —— 显式覆盖（把后端映像挪出受限目录也是用这个）
      ② <根>\backend\             —— release 产物：后端是 App 自己的资产
      ③ <根>\server\              —— 旧发布包布局（兼容）
      ④ <根>\server-rs\target\release\  —— 源码树优先 release
      ⑤ <根>\server-rs\target\debug\    —— 没编过 release 时用 debug
    ⚠ 这里**不认** "viewer.exe 同级" 那条（脚本没有 exe 同级的概念），
      所以发布包里跑 `scripts\backend.ps1` 时走的是 ③ 之后的 <根>\backend\（②）。  #>
function Resolve-RustExe {
    param([string]$ProjectRoot = $ProjectRoot)
    if ($env:COMFYHUB_SERVER_EXE) {
        $explicit = $env:COMFYHUB_SERVER_EXE.Trim()
        if ($explicit -and (Test-Path $explicit)) { return $explicit }
        Write-Host "==> COMFYHUB_SERVER_EXE=$explicit 不存在，当没设" -ForegroundColor Yellow
    }
    foreach ($cand in @(
            (Join-Path $ProjectRoot "backend\$ExeName"),
            (Join-Path $ProjectRoot "server\$ExeName"),
            (Join-Path $ProjectRoot "server-rs\target\release\$ExeName"),
            (Join-Path $ProjectRoot "server-rs\target\debug\$ExeName")
        )) {
        if (Test-Path $cand) { return $cand }
    }
    return $null
}

<#  「用哪个后端」——Kotlin 已于 2026-09-26 退役（P4 收尾），所以只剩 rust / none。
    保留这个函数与返回值形状：`scripts\e2e-recipe-test.ps1` 直接读 `.Kind` / `.Exe`。  #>
function Resolve-Backend {
    param([string]$ProjectRoot = $ProjectRoot)
    $rust = Resolve-RustExe -ProjectRoot $ProjectRoot
    if ($rust) {
        return @{ Kind = 'rust'; Exe = $rust; Note = 'Rust 后端'; RustExe = $rust }
    }
    return @{
        Kind    = 'none'
        Exe     = $null
        Note    = '找不到 Rust 后端二进制（cargo build --release --manifest-path server-rs\Cargo.toml）'
        RustExe = $null
    }
}

<#  二进制构建新鲜度：`server-rs\src` 里最新的 .rs 是否比 exe 新。

    两条路都用它：`e2e-recipe-test.ps1` 拿它挡"拿旧二进制证明新实现"（假绿），
    本脚本的 `start` 拿它决定要不要先编一次。
    界面类修复踩过同一个坑（AGENTS §1："我改了但界面没变"先查构建新鲜度）—— 后端同理。  #>
function Get-RustBuildFreshness {
    param([string]$Exe, [string]$ProjectRoot = $ProjectRoot)
    if (-not $Exe -or -not (Test-Path $Exe)) { return $null }
    $srcDir = Join-Path $ProjectRoot 'server-rs\src'
    $exeTime = (Get-Item $Exe).LastWriteTime
    if (-not (Test-Path $srcDir)) {
        return @{ ExeTime = $exeTime; NewestSource = $null; Stale = $false }
    }
    $newest = Get-ChildItem -Path $srcDir -Recurse -Filter *.rs -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
    $stale = $false
    if ($newest) { $stale = $newest.LastWriteTime -gt $exeTime }
    return @{ ExeTime = $exeTime; NewestSource = $newest; Stale = $stale }
}

function Get-BackendProcess {
    Get-CimInstance Win32_Process -Filter "Name='$ExeName'" -ErrorAction SilentlyContinue
}

<#  完整性标签修复（2026-09-30 从 scripts\integrity.ps1 并过来，那个文件已删）。

    "写不进工作区外、一律 os error 5" 的真因是 Windows 强制完整性控制（MIC）：
    工作区目录树带着 Low 标签 → 从工作区里构建出来的 exe 启动的进程就是 Low 令牌 →
    Low 写不了工作区外的 Medium 对象；而 Windows 取 min(父进程, 映像），链路上
    任何一环是 Low，子进程就掉回 Low。

    所以起后端之前把"会被我们启动的那几个映像"修回 Medium（只动标签，不动位置）。
    幂等：都不是 Low 时什么都不做。`COMFYHUB_NO_INTEGRITY_REPAIR=1` 可关掉。  #>
function Get-IntegrityLabel {
    param([string]$Path)
    if (-not $Path -or -not (Test-Path -LiteralPath $Path)) { return $null }
    try {
        $hit = & icacls $Path 2>$null | Select-String -Pattern 'Mandatory Label' | Select-Object -First 1
    } catch { return $null }
    if (-not $hit) { return $null }
    $t = $hit.ToString()
    if ($t -match 'Low Mandatory Level') { return 'Low' }
    if ($t -match 'Medium Mandatory Level') { return 'Medium' }
    if ($t -match 'High Mandatory Level') { return 'High' }
    if ($t -match 'System Mandatory Level') { return 'System' }
    return 'Other'
}

function Repair-IntegrityLabel {
    param([string]$Path, [switch]$Inherited)
    if (-not (Test-Path -LiteralPath $Path)) { return $false }
    if ((Get-IntegrityLabel -Path $Path) -ne 'Low') { return $false }
    if ($Inherited) {
        # 目录带 (OI)(CI)：以后构建出来的新文件直接继承 Medium，不用每次都修
        & icacls $Path /setintegritylevel '(OI)(CI)Medium' 2>&1 | Out-Null
    } else {
        & icacls $Path /setintegritylevel Medium 2>&1 | Out-Null
    }
    if ($LASTEXITCODE -ne 0) { return $false }
    # 改完复读一次：icacls 成功不等于标签真的变了（Low 进程改不上，静默失败）
    return ((Get-IntegrityLabel -Path $Path) -ne 'Low')
}

function Repair-LaunchImages {
    param([string]$ProjectRoot = $ProjectRoot, [string]$ExtraExe)
    if ($env:COMFYHUB_NO_INTEGRITY_REPAIR -and
        $env:COMFYHUB_NO_INTEGRITY_REPAIR.Trim() -match '^(?i:1|true|yes|on)$') {
        return
    }
    $targets = @(
        @{ Path = (Join-Path $ProjectRoot 'server-rs\target');                  Inherited = $true;  What = '后端构建目录' },
        @{ Path = (Join-Path $ProjectRoot 'build\windows\x64\runner\Release');  Inherited = $true;  What = 'App 产物目录 (Release)' },
        @{ Path = (Join-Path $ProjectRoot 'build\windows\x64\runner\Debug');    Inherited = $true;  What = 'App 产物目录 (Debug)' },
        @{ Path = (Join-Path $ProjectRoot 'storage\ai\mcp\python');             Inherited = $true;  What = 'MCP 便携 python 目录' }
    )
    if ($ExtraExe) {
        $targets = @(@{ Path = $ExtraExe; Inherited = $false; What = '后端 exe' }) + $targets
    }
    foreach ($t in $targets) {
        if (-not (Test-Path -LiteralPath $t.Path)) { continue }
        if (Repair-IntegrityLabel -Path $t.Path -Inherited:$t.Inherited) {
            Write-Host "  [权限] $($t.What) 的 Low 标签已修回 Medium：$($t.Path)" -ForegroundColor DarkGray
        }
    }
}

function Test-TcpPort([string]$TargetHost, [int]$TargetPort, [int]$TimeoutMs = 250) {
    $client = [System.Net.Sockets.TcpClient]::new()
    try {
        $iar = $client.BeginConnect($TargetHost, $TargetPort, $null, $null)
        if (-not $iar.AsyncWaitHandle.WaitOne($TimeoutMs)) { return $false }
        $client.EndConnect($iar)
        return $true
    } catch { return $false }
    finally { $client.Dispose() }
}

<#  后端实际监听的端口只有一个真源：它自己写的 `<根>\.run\api.json`。
    （解析规则与 Dart / Python 扩展共读同一个文件，别在这里自己编一个端口。）  #>
function Read-ApiPortState {
    param([string]$ProjectRoot = $ProjectRoot)
    $file = Join-Path $ProjectRoot '.run\api.json'
    if (-not (Test-Path $file)) { return $null }
    try { return (Get-Content $file -Raw | ConvertFrom-Json) } catch { return $null }
}

function Get-BackendHealth {
    param([string]$ProjectRoot = $ProjectRoot)
    $state = Read-ApiPortState -ProjectRoot $ProjectRoot
    if (-not $state -or $state.port -le 0) { return $null }
    if (-not (Test-TcpPort '127.0.0.1' $state.port)) { return $null }
    try { return (Invoke-RestMethod "http://127.0.0.1:$($state.port)/api/health" -TimeoutSec 3) }
    catch { return $null }
}

function Wait-BackendHealth {
    param([int]$Seconds = 60, [string]$ProjectRoot = $ProjectRoot)
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $deadline) {
        $h = Get-BackendHealth -ProjectRoot $ProjectRoot
        if ($h -and $h.database -eq 'ok') { return $h }
        Start-Sleep -Milliseconds 300
    }
    return $null
}

function Invoke-BackendBuild {
    param([string]$ProjectRoot = $ProjectRoot)
    Write-Host '==> cargo build --release（后端会编到最新）' -ForegroundColor Cyan
    & cargo build --release --manifest-path (Join-Path $ProjectRoot 'server-rs\Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw 'cargo build --release 失败' }
    return (Resolve-RustExe -ProjectRoot $ProjectRoot)
}

function Do-Build {
    # dev-app.ps1 调的就是这一条："已新就跳过"，免得每次热重载前都等一次 cargo。
    $exe = Resolve-RustExe
    $fresh = Get-RustBuildFreshness -Exe $exe
    if ($exe -and -not ($fresh -and $fresh.Stale)) {
        Write-Host "后端已是最新: $exe" -ForegroundColor Green
        return $exe
    }
    $exe = Invoke-BackendBuild
    if (-not $exe) { throw '编完了却找不到后端 exe' }
    Write-Host "后端: $exe" -ForegroundColor Green
    return $exe
}

function Do-Start {
    param([switch]$SkipBuild, [string]$BindHost = '127.0.0.1')

    $exe = Resolve-RustExe
    if (-not $SkipBuild) {
        $fresh = Get-RustBuildFreshness -Exe $exe
        if (-not $exe -or ($fresh -and $fresh.Stale)) {
            $why = if (-not $exe) { '没有产物' } else { "比源码旧（$($fresh.NewestSource.Name)）" }
            Write-Host "==> 后端$why，先编一次" -ForegroundColor Cyan
            $exe = Invoke-BackendBuild
        }
    }
    if (-not $exe) {
        throw '找不到 Rust 后端二进制。构建：cargo build --release --manifest-path server-rs\Cargo.toml'
    }

    $h = Get-BackendHealth
    if ($h -and $h.database -eq 'ok') {
        $state = Read-ApiPortState
        Write-Host "后端已在运行且健康（http://127.0.0.1:$($state.port)，pid $($state.pid)）。" -ForegroundColor Yellow
        return
    }
    if (Get-BackendProcess) {
        Write-Host '==> 有后端进程但不健康，先停掉' -ForegroundColor Yellow
        Do-Stop
    }

    $DataDir = Resolve-DataDir -ProjectRoot $ProjectRoot
    $StorageDir = Resolve-StorageDir -DataDir $DataDir
    New-Item -ItemType Directory -Force -Path (Join-Path $DataDir '.db') | Out-Null
    New-Item -ItemType Directory -Force -Path $StorageDir | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $ProjectRoot '.run') | Out-Null

    # 起之前先把 Low 完整性标签修回 Medium（只动标签，不动位置；详见 Repair-LaunchImages）
    Repair-LaunchImages -ProjectRoot $ProjectRoot -ExtraExe $exe

    # 端口状态与工具权限的基准：后端认这个环境变量（App 起它时也是这么传的）
    $env:COMFYHUB_PROJECT_ROOT = $ProjectRoot
    $argLine = "--db `"$(Join-Path $DataDir '.db\comfyhub.db')`" " +
        "--storage `"$StorageDir`" " +
        "--host $BindHost --port 0"

    Write-Host "==> 启动后端（$BindHost，端口自动）exe=$exe" -ForegroundColor Cyan
    # -WindowStyle Hidden：控制台程序不给窗口（App 那条路靠 Dart 的 CREATE_NO_WINDOW，
    # 这里使用 .NET 的 SW_HIDE —— 两条路径均不显示控制台窗口）
    $proc = Start-Process -FilePath $exe -ArgumentList $argLine -WorkingDirectory $ProjectRoot `
        -WindowStyle Hidden -PassThru
    Write-Host "    pid $($proc.Id)" -ForegroundColor DarkGray

    $health = Wait-BackendHealth -Seconds $Seconds
    if (-not $health) {
        Write-Host '启动超时 / 数据库不通。最近日志：' -ForegroundColor Red
        Do-Status
        throw '后端启动失败'
    }
    $state = Read-ApiPortState
    Write-Host "后端就绪: http://127.0.0.1:$($state.port)  (db=$($health.database))" -ForegroundColor Green
}

function Do-Stop {
    $killed = $false
    $state = Read-ApiPortState
    if ($state -and $state.pid -gt 0) {
        Stop-Process -Id $state.pid -Force -ErrorAction SilentlyContinue
        $killed = $true
    }
    foreach ($p in @(Get-BackendProcess)) {
        Stop-Process -Id $p.ProcessId -Force -ErrorAction SilentlyContinue
        $killed = $true
    }
    if (-not $killed) { Write-Host '后端未运行。' -ForegroundColor Yellow; return }

    # 等它真的死透并放开端口（重启时不等就会撞进空窗）
    $deadline = (Get-Date).AddSeconds(15)
    while ((Get-Date) -lt $deadline -and @(Get-BackendProcess).Count -gt 0) {
        Start-Sleep -Milliseconds 200
    }
    # 强杀没有优雅退出，端口记录要自己删（留着只会让下一次去探一个空端口）
    Remove-Item (Join-Path $ProjectRoot '.run\api.json') -Force -ErrorAction SilentlyContinue
    Write-Host '后端已停止。' -ForegroundColor Green
}

function Do-Status {
    $DataDir = Resolve-DataDir -ProjectRoot $ProjectRoot
    $dbFile = Join-Path $DataDir '.db\comfyhub.db'
    if (Test-Path $dbFile) {
        Write-Host "数据库:    SQLite $dbFile（$([math]::Round((Get-Item $dbFile).Length / 1MB, 2)) MB）"
    } else {
        Write-Host "数据库:    SQLite 还没建（$dbFile；后端首次启动会建）" -ForegroundColor Yellow
    }
    $procs = @(Get-BackendProcess)
    if ($procs.Count -gt 0) {
        Write-Host "后端进程:  PID $($procs.ProcessId -join ',')" -ForegroundColor Green
    } else {
        Write-Host '后端进程:  未运行' -ForegroundColor Yellow
    }
    $h = Get-BackendHealth
    if ($h -and $h.database -eq 'ok') {
        $state = Read-ApiPortState
        Write-Host "健康检查:  OK  version=$($h.version)  db=ok" -ForegroundColor Green
        Write-Host "API 地址:  http://127.0.0.1:$($state.port)"
        Write-Host "存储目录:  $($h.storageDir)"
    } elseif ($h) {
        Write-Host "健康检查:  异常  db=$($h.database)" -ForegroundColor Red
    } else {
        $state = Read-ApiPortState
        if ($state -and $state.port -gt 0) { Write-Host "端口记录:  $($state.port)（.run\api.json）" }
        Write-Host '健康检查:  无法连接' -ForegroundColor Red
    }
}

function Do-Exe {
    $exe = Resolve-RustExe
    if (-not $exe) { throw (Resolve-Backend).Note }
    Write-Host $exe
}

<#  ⚠ **只有"直接跑本文件"才执行动作**：`scripts\e2e-recipe-test.ps1` 会 dot-source
    本文件来借 `Resolve-Backend` / `Get-RustBuildFreshness`，那时不能顺手把后端起了
    （AGENTS §5 那条"脚本被 dot-source 时 param() 会覆盖宿主变量"的坑同源）。  #>
$DotSourced = $MyInvocation.InvocationName -eq '.'
if (-not $DotSourced) {
    switch ($Action) {
        'exe' { Do-Exe }
        'build' { Do-Build | Out-Null }
        'start' { Do-Start -SkipBuild:$SkipBuild -BindHost $BindHost }
        'stop' { Do-Stop }
        'restart' { Do-Stop; Start-Sleep -Milliseconds 500; Do-Start -SkipBuild:$SkipBuild -BindHost $BindHost }
        'status' { Do-Status }
        'wait' {
            if (Wait-BackendHealth -Seconds $Seconds) { Write-Host '后端就绪' -ForegroundColor Green }
            else { throw "等了 $Seconds 秒后端还没就绪" }
        }
    }
}
