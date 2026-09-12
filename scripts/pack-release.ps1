<#
.SYNOPSIS
    ComfyHub 发布包打包：把 Flutter App + Kotlin 后端 + 便携版 MySQL + JRE 装配成一个可以直接给别人的目录。

.DESCRIPTION
    之前「出正式产物」只有 scripts\autorun-app.ps1：它只跑 flutter build windows --release，
    产物就是 build\windows\x64\runner\Release\ ——里面**只有 App**，缺两样要紧的东西：

      · Kotlin 后端：在 server\build\install\ 下，是 Gradle 的构建中间产物，不会跟着 Release 目录走；
      · MySQL：mysqld.exe 只在 D:\tools\mysql 这种**本机绝对路径**下找，换台机器直接崩。

    结果就是"把 Release 目录拷给别人"跑不起来。本脚本按 packaging\manifest.json 这份
    **软件打包清单**把它们装配到同一个目录里。

    **默认就地装配**：输出目录就是 flutter build 的产物目录
    `build\windows\x64\runner\Release\` —— 装完之后这个目录本身就是完整可运行的发布包，
    直接双击里面的 viewer.exe 就能用，也可以整个目录拷给别人。

      · 想另存一份干净的副本（比如打成 zip 发出去）用 -OutDir 指个别的地方；
      · ⚠ `flutter clean` / 重新 clone 会把这个目录整个清掉，那样就只剩 App 了，
        需要重新跑一次本脚本。

    发布包是便携式的（免安装、免管理员）：整个目录就是运行时根目录，可整体搬走。
    可写数据都在根目录内：
      · 数据库实例  <根>\.mysql     （可用 mysql.ps1 move / -DataDir 搬走）
      · 生成产物    <根>\storage    （后端按 COMFYHUB_STORAGE 写这里）
      · 日志 / PID  <根>\.run
    只读的运行时件：
      · 后端        <根>\server     （gradle installDist 的 bin\ + lib\）
      · MySQL       <根>\mysql
      · JRE         <根>\jre        （后端是 JVM 程序，目标机器不假设装了 JDK）
      · 脚本        <根>\scripts    （App 启动时调它拉起服务）
      · SQL         <根>\db         （首次初始化要导入 schema.sql）

.EXAMPLE
    pwsh -File scripts\pack-release.ps1
    pwsh -File scripts\pack-release.ps1 -Zip
    pwsh -File scripts\pack-release.ps1 -SkipApp -SkipJre -Clean      # 只重装后端 + MySQL，快速迭代
    pwsh -File scripts\pack-release.ps1 -OutDir 'D:\dist\ComfyHub'    # 另存一份干净的独立副本
#>
[CmdletBinding()]
param(
    # 发布包输出目录。默认**就地**装配进 flutter build 的产物目录
    # build\windows\x64\runner\Release（装完那个目录就是完整发布包）
    [string]$OutDir,

    # 装配完再打一个 zip（几百 MB，会慢一会儿）
    [switch]$Zip,

    # 跳过某几件（迭代时用：MySQL / JRE 一次装好后不用每次重拷）
    [switch]$SkipApp,
    [switch]$SkipBackend,
    [switch]$SkipMysql,
    [switch]$SkipJre,

    # 不裁剪（MySQL 全量拷 = 约 990MB；默认裁剪后约 400MB）
    [switch]$NoPrune,

    # 装配前清空输出目录（默认只增量覆盖；改裁剪规则后建议带上）
    [switch]$Clean,

    # 显式指定工具链位置（默认按 scripts\server.ps1 / mysql.ps1 同一套顺序解析）
    [string]$JdkHome = $env:COMFYHUB_JDK_HOME,
    [string]$MySqlHome = $env:COMFYHUB_MYSQL_HOME
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
#  工具链解析
#  与 scripts\server.ps1 的 Resolve-Jdk、scripts\mysql.ps1 的 Resolve-MySqlHome
#  保持同一套顺序（本项目刻意不引共享模块，见 mysql.ps1 顶部的说明）。
#  差别只有一处：**发布包内自带的 <根>\jre、<根>\mysql 优先**，
#  这样脚本在装配好的发布包里跑时用的就是包里那份，而不是开发机的 D:\tools。
# ---------------------------------------------------------------------------

function Resolve-Jdk {
    param([string]$Explicit)

    function Test-Jdk([string]$javaHome) {
        if (-not $javaHome) { return $null }
        $java = Join-Path $javaHome 'bin\java.exe'
        if (-not (Test-Path -LiteralPath $java)) { return $null }
        $out = & $java -version 2>&1 | Out-String
        if ($out -match 'version "(\d+)') {
            $major = [int]$Matches[1]
            # 后端是 Kotlin/JVM 21 目标，Gradle 8.12 也要 21~23
            if ($major -ge 21 -and $major -le 23) { return $javaHome }
        }
        return $null
    }

    foreach ($cand in @(
            $Explicit,
            $env:COMFYHUB_JDK_HOME,
            (Join-Path $ProjectRoot 'jre'),          # 发布包自带的运行时
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
    return $null
}

function Resolve-MySqlHome {
    param([string]$Explicit)

    if ($Explicit -and (Test-Path (Join-Path $Explicit 'bin\mysqld.exe'))) { return $Explicit }

    $candidates = @(
        (Join-Path $ProjectRoot 'mysql'),        # 发布包自带的便携版
        'D:\tools\mysql\mysql-8.4.3-winx64',
        'C:\tools\mysql\mysql-8.4.3-winx64'
    ) + @(Get-ChildItem -Path 'D:\tools\mysql', 'C:\tools\mysql' -Directory -ErrorAction SilentlyContinue |
            ForEach-Object { $_.FullName })

    foreach ($c in $candidates) {
        if ($c -and (Test-Path (Join-Path $c 'bin\mysqld.exe'))) { return $c }
    }
    return $null
}

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

function Build-ServerDist {
    <#  gradle installDist → server\build\install\comfy-hub-server\ （含 bin\ + lib\）  #>
    Step '构建 Kotlin 后端（gradle installDist）'
    # 必须显式用挑出来的 JDK 21~23 跑 Gradle：Gradle 8.12 不认 JDK 24/25，
    # 而本机 JAVA_HOME / PATH 上就是 JDK 25（server.ps1 同样要先钉住 $env:JAVA_HOME）。
    # 不钉的话会以 "What went wrong: 25.0.1" 这种没头没尾的方式失败。
    $jdk = Resolve-Jdk -Explicit $JdkHome
    if (-not $jdk) { throw '找不到可用的 JDK 21~23，无法构建后端。用 -JdkHome 指定。' }
    $env:JAVA_HOME = $jdk
    $env:PATH = "$jdk\bin;$env:PATH"
    Say "  使用 JDK: $jdk" 'DarkGray'
    Push-Location (Join-Path $ProjectRoot 'server')
    try {
        & gradle -p (Join-Path $ProjectRoot 'server') --console=plain installDist
        if ($LASTEXITCODE -ne 0) { throw "gradle installDist 失败（退出码 $LASTEXITCODE）" }
    } finally { Pop-Location }
}

function Get-ServerDistDir {
    # installDist 出来的目录名 = settings.gradle.kts 里的 rootProject.name
    $base = Join-Path $ProjectRoot 'server\build\install'
    if (-not (Test-Path $base)) { return $null }
    foreach ($d in (Get-ChildItem $base -Directory -ErrorAction SilentlyContinue)) {
        if (Test-Path (Join-Path $d.FullName 'bin\comfy-hub-server.bat')) { return $d.FullName }
    }
    return $null
}

function Install-Jre {
    <#  <jdk> → <OutDir>\jre。有 jmods 就 jlink 出精简运行时，否则整份拷（剪掉开发用的东西）。  #>
    param([string]$Target)

    $jdk = Resolve-Jdk -Explicit $JdkHome
    if (-not $jdk) {
        # 本机连 JDK 都没有 → 借 ensure-runtime.ps1 的自动下载能力拿一份便携版，
        # 免得"想打个包还得先自己装 JDK"。
        $runtimeHelper = Join-Path $PSScriptRoot 'runtime-deps.ps1'
        if (Test-Path $runtimeHelper) {
            if (-not (Get-Command Ensure-JavaRuntime -ErrorAction SilentlyContinue)) { . $runtimeHelper }
            Say '  本机找不到 JDK 21~23，自动下载一份便携版运行时…' 'Yellow'
            $javaExe = Ensure-JavaRuntime -ProjectRoot $ProjectRoot
            # Ensure-JavaRuntime 返回的是 ...\jre\bin\java.exe，这里要的是运行时根目录
            if ($javaExe) { $jdk = Split-Path -Parent (Split-Path -Parent $javaExe) }
        }
    }
    if (-not $jdk) { throw '找不到可用的 JDK 21~23，无法准备 JRE。用 -JdkHome 指定，或先跑 scripts\ensure-runtime.ps1。' }
    Say "  来源 JDK: $jdk" 'DarkGray'

    $jmods = Join-Path $jdk 'jmods'
    if ((Test-Path $jmods) -and (Get-Command (Join-Path $jdk 'bin\jlink.exe') -ErrorAction SilentlyContinue)) {
        Say '  用 jlink 生成精简运行时…' 'DarkGray'
        if (Test-Path -LiteralPath $Target) { Remove-Item -LiteralPath $Target -Recurse -Force }
        # java.se 覆盖 java.sql / java.naming / java.management / java.desktop 等标准模块；
        # jdk.unsupported 是 Netty / HikariCP 要的 sun.misc.Unsafe；jdk.crypto.ec 是 TLS 的 EC 套件。
        $mods = 'java.se,jdk.unsupported,jdk.crypto.ec'
        foreach ($compress in @('zip-6', '2', $null)) {
            $args = @('--add-modules', $mods, '--strip-debug', '--no-man-pages', '--no-header-files', '--output', $Target)
            if ($compress) { $args += "--compress=$compress" }
            & (Join-Path $jdk 'bin\jlink.exe') @args
            if ($LASTEXITCODE -eq 0) { return }
            Say "  jlink 失败（--compress=$compress），换个参数重试…" 'Yellow'
            if (Test-Path -LiteralPath $Target) { Remove-Item -LiteralPath $Target -Recurse -Force }
        }
        Warn 'jlink 一直失败，退化为整份拷贝 JRE。'
    } else {
        Say '  该 JDK 没有 jmods（Android Studio 的 JBR 就是这样），整份拷贝 JRE。' 'DarkGray'
    }

    # 整份拷贝：开发时才用的东西（jmods / include / demo / src.zip / 调试符号）都剪掉
    $prune = @('jmods/**', 'include/**', 'demo/**', 'sample/**', 'lib/src.zip', '**/*.pdb', '**/*.map')
    Copy-Tree -Source $jdk -Dest $Target -Prune $(if ($NoPrune) { @() } else { $prune }) | Out-Null
}

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
# 运行期数据（.mysql 库、storage 产物、.run 日志）**不动**：那是用户的数据，不是装配产物。
if ($Clean) {
    Say '  清理上次装进去的东西（Flutter 自己的产物保留不动）…' 'DarkGray'
    $owned = New-Object System.Collections.ArrayList
    foreach ($c in $manifest.components) {
        if ($c.target -and $c.target -ne '.') { [void]$owned.Add([string]$c.target) }
    }
    [void]$owned.Add('packaging')
    [void]$owned.Add('BUILD-INFO.txt')
    foreach ($e in ($owned | Select-Object -Unique)) {
        $p = Join-Path $OutDir $e
        if (Test-Path -LiteralPath $p) {
            Remove-Item -LiteralPath $p -Recurse -Force -ErrorAction SilentlyContinue
            Say "    已删 $e" 'DarkGray'
        }
    }
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$skipped = @{ app = $SkipApp; backend = $SkipBackend; mysql = $SkipMysql; jre = $SkipJre }
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
                if ($srcFull -eq $dstFull) {
                    Say '    就地装配：Flutter 产物本来就在输出目录里，跳过拷贝' 'DarkGray'
                    $results += [pscustomobject]@{ id = $c.id; title = $c.title; size = (Get-DirSize $src); dropped = 0 }
                } else {
                    $r = Copy-Tree -Source $src -Dest $target
                    $results += [pscustomobject]@{ id = $c.id; title = $c.title; size = $r.Bytes; dropped = $r.DroppedBytes }
                }
            }
            'gradle_install_dist' {
                Build-ServerDist
                $src = Get-ServerDistDir
                if (-not $src) { throw '找不到 installDist 产物（server\build\install\*\bin\comfy-hub-server.bat）。' }
                Say "  来源: $($src.Replace($ProjectRoot + '\', ''))" 'DarkGray'
                $r = Copy-Tree -Source $src -Dest $target
                $results += [pscustomobject]@{ id = $c.id; title = $c.title; size = $r.Bytes; dropped = $r.DroppedBytes }
            }
            'jdk_runtime' {
                if (-not $NoPrune -and $c.prune) { } # jdk_runtime 的裁剪规则写在 Install-Jre 里
                Install-Jre -Target $target
                $results += [pscustomobject]@{ id = $c.id; title = $c.title; size = (Get-DirSize $target); dropped = 0 }
            }
            'mysql_portable' {
                # 别把变量起名 $home：PowerShell 里 $HOME 是只读自动变量（大小写不敏感），
                # 赋值会直接抛 "Cannot overwrite variable HOME because it is read-only or constant"。
                $mysqlSrc = Resolve-MySqlHome -Explicit $MySqlHome
                if (-not $mysqlSrc) { throw '找不到便携版 MySQL。用 -MySqlHome / COMFYHUB_MYSQL_HOME 指定解压目录。' }
                Say "  来源: $mysqlSrc" 'DarkGray'
                $prune = if ($NoPrune) { @() } else { @($c.prune) }
                $r = Copy-Tree -Source $mysqlSrc -Dest $target -Prune $prune
                $results += [pscustomobject]@{ id = $c.id; title = $c.title; size = $r.Bytes; dropped = $r.DroppedBytes }
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
    @{ p = 'scripts\comfyhub.ps1';                what = '统一入口脚本' },
    @{ p = 'scripts\mysql.ps1';                   what = 'MySQL 脚本' },
    @{ p = 'scripts\server.ps1';                  what = '后端脚本' },
    @{ p = 'server\bin\comfy-hub-server.bat';     what = 'Kotlin 后端启动脚本' },
    @{ p = 'mysql\bin\mysqld.exe';                what = 'MySQL 服务端' },
    @{ p = 'jre\bin\java.exe';                    what = 'Java 运行时' },
    @{ p = 'db\schema.sql';                       what = '库表结构 SQL' }
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

# 后端 jar 至少得有一个，否则 bin\*.bat 跑起来会 ClassNotFound
$jarCount = @(Get-ChildItem (Join-Path $OutDir 'server\lib') -Filter '*.jar' -ErrorAction SilentlyContinue).Count
if ($jarCount -gt 0) { Say ("  {0,-34} OK ({1} 个 jar)" -f 'server\lib\*.jar', $jarCount) 'Green' }
else { Say ("  {0,-34} 缺失" -f 'server\lib\*.jar') 'Red'; $missing += 'server\lib\*.jar' }

# 可写目录要预先建好，但里面不留东西
foreach ($d in @('.mysql', 'storage', '.run')) {
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
             跑 flutter clean / 重新构建会把它整个清空，后端、MySQL、JRE 会一起没，
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
  .mysql\     数据库实例（data\ + my.ini + 日志）
  storage\    生成产物（后端按 COMFYHUB_STORAGE 写入）
  .run\       后端日志 / PID

怎么用:
  1. 双击 viewer.exe —— 它会自动把 MySQL + 后端拉起来（首次会自动初始化数据库，稍等一会儿）
  2. 或命令行: pwsh -File scripts\comfyhub.ps1 up -WithApp
  3. 起不来先体检: pwsh -File scripts\comfyhub.ps1 doctor
  4. 换数据库存放位置: pwsh -File scripts\mysql.ps1 move -DataDir <新位置>
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
