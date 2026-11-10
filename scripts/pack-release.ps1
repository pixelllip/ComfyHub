<#
.SYNOPSIS
    ComfyHub 发布包打包：把 Flutter App + Rust 后端装配成一个可以直接给别人的目录。

.DESCRIPTION
    2026-09-30 起 `flutter build windows --release` 自己就会把后端装进产物
    （`windows\CMakeLists.txt` 跑 cargo 并装到 `<bundle>\backend\`），所以"把 Release
    目录拷给别人"本身已经能跑。本脚本按 packaging\manifest.json 这份**软件打包清单**
    做"发布级"装配：校验关键文件、顺带拷 scripts/db/skills、写 BUILD-INFO、可选打 zip。

    **默认就地装配**：输出目录就是 flutter build 的产物目录
    `build\windows\x64\runner\Release\` —— 装完之后这个目录本身就是完整可运行的发布包，
    直接双击里面的 viewer.exe 就能用，也可以整个目录拷给别人。

      · 想另存一份干净的副本（比如打成 zip 发出去）用 -OutDir 指个别的地方；
      · ⚠ `flutter clean` / 重新 clone 会把这个目录整个清掉，那样就只剩 App 了，
        需要重新跑一次本脚本。

    发布包是便携式的（免安装、免管理员）：整个目录就是运行时根目录，可整体搬走。
    可写数据都在根目录内：
      · 数据库      <根>\.db       （SQLite 单文件 comfyhub.db + 它自己的 -wal / -shm）
      · 生成产物    <根>\storage    （后端按 --storage 写入，App 直启时显式传入）
      · 日志 / PID  <根>\.run
    只读的运行时件：
      · App         <根>\viewer.exe（+ flutter_windows.dll / data/）
      · 后端        <根>\backend\   （comfyhub-server-rs.exe，App 自己的资产）
      · 脚本        <根>\scripts    （构建脚本 + 必要脚本 + video-poster 运行期依赖）
      · SQL         <根>\db         （schema.sqlite.sql 是现在的真源）
      · Skills      <根>\skills     （只读的项目自带 skill）

    注意（P2，2026-09-23）：数据库是 **SQLite**（驱动自带引擎），
    所以发布包**不再携带便携版 MySQL**（少约 300MB），也不再需要 VC++ 运行时。
    （2026-09-29 起 MySQL 连"把老库搬进新库"那条通道一起退役了 —— 运行期数据库是
     SQLite 单文件，包里不再有任何数据库本体，这两行只是说明来龙去脉。）
    注意（P4，2026-09-26）：后端是 **Rust 单文件 exe**（静态链接），不再需要 JVM，
    发布包里没有 jre / server\bin。旧 Kotlin 源码（`legacy/`）**2026-09-30 已整个删除**
    （用户要求精简）—— 要对照从 git 历史取，别指望在树里找到它。

.EXAMPLE
    pwsh -File scripts\pack-release.ps1
    pwsh -File scripts\pack-release.ps1 -Zip
    pwsh -File scripts\pack-release.ps1 -SkipApp -Clean               # 只重装后端，快速迭代
    pwsh -File scripts\pack-release.ps1 -OutDir "$env:USERPROFILE\ComfyHub"   # 另存一份干净的独立副本（目录随便挑）
#>
[CmdletBinding()]
param(
    # 发布包输出目录。默认**就地**装配进 flutter build 的产物目录
    # build\windows\x64\runner\Release（装完那个目录就是完整发布包）
    [string]$OutDir,

    # 装配完再打一个 zip（几百 MB，会慢一会儿）
    [switch]$Zip,

    # 跳过某几件（迭代时用）
    [switch]$SkipApp,
    [switch]$SkipBackend,
    # ⚠ JRE 组件随 Kotlin 退役一起删了（P4 收尾，2026-09-26）：发布包里没有 JVM。

    # 不裁剪（留给还带 prune 规则的组件）
    [switch]$NoPrune,

    # 装配前清空输出目录（默认只增量覆盖；改裁剪规则后建议带上）
    [switch]$Clean
)

$ErrorActionPreference = 'Stop'

$ProjectRoot = Split-Path -Parent $PSScriptRoot
$ManifestFile = Join-Path $ProjectRoot 'packaging\manifest.json'
if (-not $OutDir) { $OutDir = Join-Path $ProjectRoot 'build\windows\x64\runner\Release' }

# 被 App / 别的脚本重定向时钉成 UTF-8（与其它脚本一致，避免中文字符串乱码）
if ([Console]::IsOutputRedirected) {
    try { [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false) } catch { }
}

$Script:Warnings = @()
$Script:Failures = @()

function Say([string]$msg, [string]$color = 'Gray') { Write-Host $msg -ForegroundColor $color }
function Step([string]$msg) { Say ''; Say "==> $msg" 'Cyan' }
function Warn([string]$msg) { $Script:Warnings += $msg; Say "  !! $msg" 'Yellow' }
function Fail([string]$msg) { $Script:Failures += $msg; Say "  !! $msg" 'Red' }

function Format-Size([long]$Bytes) {
    if ($Bytes -ge 1GB) { return ('{0:N2} GB' -f ($Bytes / 1GB)) }
    if ($Bytes -ge 1MB) { return ('{0:N1} MB' -f ($Bytes / 1MB)) }
    return ('{0:N1} KB' -f ($Bytes / 1KB))
}

function Get-DirSize([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return 0 }
    return (Get-ChildItem -LiteralPath $Path -Recurse -File -Force -ErrorAction SilentlyContinue |
            Measure-Object -Property Length -Sum).Sum
}

# ---------------------------------------------------------------------------
#  （2026-09-30：JDK/JRE 解析整体删除。Rust 后端是静态链接的单文件 exe，
#  发布包与构建都不再需要 JVM；Android 构建走 AGP 内建 Kotlin，见 docs 里那篇迁移说明。）
# ---------------------------------------------------------------------------

# ---------------------------------------------------------------------------
#  裁剪规则：manifest 里写的是 glob（** / * / ?），PowerShell 的 -like 不认 **，
#  这里翻成正则再匹配相对路径（统一用 / 分隔）。
# ---------------------------------------------------------------------------

function Convert-GlobToRegex([string]$Glob) {
    $g = $Glob -replace '\\', '/'
    $sb = [System.Text.StringBuilder]::new()
    [void]$sb.Append('^')
    $i = 0
    while ($i -lt $g.Length) {
        $c = $g[$i]
        if ($c -eq '*') {
            if (($i + 1) -lt $g.Length -and $g[$i + 1] -eq '*') {
                if (($i + 2) -lt $g.Length -and $g[$i + 2] -eq '/') {
                    [void]$sb.Append('(.*/)?'); $i += 3; continue
                }
                [void]$sb.Append('.*'); $i += 2; continue
            }
            [void]$sb.Append('[^/]*'); $i++; continue
        } elseif ($c -eq '?') {
            [void]$sb.Append('[^/]'); $i++; continue
        }
        [void]$sb.Append([regex]::Escape([string]$c)); $i++
    }
    [void]$sb.Append('$')
    return $sb.ToString()
}

function Test-Pruned([string]$RelPath, [string[]]$Patterns) {
    foreach ($p in $Patterns) {
        if (-not $p) { continue }
        if ($RelPath -match (Convert-GlobToRegex $p)) { return $true }
    }
    return $false
}

# ---------------------------------------------------------------------------
#  拷贝
# ---------------------------------------------------------------------------

function Copy-Tree {
    <#  把 $Source 整个铺到 $Dest，按 $Prune 的 glob 丢掉不需要的文件  #>
    param(
        [Parameter(Mandatory)][string]$Source,
        [Parameter(Mandatory)][string]$Dest,
        [string[]]$Prune = @()
    )
    if (-not (Test-Path -LiteralPath $Source)) { throw "源目录不存在: $Source" }
    $src = (Resolve-Path -LiteralPath $Source).Path.TrimEnd('\')
    New-Item -ItemType Directory -Force -Path $Dest | Out-Null

    $copied = 0; $copiedBytes = 0L; $dropped = 0; $droppedBytes = 0L
    foreach ($f in (Get-ChildItem -LiteralPath $src -Recurse -File -Force -ErrorAction SilentlyContinue)) {
        $rel = $f.FullName.Substring($src.Length + 1)
        $relSlash = $rel -replace '\\', '/'
        if ($Prune.Count -gt 0 -and (Test-Pruned $relSlash $Prune)) {
            $dropped++; $droppedBytes += $f.Length; continue
        }
        $target = Join-Path $Dest $rel
        $dir = Split-Path -Parent $target
        if ($dir -and -not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
        Copy-Item -LiteralPath $f.FullName -Destination $target -Force
        $copied++; $copiedBytes += $f.Length
    }
    Say ("    拷贝 {0} 个文件 ({1})" -f $copied, (Format-Size $copiedBytes)) 'DarkGray'
    if ($dropped -gt 0) {
        Say ("    裁剪 {0} 个文件，省下 {1}" -f $dropped, (Format-Size $droppedBytes)) 'DarkGray'
    }
    return [pscustomobject]@{ Copied = $copied; Bytes = $copiedBytes; Dropped = $dropped; DroppedBytes = $droppedBytes }
}

# ---------------------------------------------------------------------------
#  各 kind 的实现
# ---------------------------------------------------------------------------

function Build-FlutterRelease {
    Step '构建 Flutter 桌面版（Release）'
    $env:PUB_HOSTED_URL = 'https://pub.dev'
    $env:FLUTTER_SUPPRESS_ANALYTICS = 'true'
    if (-not (Get-Command flutter -ErrorAction SilentlyContinue)) {
        throw 'PATH 里找不到 flutter。'
    }
    Push-Location $ProjectRoot
    try {
        & flutter build windows --release
        if ($LASTEXITCODE -ne 0) { throw "flutter build windows --release 失败（退出码 $LASTEXITCODE）" }
    } finally { Pop-Location }
}

function Get-FlutterReleaseDir {
    <#  定位 flutter build windows --release 的输出目录（各 Flutter 版本层级略有差异，兜底搜一次）  #>
    foreach ($rel in @('build\windows\x64\runner\Release', 'build\windows\runner\Release')) {
        $dir = Join-Path $ProjectRoot $rel
        if (Test-Path (Join-Path $dir 'viewer.exe')) { return $dir }
    }
    $exe = Get-ChildItem (Join-Path $ProjectRoot 'build\windows') -Filter 'viewer.exe' -Recurse -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -like '*\Release\*' } |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if ($exe) { return $exe.DirectoryName }
    return $null
}

# （2026-09-30：Build-ServerDist / Get-ServerDistDir / Install-Jre 已删。
# Kotlin 后端与 JRE 随 P4 收尾退役，Rust 后端是静态链接单文件 exe，构建与发布包都不需要 JVM。）

function Get-RustReleaseExe {
    $exe = Join-Path $ProjectRoot 'server-rs\target\release\comfyhub-server-rs.exe'
    if (Test-Path $exe) { return $exe }
    return $null
}

function Build-RustRelease {
    <#  后端现在是 Rust 二进制（P4）：`cargo build --release`。

        **构建新鲜度优先**：源码比 exe 新才编 —— 与 `scripts\backend.ps1` 的
        `Get-RustBuildFreshness` 同一条判据，免得每打一次包都白等几分钟。  #>
    $exe = Get-RustReleaseExe
    $srcDir = Join-Path $ProjectRoot 'server-rs\src'
    $need = $true
    if ($exe -and (Test-Path $srcDir)) {
        $exeTime = (Get-Item -LiteralPath $exe).LastWriteTime
        $newest = Get-ChildItem -Path $srcDir -Recurse -Filter *.rs -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending | Select-Object -First 1
        $cargoToml = Join-Path $ProjectRoot 'server-rs\Cargo.toml'
        $tomlTime = if (Test-Path $cargoToml) { (Get-Item -LiteralPath $cargoToml).LastWriteTime } else { $null }
        $need = ($newest -and $newest.LastWriteTime -gt $exeTime) -or ($tomlTime -and $tomlTime -gt $exeTime)
    }
    if (-not $need) {
        Say '  Rust 二进制是最新的（源码没有更新），跳过 cargo build' 'DarkGray'
        return
    }
    Step '构建 Rust 后端（cargo build --release）'
    Push-Location $ProjectRoot
    try {
        & cargo build --release --manifest-path (Join-Path $ProjectRoot 'server-rs\Cargo.toml')
        if ($LASTEXITCODE -ne 0) { throw "cargo build --release 失败（退出码 $LASTEXITCODE）" }
    } finally { Pop-Location }
    Say '  构建完成' 'DarkGray'
}

# （2026-09-30：Install-Jre 已删。Rust 后端是静态链接单文件 exe，不需要 JVM；
# Android 构建走 AGP 内建 Kotlin，见 docs 里那篇迁移说明。）

# ---------------------------------------------------------------------------
#  主流程
# ---------------------------------------------------------------------------

Say ''
Say '  ComfyHub 发布包打包' 'White'
Say '  ────────────────────────────────────────────────────────────'

if (-not (Test-Path -LiteralPath $ManifestFile)) { throw "找不到打包清单: $ManifestFile" }
$manifest = Get-Content -LiteralPath $ManifestFile -Raw -Encoding utf8 | ConvertFrom-Json
Say ("  清单: {0}  ({1} 个组件)" -f $ManifestFile.Replace($ProjectRoot + '\', ''), $manifest.components.Count) 'DarkGray'
Say "  输出: $OutDir" 'DarkGray'

# -Clean 只清掉"我们自己装进去的东西"。
# 默认输出目录就是 flutter 的产物目录，整个 Remove-Item 会把 viewer.exe /
# flutter_windows.dll 一起删掉 —— 那不叫清理，那叫把 App 弄没了。
# 运行期数据（.db 库、storage 产物、.run 日志）**不动**：那是用户的数据，不是装配产物。
if ($Clean) {
    Say '  清理上次装进去的东西（Flutter 自己的产物保留不动）…' 'DarkGray'
    $owned = New-Object System.Collections.ArrayList
    foreach ($c in $manifest.components) {
        if ($c.target -and $c.target -ne '.') { [void]$owned.Add([string]$c.target) }
    }
    [void]$owned.Add('packaging')
    [void]$owned.Add('BUILD-INFO.txt')
    # 2026-09-29：MySQL 已彻底退役（`mysql` 组件早就不在清单里了）。
    # 这里仍显式补一刀清 `mysql` —— 老发布目录里可能还留着那 300MB，
    # 只按清单清会把它永久留下。`.mysql` 是实例目录，**不**在这里清（它属于运行期数据）。
    [void]$owned.Add('mysql')
    # 2026-09-30：后端从 `<根>\server\` 挪到了 `<根>\backend\`（App 自己的资产），
    # `jre\` 随 Kotlin 后端一起退役。老发布目录里这两样都还在，只按清单清会永久留下
    # —— 而 `server\comfyhub-server-rs.exe` 正好是**旧的**那一份，留着会让人以为
    # "后端在这儿"（`backend_paths.dart` 还会把它当兼容候选）。这里顺手清掉。
    [void]$owned.Add('server')
    [void]$owned.Add('jre')
    foreach ($e in ($owned | Select-Object -Unique)) {
        $p = Join-Path $OutDir $e
        if (Test-Path -LiteralPath $p) {
            Remove-Item -LiteralPath $p -Recurse -Force -ErrorAction SilentlyContinue
            Say "    已删 $e" 'DarkGray'
        }
    }
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$skipped = @{ app = $SkipApp; backend = $SkipBackend }
$results = @()

foreach ($c in $manifest.components) {
    if ($skipped[$c.id]) { Say ''; Say "==> [$($c.id)] $($c.title) — 已按要求跳过" 'DarkGray'; continue }

    Step "[$($c.id)] $($c.title)"
    $target = Join-Path $OutDir $c.target
    try {
        switch ($c.kind) {
            'flutter_release' {
                Build-FlutterRelease
                $src = Get-FlutterReleaseDir
                if (-not $src) { throw '找不到 Release 产物（viewer.exe），flutter 构建可能没成功。' }
                Say "  来源: $($src.Replace($ProjectRoot + '\', ''))" 'DarkGray'

                # 默认输出目录**就是** flutter 的产物目录 —— 源和目标是同一个目录，
                # 不能自己往自己身上拷（会报 "Cannot copy item to itself"）。
                $srcFull = [System.IO.Path]::GetFullPath($src).TrimEnd('\')
                $dstFull = [System.IO.Path]::GetFullPath($target).TrimEnd('\')
                # 发布包里只装程序：上一份构建残留的运行期数据（`.db` / `storage` /
                # `.run` / `app-support`）**不许跟着进包** —— 2026-11-10 用户报的
                # "复制出去的构建对话不见了、角色卡是旧版"就是它们引起的（那个目录一带
                # 出去，它自己就成了运行时根，App 照着里面那份旧库开）。
                # ⚠ 只是**不拷**：源目录里一个字节都不删（那份数据可能是用户唯一的旧库，
                # 见 bundle_backend.cmake ④ 的实测）。
                $runtimeJunk = @('.db/**', 'storage/**', '.run/**', 'app-support/**', '_leftover-data/**')
                if ($srcFull -eq $dstFull) {
                    Say '    就地装配：Flutter 产物本来就在输出目录里，跳过拷贝' 'DarkGray'
                    Say '    运行期数据在装配时已经挪进 _leftover-data\，不进包也不删' 'DarkGray'
                    $results += [pscustomobject]@{ id = $c.id; title = $c.title; size = (Get-DirSize $src); dropped = 0 }
                } else {
                    $r = Copy-Tree -Source $src -Dest $target -Prune $runtimeJunk
                    $results += [pscustomobject]@{ id = $c.id; title = $c.title; size = $r.Bytes; dropped = $r.DroppedBytes }
                }
            }
            'rust_binary' {
                # P4（2026-09-26）起后端就是**一个 exe**：SQLite 引擎编在里面，
                # 所以这里既没有 installDist 也没有 jre —— 少了约 40MB（jre）+ 20MB（jar）。
                Build-RustRelease
                $exe = Get-RustReleaseExe
                if (-not $exe) {
                    throw '找不到 Rust 后端二进制。构建：cargo build --release --manifest-path server-rs\Cargo.toml'
                }
                Say "  来源: $($exe.Replace($ProjectRoot + '\', ''))" 'DarkGray'
                New-Item -ItemType Directory -Force -Path $target | Out-Null
                $dest = Join-Path $target 'comfyhub-server-rs.exe'
                Copy-Item -LiteralPath $exe -Destination $dest -Force
                $size = (Get-Item -LiteralPath $dest).Length
                Say ("    已装 backend\{0}  ({1:N1} MB)" -f 'comfyhub-server-rs.exe', ($size / 1MB)) 'DarkGray'
                $results += [pscustomobject]@{ id = $c.id; title = $c.title; size = $size; dropped = 0 }
            }
            'mysql_portable' {
                # P2（2026-09-23）起清单里已经没有这个组件（数据库换成 SQLite）。
                # 分支保留成一句明确报错：万一有人把清单改回去，这里要能说清为什么不该有。
                throw '清单里不该再有 mysql_portable：2026-09-29 起 MySQL 已彻底退役，运行期数据库是 SQLite 单文件，发布包不携带任何数据库本体。'
            }
            'copy_tree' {
                $src = Join-Path $ProjectRoot $c.source
                $r = Copy-Tree -Source $src -Dest $target
                $results += [pscustomobject]@{ id = $c.id; title = $c.title; size = $r.Bytes; dropped = $r.DroppedBytes }
            }
            'copy_file' {
                $src = Join-Path $ProjectRoot $c.source
                if (-not (Test-Path -LiteralPath $src)) { Warn "源文件不存在，跳过: $($c.source)"; break }
                Copy-Item -LiteralPath $src -Destination $target -Force
                Say '    已拷贝' 'DarkGray'
                $results += [pscustomobject]@{ id = $c.id; title = $c.title; size = (Get-Item -LiteralPath $target).Length; dropped = 0 }
            }
            default { throw "清单里有不认识的 kind: $($c.kind)" }
        }
    } catch {
        if ($c.required) { Fail "[$($c.id)] 失败: $($_.Exception.Message)" }
        else { Warn "[$($c.id)] 跳过: $($_.Exception.Message)" }
    }
}

# 把清单本身也放进发布包：拿到包的人能一眼看出里面装了什么
$manifestOut = Join-Path $OutDir 'packaging'
New-Item -ItemType Directory -Force -Path $manifestOut | Out-Null
Copy-Item -LiteralPath $ManifestFile -Destination (Join-Path $manifestOut 'manifest.json') -Force

# ---------------------------------------------------------------------------
#  自检
# ---------------------------------------------------------------------------

Step '自检（发布包里的关键文件）'

$requiredFiles = @(
    @{ p = 'viewer.exe';                          what = 'Flutter App' },
    @{ p = 'scripts\backend.ps1';                 what = '后端找/编/起/停（e2e 与 dev 用）' },
    @{ p = 'scripts\video-poster.ps1';            what = '视频封面抽帧（后端运行期依赖）' },
    # P4（2026-09-26）起后端是**这一个 exe**（SQLite 引擎编在里面，viewer.exe 同级 backend\）。
    @{ p = 'backend\comfyhub-server-rs.exe';      what = 'Rust 后端二进制' },
    @{ p = 'db\schema.sqlite.sql';                what = 'SQLite 库表结构（P2 起的真源）' }
)


$missing = @()
foreach ($f in $requiredFiles) {
    $full = Join-Path $OutDir $f.p
    if (Test-Path -LiteralPath $full) {
        Say ("  {0,-34} OK" -f $f.p) 'Green'
    } else {
        Say ("  {0,-34} 缺失  ({1})" -f $f.p, $f.what) 'Red'
        $missing += $f.p
    }
}



# 可写目录要预先建好，但里面不留东西（P2 起数据库是 .db\ 下的 SQLite 单文件）
foreach ($d in @('.db', 'storage', '.run')) {
    New-Item -ItemType Directory -Force -Path (Join-Path $OutDir $d) | Out-Null
}

# ---------------------------------------------------------------------------
#  构建信息 + 汇总
# ---------------------------------------------------------------------------

$totalSize = Get-DirSize $OutDir

# 运行时依赖：bundled 的随包带走了，其余要目标机器自己装。
# 直接读清单，避免"清单改了、说明没改"。
$reqLines = ''
if ($manifest.runtimeRequirements -and $manifest.runtimeRequirements.required) {
    $reqLines = "`n运行时依赖（清单 packaging\manifest.json 的 runtimeRequirements）:`n"
    foreach ($r in $manifest.runtimeRequirements.required) {
        $mark = if ($r.bundled) { "[随包携带]" } else { "[需自行安装]" }
        $loc = if ($r.location) { " -> $($r.location)\" } else { '' }
        $reqLines += "  $mark $($r.name)$loc`n"
        if ($r.why) { $reqLines += "            为什么: $($r.why)`n" }
        if (-not $r.bundled -and $r.install) { $reqLines += "            安装:   $($r.install)`n" }
    }
}

$flutterRelDir = Join-Path $ProjectRoot 'build\windows\x64\runner\Release'
$inPlaceNote = ''
if ([System.IO.Path]::GetFullPath($OutDir).TrimEnd('\') -eq [System.IO.Path]::GetFullPath($flutterRelDir).TrimEnd('\')) {
    $inPlaceNote = @"

注意       : 本目录就是 flutter build 的产物目录（默认就地装配）。
             跑 flutter clean / 重新构建会把它整个清空，后端会一起没，
             那时重新执行一次 scripts\pack-release.ps1 即可。
"@
}

$buildInfo = @"
ComfyHub 发布包
打包时间   : $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')
清单       : packaging\manifest.json
包大小     : $(Format-Size $totalSize)
位置       : $OutDir
运行时布局 : 便携式 —— 整个目录即根目录，可整体移动，免安装、免管理员
$reqLines
可写数据（都在根目录内）:
  .db\        数据库（SQLite 单文件 comfyhub.db + 它自己的 -wal / -shm）
  storage\    生成产物（App 直启后端时按 --storage 显式传入）
  .run\       后端日志 / PID

怎么用:
  1. 双击 viewer.exe —— 它会自动把 backend\ 里的后端拉起来（首次启动会自己建库，稍等一会儿）
  2. 用命令行看后端状态：pwsh -File scripts\backend.ps1 status
  3. 备份数据库: 停掉后端，把 .db 整个目录拷走（-wal / -shm 要一起）
$inPlaceNote
"@
Set-Content -Path (Join-Path $OutDir 'BUILD-INFO.txt') -Value $buildInfo -Encoding utf8

Step '完成情况'
foreach ($r in $results) {
    Say ("  {0,-10} {1,-44} {2,10}" -f $r.id, $r.title, (Format-Size $r.size)) 'Gray'
}
Say ("  {0,-10} {1,-44} {2,10}" -f '', '合计', (Format-Size $totalSize)) 'White'
Say ''
Say "  发布包: $OutDir"

if ($Zip) {
    $zipPath = "$OutDir.zip"
    Step "打包成 zip"
    if (Test-Path -LiteralPath $zipPath) { Remove-Item -LiteralPath $zipPath -Force }
    Compress-Archive -Path (Join-Path $OutDir '*') -DestinationPath $zipPath -CompressionLevel Optimal
    Say ("  已生成: {0} ({1})" -f $zipPath, (Format-Size (Get-Item -LiteralPath $zipPath).Length)) 'Green'
}

Say ''
if ($Warnings.Count -gt 0) {
    Say '  警告:' 'Yellow'
    foreach ($w in $Warnings) { Say "    · $w" 'Yellow' }
}
if ($missing.Count -gt 0 -or $Failures.Count -gt 0) {
    Say '  打包未通过自检，发布包不完整。' 'Red'
    foreach ($m in $missing) { Say "    · 缺文件: $m" 'Red' }
    foreach ($f in $Failures) { Say "    · $f" 'Red' }
    exit 1
}
Say '  发布包装配完成，自检全部通过。' 'Green'
Say ''
exit 0
