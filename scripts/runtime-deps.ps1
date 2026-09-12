<#
.SYNOPSIS
    ComfyHub 运行时依赖体检：发布包/源码树跑起来到底还需要目标机器装什么。

.DESCRIPTION
    发布包（scripts\pack-release.ps1）只带了**两样**能带走的东西：
      · Java 运行时   <根>\jre        —— 后端是 Kotlin/JVM 程序
      · 便携版 MySQL  <根>\mysql      —— 数据库本体
    这两样之外，还有几样**必须目标机器自己装**，缺一个服务就起不来，
    而失败现场往往只是一句没头没尾的报错（"mysqld 启动超时" / "不是内部或外部命令"）。
    这个文件把它们列清楚，并在启动失败时给出可操作的安装提示。

    本文件被 comfyhub.ps1 dot-source（跟 silent-process.ps1 同一套路，不做独立模块）。

    依赖清单：
      1. PowerShell 7 (pwsh)            —— 必需，且 **Windows 自带的 powershell.exe 5.1 不算数**
      2. VC++ 2015-2022 x64 可再发行组件 —— mysqld.exe 依赖 vcruntime140.dll / msvcp140.dll
      3. Java 21 运行时                  —— 发布包自带 <根>\jre；源码树里要装 JDK 21~23
      4. MySQL 本体                      —— 发布包自带 <根>\mysql；源码树里靠 COMFYHUB_MYSQL_HOME
#>

function Test-VcRuntime {
    <#
      mysqld.exe 是 MSVC 编的，缺 vcruntime140.dll / msvcp140.dll 会**直接起不来**，
      而且报错往往落在 mysql-error.log 里，命令行上只看到"启动超时"。
      优先看 mysqld.exe 旁边（有些发行版会把 DLL 放在 bin\ 下），再退回 System32。
    #>
    param([string]$MySqlBinDir)

    foreach ($dll in @('vcruntime140.dll', 'vcruntime140_1.dll', 'msvcp140.dll')) {
        $found = $false
        if ($MySqlBinDir) { $found = Test-Path -LiteralPath (Join-Path $MySqlBinDir $dll) }
        if (-not $found) { $found = Test-Path -LiteralPath (Join-Path $env:SystemRoot "System32\$dll") }
        if (-not $found) { return $false }
    }
    return $true
}

function Get-RuntimeChecks {
    <#
      返回一组体检项：Name / Ok / Detail / Why / Hint。
      Hint 只在 Ok=$false 时展示 —— 就是"该装什么"。
    #>
    param(
        [Parameter(Mandatory)][string]$ProjectRoot,
        [string]$MySqlBinDir,
        [string]$MySqlHome,
        [string]$Jdk
    )

    $checks = @()

    # ---- 1) PowerShell 7 -------------------------------------------------
    # 脚本内部到处是 `& pwsh -NoProfile -File xxx.ps1`（comfyhub.ps1 / mysql.ps1 /
    # server.ps1 互相调用），Windows 自带的 powershell.exe(5.1) 满足不了这些调用。
    $pwshCmd = Get-Command pwsh -ErrorAction SilentlyContinue
    $checks += [pscustomobject]@{
        Name   = 'PowerShell 7 (pwsh)'
        Ok     = [bool]$pwshCmd
        Detail = $(if ($pwshCmd) { $pwshCmd.Source } else { 'PATH 里找不到 pwsh' })
        Why    = 'App 用它执行 scripts\comfyhub.ps1；脚本之间也互相调 pwsh（Windows 自带的 5.1 不够）'
        Hint   = 'winget install --id Microsoft.PowerShell --source winget    （或 https://aka.ms/powershell）'
    }

    # ---- 2) VC++ 运行时（mysqld 的硬依赖）--------------------------------
    $vcOk = Test-VcRuntime -MySqlBinDir $MySqlBinDir
    $checks += [pscustomobject]@{
        Name   = 'VC++ 2015-2022 x64 运行时'
        Ok     = $vcOk
        Detail = $(if ($vcOk) { 'vcruntime140 / msvcp140 都在' } else { '缺 vcruntime140.dll / msvcp140.dll' })
        Why    = 'mysqld.exe 是 MSVC 编的，缺这几个 DLL 会直接起不来（错误只写在 mysql-error.log 里）'
        Hint   = '装 "Microsoft Visual C++ 2015-2022 Redistributable (x64)"：winget install --id Microsoft.VCRedist.2015+.x64'
    }

    # ---- 3) Java 运行时 --------------------------------------------------
    # 顺序：显式传入的 $Jdk → 发布包自带的 <根>\jre → PATH 上的 java → 本机常见 JDK 位置。
    # 这里刻意**只查存在性、不校验版本**：它的用途是"给出安装提示"，
    # 误报"缺 Java"比漏报更烦人，所以宁可放宽。
    $javaExe = $null
    if ($Jdk) {
        $candidate = Join-Path $Jdk 'bin\java.exe'
        if (Test-Path -LiteralPath $candidate) { $javaExe = $candidate }
    }
    if (-not $javaExe) {
        $packagedJre = Join-Path $ProjectRoot 'jre\bin\java.exe'
        if (Test-Path -LiteralPath $packagedJre) { $javaExe = $packagedJre }
    }
    if (-not $javaExe) {
        $onPath = Get-Command java -ErrorAction SilentlyContinue
        if ($onPath) { $javaExe = $onPath.Source }
    }
    if (-not $javaExe) {
        foreach ($cand in @(
                'D:\tools\jdk-21', 'D:\tools\jdk-22', 'D:\tools\jdk-23',
                "$env:ProgramFiles\Eclipse Adoptium\jdk-21*",
                "$env:ProgramFiles\Java\jdk-21*",
                "$env:ProgramFiles\Java\jdk-22*",
                "$env:ProgramFiles\Java\jdk-23*",
                'D:\Program Files\Android Studio\jbr',
                "$env:LOCALAPPDATA\Programs\Android Studio\jbr"
            )) {
            if (-not $cand) { continue }
            foreach ($p in (Resolve-Path $cand -ErrorAction SilentlyContinue)) {
                $j = Join-Path $p.Path 'bin\java.exe'
                if (Test-Path -LiteralPath $j) { $javaExe = $j; break }
            }
            if ($javaExe) { break }
        }
    }

    $javaOk = [bool]$javaExe
    $javaDetail = '缺 Java 运行时'
    if ($javaOk) {
        $javaDetail = $javaExe
        try {
            $ver = (& $javaExe -version 2>&1 | Out-String)
            if ($ver -match 'version "([^"]+)"') { $javaDetail = "$javaExe  (v$($Matches[1]))" }
        } catch { }
    }
    $checks += [pscustomobject]@{
        Name   = 'Java 21 运行时'
        Ok     = $javaOk
        Detail = $javaDetail
        Why    = 'Kotlin 后端跑在 JVM 上'
        Hint   = '发布包自带 <根>\jre；源码树里请装 JDK 21~23，或用 -JdkHome / COMFYHUB_JDK_HOME 指定'
    }

    # ---- 4) MySQL 本体 ---------------------------------------------------
    $mysqldOk = $MySqlBinDir -and (Test-Path -LiteralPath (Join-Path $MySqlBinDir 'mysqld.exe'))
    $checks += [pscustomobject]@{
        Name   = 'MySQL 服务端 (mysqld.exe)'
        Ok     = [bool]$mysqldOk
        Detail = $(if ($mysqldOk) { Join-Path $MySqlBinDir 'mysqld.exe' } else { '找不到 mysqld.exe' })
        Why    = '数据库本体，免安装但必须存在'
        Hint   = '发布包自带 <根>\mysql；源码树里设 COMFYHUB_MYSQL_HOME 指向 MySQL 解压目录'
    }

    return $checks
}

function Show-RuntimeReport {
    <#  体检表：Ok 的一行带过，缺的重点标出来  #>
    param(
        [Parameter(Mandatory)][string]$ProjectRoot,
        [string]$MySqlBinDir,
        [string]$MySqlHome,
        [string]$Jdk,
        [switch]$OnlyProblems
    )

    $checks = Get-RuntimeChecks -ProjectRoot $ProjectRoot -MySqlBinDir $MySqlBinDir -MySqlHome $MySqlHome -Jdk $Jdk
    $bad = @($checks | Where-Object { -not $_.Ok })

    foreach ($c in $checks) {
        if ($OnlyProblems -and $c.Ok) { continue }
        $mark = if ($c.Ok) { 'OK  ' } else { '缺失' }
        $color = if ($c.Ok) { 'Green' } else { 'Red' }
        Write-Host ("  {0,-28} {1}  {2}" -f $c.Name, $mark, $c.Detail) -ForegroundColor $color
    }

    if ($bad.Count -gt 0) {
        Write-Host ''
        Write-Host '  还缺这些运行时，服务起不来就是它们的原因：' -ForegroundColor Yellow
        foreach ($c in $bad) {
            Write-Host ("    · {0}" -f $c.Name) -ForegroundColor Yellow
            Write-Host ("        为什么要: {0}" -f $c.Why) -ForegroundColor DarkGray
            Write-Host ("        怎么装:   {0}" -f $c.Hint) -ForegroundColor Cyan
        }
    }
    return $checks
}

function Show-RuntimeFailureHints {
    <#
      启动失败时调用：只在**确实缺东西**的时候才刷提示，
      依赖齐全（纯粹是别的原因失败）就一句话都不说，免得误导。
    #>
    param(
        [Parameter(Mandatory)][string]$ProjectRoot,
        [string]$MySqlBinDir,
        [string]$MySqlHome,
        [string]$Jdk
    )

    $checks = Get-RuntimeChecks -ProjectRoot $ProjectRoot -MySqlBinDir $MySqlBinDir -MySqlHome $MySqlHome -Jdk $Jdk
    $bad = @($checks | Where-Object { -not $_.Ok })
    if ($bad.Count -eq 0) { return $false }

    Write-Host ''
    Write-Host '  启动失败，而且检测到运行时缺失 —— 先把下面这些装上再试：' -ForegroundColor Red
    foreach ($c in $bad) {
        Write-Host ("    · 缺 {0}" -f $c.Name) -ForegroundColor Red
        Write-Host ("        影响: {0}" -f $c.Why) -ForegroundColor DarkGray
        Write-Host ("        安装: {0}" -f $c.Hint) -ForegroundColor Cyan
    }
    Write-Host '  （只差日志的话看: pwsh -File scripts\comfyhub.ps1 logs）' -ForegroundColor DarkGray
    return $true
}

# ===========================================================================
#  自动获取运行时
#  Java 能"下载一个便携版塞进 <根>\jre"就解决（免安装、免管理员）；
#  pwsh / VC++ 运行时属于系统级安装，走 winget（没有 winget 就只给出提示）。
#  入口脚本：scripts\ensure-runtime.ps1
# ===========================================================================

function Initialize-Tls12 {
    <#
      Windows PowerShell 5.1 默认只开 TLS 1.0/1.1，直接下 HTTPS 会报
      "基础连接已经关闭: 未能为 SSL/TLS 建立安全通道"。
      （本文件要能在 5.1 下跑：pwsh 本身就是可能缺的那个运行时。）
    #>
    try {
        $cur = [Net.ServicePointManager]::SecurityProtocol
        [Net.ServicePointManager]::SecurityProtocol = $cur -bor [Net.SecurityProtocolType]::Tls12
    } catch { }
}

function Get-JavaRuntimePath {
    <#
      找"能跑后端的 java.exe"。顺序与 server.ps1 的 Resolve-Jdk 一致，
      但**不要求是完整 JDK**（运行后端只要有 JRE；Gradle 才需要 JDK）。
    #>
    param(
        [string]$ProjectRoot,
        [string]$Explicit
    )

    $cands = New-Object System.Collections.ArrayList
    if ($Explicit) { [void]$cands.Add((Join-Path $Explicit 'bin\java.exe')) }
    if ($env:COMFYHUB_JDK_HOME) { [void]$cands.Add((Join-Path $env:COMFYHUB_JDK_HOME 'bin\java.exe')) }
    # 发布包/自建自带的运行时
    if ($ProjectRoot) { [void]$cands.Add((Join-Path $ProjectRoot 'jre\bin\java.exe')) }
    [void]$cands.Add((Join-Path $env:LOCALAPPDATA 'ComfyHub\jre\bin\java.exe'))

    foreach ($base in @(
            'D:\tools\jdk-21', 'D:\tools\jdk-22', 'D:\tools\jdk-23',
            "$env:ProgramFiles\Eclipse Adoptium\jdk-21*",
            "$env:ProgramFiles\Java\jdk-21*",
            "$env:ProgramFiles\Java\jdk-22*",
            "$env:ProgramFiles\Java\jdk-23*",
            'D:\Program Files\Android Studio\jbr',
            "$env:LOCALAPPDATA\Programs\Android Studio\jbr"
        )) {
        if (-not $base) { continue }
        foreach ($p in (Resolve-Path $base -ErrorAction SilentlyContinue)) {
            [void]$cands.Add((Join-Path $p.Path 'bin\java.exe'))
        }
    }

    # PATH 上的 java 放最后：它可能是 JDK 25 那种"能启动但跑不了后端"的
    $onPath = Get-Command java -ErrorAction SilentlyContinue
    if ($onPath) { [void]$cands.Add($onPath.Source) }

    foreach ($c in $cands) {
        if ($c -and (Test-Path -LiteralPath $c)) { return $c }
    }
    return $null
}

function Get-JavaMajorVersion {
    param([string]$JavaExe)
    if (-not $JavaExe) { return $null }
    try {
        $out = (& $JavaExe -version 2>&1 | Out-String)
        if ($out -match 'version "(\d+)') { return [int]$Matches[1] }
    } catch { }
    return $null
}

function Test-DirWritable {
    param([string]$Dir)
    try {
        if (-not (Test-Path -LiteralPath $Dir)) { return $false }
        $probe = Join-Path $Dir (".comfyhub-write-test-" + [guid]::NewGuid().ToString('N'))
        [System.IO.File]::WriteAllText($probe, 'x')
        Remove-Item -LiteralPath $probe -Force -ErrorAction SilentlyContinue
        return $true
    } catch { return $false }
}

function Invoke-FileDownload {
    <#  用 WebClient 而不是 Invoke-WebRequest：5.1 的 Invoke-WebRequest 走 IE 引擎，几十 MB 会慢到离谱  #>
    param(
        [Parameter(Mandatory)][string]$Url,
        [Parameter(Mandatory)][string]$OutFile,
        [string]$Proxy,
        [int]$TimeoutMs = 600000
    )

    $wc = New-Object System.Net.WebClient
    try {
        if ($Proxy) {
            $wc.Proxy = New-Object System.Net.WebProxy($Proxy, $true)
        } else {
            # 不显式给 -Proxy 就沿用系统/环境里的代理设置
            $wc.Proxy = [System.Net.WebRequest]::DefaultWebProxy
        }
        $wc.Headers.Add('User-Agent', 'ComfyHub-ensure-runtime/1.0')
        $wc.DownloadFile($Url, $OutFile)
    } finally {
        $wc.Dispose()
    }
}

function Get-AdoptiumJreInfo {
    <#  Adoptium API：拿到当前 21 GA 的 JRE 直链 + 大小 + sha256  #>
    param([string]$Proxy)

    $api = 'https://api.adoptium.net/v3/assets/latest/21/hotspot?architecture=x64&image_type=jre&os=windows&vendor=eclipse'
    $params = @{ Uri = $api; TimeoutSec = 30; UseBasicParsing = $true }
    if ($Proxy) { $params['Proxy'] = $Proxy }

    $res = Invoke-RestMethod @params
    if (-not $res -or -not $res[0].binary.package.link) { return $null }
    return [pscustomobject]@{
        Link    = [string]$res[0].binary.package.link
        Name    = [string]$res[0].binary.package.name
        Size    = [long]$res[0].binary.package.size
        Sha256  = [string]$res[0].binary.package.checksum
        Version = [string]$res[0].version.semver
    }
}

function Expand-JavaArchive {
    <#
      解压 JDK/JRE zip。zip 顶层通常套了一层（jdk-21.0.x+y-jre\），
      这里自动把它剥掉，让 <目标>\bin\java.exe 直接成立 ——
      server.ps1 / comfyhub.ps1 认的就是这个形状。
    #>
    param(
        [Parameter(Mandatory)][string]$ZipPath,
        [Parameter(Mandatory)][string]$DestDir
    )

    Add-Type -AssemblyName System.IO.Compression.FileSystem -ErrorAction SilentlyContinue

    $stage = Join-Path ([System.IO.Path]::GetTempPath()) ('comfyhub-jre-stage-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $stage | Out-Null
    try {
        [System.IO.Compression.ZipFile]::ExtractToDirectory($ZipPath, $stage)

        # 找 bin\java.exe，取层级最浅的那个（有些发行版里还带别的 java.exe）
        $javaExe = Get-ChildItem -LiteralPath $stage -Recurse -Filter 'java.exe' -ErrorAction SilentlyContinue |
            Where-Object { $_.Directory -and $_.Directory.Name -eq 'bin' } |
            Sort-Object { $_.FullName.Split('\').Count } |
            Select-Object -First 1
        if (-not $javaExe) { throw "解压出来的包里找不到 bin\java.exe" }

        # 注意：别把变量叫 $home —— PowerShell 里 $HOME 是只读自动变量（大小写不敏感）。
        $javaHome = $javaExe.Directory.Parent.FullName

        # 安全闸：只允许搬临时解压目录里的东西。
        # 这条断言是有来历的：这个变量曾经一处漏改成 $home（=$HOME，也就是整个用户目录），
        # 于是 Move-Item 真的动手去搬 C:\Users\<用户>\ —— 幸好目录被占用才没搬成。
        # 有了它，同类错误会当场抛异常，而不是去动用户的文件。
        if (-not $javaHome.StartsWith($stage, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "内部错误：要搬运的目录 '$javaHome' 不在临时解压目录 '$stage' 内，已拒绝执行。"
        }

        # 先把旧的挪走再放新的：中途失败也不会留下半残的 jre\
        $backup = $null
        if (Test-Path -LiteralPath $DestDir) {
            $backup = "$DestDir.old-" + [guid]::NewGuid().ToString('N').Substring(0, 8)
            Move-Item -LiteralPath $DestDir -Destination $backup -Force
        }
        try {
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $DestDir) | Out-Null
            Move-Item -LiteralPath $javaHome -Destination $DestDir -Force
        } catch {
            if ($backup -and (Test-Path -LiteralPath $backup) -and -not (Test-Path -LiteralPath $DestDir)) {
                Move-Item -LiteralPath $backup -Destination $DestDir -Force
            }
            throw
        }
        if ($backup) { Remove-Item -LiteralPath $backup -Recurse -Force -ErrorAction SilentlyContinue }
    } finally {
        Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function Test-UrlReachable {
    <#
      下载前先花几秒探一下（Range: 只取头 1KB）。
      源被墙 / 挂掉时立刻换下一个 —— 否则要在一个连不上的地址上白等到超时，
      后面的可用源（比如国内的华为云）迟迟轮不到。
    #>
    param(
        [Parameter(Mandatory)][string]$Url,
        [string]$Proxy,
        [int]$TimeoutSec = 10
    )

    $resp = $null
    try {
        $req = [System.Net.HttpWebRequest]::Create($Url)
        $req.Method = 'GET'
        [void]$req.AddRange(0, 1023)
        $req.Timeout = $TimeoutSec * 1000
        $req.ReadWriteTimeout = $TimeoutSec * 1000
        $req.UserAgent = 'ComfyHub-ensure-runtime/1.0'
        if ($Proxy) { $req.Proxy = New-Object System.Net.WebProxy($Proxy, $true) }
        else { $req.Proxy = [System.Net.WebRequest]::DefaultWebProxy }
        $resp = $req.GetResponse()
        $code = [int]$resp.StatusCode
        return ($code -ge 200 -and $code -lt 300)
    } catch {
        return $false
    } finally {
        if ($resp) { $resp.Close() }
    }
}

function Install-JavaRuntime {
    <#
      下载一个便携版 JRE 21 并解压到 <根>\jre（根目录不可写时退到
      %LOCALAPPDATA%\ComfyHub\jre，并写用户环境变量 COMFYHUB_JDK_HOME 让脚本找得到）。

      下载源按顺序试，任何一个成功就停：
        1. Adoptium 官方直链（api.adoptium.net 给出的 GitHub 地址，带 sha256）
        2. 清华 TUNA 镜像（国内快；文件名与官方一致，同一个 sha256）
        3. Adoptium 的 "latest binary" 重定向（不需要先拿文件名）
        4. Microsoft Build of OpenJDK（aka.ms）
    #>
    param(
        [Parameter(Mandatory)][string]$ProjectRoot,
        [string]$Proxy,
        [switch]$Quiet
    )

    Initialize-Tls12

    function Say2([string]$m, [string]$c = 'Gray') { if (-not $Quiet) { Write-Host $m -ForegroundColor $c } }

    # ---- 目标目录 --------------------------------------------------------
    $dest = Join-Path $ProjectRoot 'jre'
    $fallback = Join-Path $env:LOCALAPPDATA 'ComfyHub\jre'
    if (-not (Test-DirWritable -Dir $ProjectRoot)) {
        Say2 "  $ProjectRoot 不可写，改用 $fallback" 'Yellow'
        $dest = $fallback
    }

    # ---- 候选下载源 ------------------------------------------------------
    $cands = New-Object System.Collections.ArrayList
    $info = $null
    try {
        $info = Get-AdoptiumJreInfo -Proxy $Proxy
    } catch {
        Say2 "  取 Adoptium 元数据失败（$($_.Exception.Message)），改用固定地址。" 'Yellow'
    }
    if ($info) {
        Say2 ("  目标版本: Temurin JRE {0}  ({1:N1} MB)" -f $info.Version, ($info.Size / 1MB)) 'DarkGray'
        [void]$cands.Add([pscustomobject]@{ Name = 'Adoptium 官方 (GitHub)'; Url = $info.Link; Sha256 = $info.Sha256; File = $info.Name })
    }
    # 国内可达性最好的一个（实测 TUNA / USTC / BFSU / 腾讯云的 Adoptium 镜像要么 403 要么 404）。
    # 这是 Oracle OpenJDK 21.0.2 的完整 JDK（约 190MB，比 Temurin JRE 大），跑后端绰绰有余。
    # sha256 取自华为云随文件发布的 .sha256；URL 钉死了版本号，所以校验值是稳定的。
    [void]$cands.Add([pscustomobject]@{
            Name   = '华为云 OpenJDK 21.0.2'
            Url    = 'https://mirrors.huaweicloud.com/openjdk/21.0.2/openjdk-21.0.2_windows-x64_bin.zip'
            Sha256 = 'b6c17e747ae78cdd6de4d7532b3164b277daee97c007d3eaa2b39cca99882664'
            File   = 'openjdk-21.0.2_windows-x64_bin.zip'
        })
    [void]$cands.Add([pscustomobject]@{
            Name   = 'Adoptium 重定向'
            Url    = 'https://api.adoptium.net/v3/binary/latest/21/ga/windows/x64/jre/hotspot/normal/eclipse'
            Sha256 = $null; File = 'temurin-jre21.zip'
        })
    [void]$cands.Add([pscustomobject]@{
            Name   = 'Microsoft OpenJDK'
            Url    = 'https://aka.ms/download-jdk/microsoft-jdk-21-windows-x64.zip'
            Sha256 = $null; File = 'ms-openjdk21.zip'
        })

    $cacheDir = Join-Path ([System.IO.Path]::GetTempPath()) 'comfyhub-runtime-cache'
    New-Item -ItemType Directory -Force -Path $cacheDir | Out-Null

    $lastErr = $null
    foreach ($c in $cands) {
        $zip = Join-Path $cacheDir $c.File
        try {
            Say2 ("  下载 {0} …" -f $c.Name) 'Cyan'
            Say2 ("    {0}" -f $c.Url) 'DarkGray'
            if (-not (Test-UrlReachable -Url $c.Url -Proxy $Proxy)) {
                throw '连不上（探测超时或被拒绝），换下一个源'
            }
            if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip -Force }
            Invoke-FileDownload -Url $c.Url -OutFile $zip -Proxy $Proxy

            $len = (Get-Item -LiteralPath $zip).Length
            if ($len -lt 5MB) { throw "下下来的文件只有 $([math]::Round($len/1KB)) KB，不像一个 JRE，换下一个源。" }
            Say2 ("    已下载 {0:N1} MB" -f ($len / 1MB)) 'DarkGray'

            if ($c.Sha256) {
                $actual = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLower()
                if ($actual -ne $c.Sha256.ToLower()) {
                    throw "sha256 校验失败（期望 $($c.Sha256)，实际 $actual）"
                }
                Say2 '    sha256 校验通过' 'DarkGray'
            } else {
                Say2 '    （该源没有官方校验值，跳过 sha256）' 'DarkGray'
            }

            Say2 ("  解压到 {0} …" -f $dest) 'Cyan'
            Expand-JavaArchive -ZipPath $zip -DestDir $dest

            $java = Join-Path $dest 'bin\java.exe'
            if (-not (Test-Path -LiteralPath $java)) { throw "解压后没找到 $java" }
            $major = Get-JavaMajorVersion -JavaExe $java
            if (-not $major -or $major -lt 21) { throw "装好的 java 版本不对劲（$major）" }

            # 退到 LOCALAPPDATA 时，让别的脚本（server.ps1 的 Resolve-Jdk）也能找到它
            if ($dest -ne (Join-Path $ProjectRoot 'jre')) {
                try {
                    [Environment]::SetEnvironmentVariable('COMFYHUB_JDK_HOME', $dest, 'User')
                    Say2 "  已写入用户环境变量 COMFYHUB_JDK_HOME=$dest" 'DarkGray'
                } catch { }
            }

            Say2 ("  Java 运行时就绪: {0} (v{1})" -f $java, $major) 'Green'
            return $java
        } catch {
            $lastErr = $_.Exception.Message
            Say2 ("    {0} 失败: {1}" -f $c.Name, $lastErr) 'Yellow'
        }
    }

    throw "自动获取 Java 运行时失败（所有下载源都不行）。最后一个错误: $lastErr`n手动安装 JDK 21 后用 -JdkHome / COMFYHUB_JDK_HOME 指定，或直接看 README 9.2。"
}

function Ensure-JavaRuntime {
    <#
      给 comfyhub.ps1 用的"够用就好"入口：
        · 已经有 >=21 的 java → 直接返回，什么都不做（快，不联网）
        · 没有 → 自动下一个便携版塞进 <根>\jre
      $Quiet=true 时只在"真的装了"的时候才有输出。
    #>
    param(
        [Parameter(Mandatory)][string]$ProjectRoot,
        [string]$Proxy,
        [switch]$Quiet,
        [switch]$Force
    )

    $exe = $null
    if (-not $Force) { $exe = Get-JavaRuntimePath -ProjectRoot $ProjectRoot }

    if ($exe) {
        $major = Get-JavaMajorVersion -JavaExe $exe
        if ($major -and $major -ge 21) {
            if (-not $Quiet) { Write-Host "  Java 运行时已就绪: $exe (v$major)" -ForegroundColor Green }
            return $exe
        }
        # 真要装了就把话说清楚（哪怕 -Quiet）—— 否则启动会莫名其妙卡住几十秒
        Write-Host ("  找到的 Java 是 v{0}（后端要 >= 21），自动装一个 21…" -f $major) -ForegroundColor Yellow
    } else {
        Write-Host '  没找到 Java 运行时，自动下载一个便携版 JRE 21（约 47MB，只此一次）…' -ForegroundColor Yellow
    }

    return (Install-JavaRuntime -ProjectRoot $ProjectRoot -Proxy $Proxy)
}

function Get-WingetPath {
    $w = Get-Command winget -ErrorAction SilentlyContinue
    if ($w) { return $w.Source }
    return $null
}

function Install-WingetPackage {
    <#
      用 winget 装系统级运行时。装 VC++ 运行时需要管理员权限，会弹 UAC ——
      这是安装本身的性质，脚本没法绕过。
    #>
    param(
        [Parameter(Mandatory)][string]$Id,
        [switch]$Quiet
    )

    $winget = Get-WingetPath
    if (-not $winget) { return $false }

    if (-not $Quiet) { Write-Host ("  winget install --id {0} …" -f $Id) 'Cyan' }
    & $winget install --id $Id --source winget --accept-package-agreements --accept-source-agreements --silent
    return ($LASTEXITCODE -eq 0)
}

function Invoke-RuntimeEnsure {
    <#
      ensure-runtime.ps1 的主逻辑：检测全部运行时，把缺的补上。
        · Java        → 自动下载便携版（免安装、免管理员）
        · pwsh / VC++ → 有 winget 就自动装，没有就给出下载地址
      返回 $true 表示全部就绪。
    #>
    param(
        [Parameter(Mandatory)][string]$ProjectRoot,
        [switch]$Force,
        [switch]$CheckOnly,
        [switch]$Quiet,
        [switch]$JavaOnly,
        [switch]$NoSystem,
        [string]$Proxy
    )

    function Say3([string]$m, [string]$c = 'Gray') { if (-not $Quiet) { Write-Host $m -ForegroundColor $c } }

    Initialize-Tls12

    # ---- Java ------------------------------------------------------------
    if (-not $CheckOnly) {
        try {
            Ensure-JavaRuntime -ProjectRoot $ProjectRoot -Proxy $Proxy -Quiet:$Quiet -Force:$Force | Out-Null
        } catch {
            Write-Host "  !! Java 运行时没能自动装好: $($_.Exception.Message)" -ForegroundColor Red
        }
    }

    if ($JavaOnly) {
        $j = Get-JavaRuntimePath -ProjectRoot $ProjectRoot
        $maj = Get-JavaMajorVersion -JavaExe $j
        return [bool]($j -and $maj -and $maj -ge 21)
    }

    # ---- pwsh / VC++ -----------------------------------------------------
    $pwshCmd = Get-Command pwsh -ErrorAction SilentlyContinue
    $mysqld = $null
    try {
        $mysqld = Join-Path $ProjectRoot 'mysql\bin\mysqld.exe'
    } catch { }
    $binDir = $null
    if ($mysqld -and (Test-Path -LiteralPath $mysqld)) { $binDir = Split-Path -Parent $mysqld }
    $vcOk = Test-VcRuntime -MySqlBinDir $binDir

    if (-not $NoSystem -and -not $CheckOnly) {
        if (-not $pwshCmd) {
            Say3 '  缺 PowerShell 7，尝试用 winget 安装…' 'Yellow'
            if (-not (Install-WingetPackage -Id 'Microsoft.PowerShell' -Quiet:$Quiet)) {
                Say3 '  winget 装不了（可能没有 winget）。请手动装: https://aka.ms/powershell' 'Red'
            } else {
                Say3 '  PowerShell 7 安装完成（新开的终端/重启 App 后生效）。' 'Green'
                $pwshCmd = Get-Command pwsh -ErrorAction SilentlyContinue
            }
        }
        if (-not $vcOk) {
            Say3 '  缺 VC++ 运行时，尝试用 winget 安装（会弹 UAC）…' 'Yellow'
            if (-not (Install-WingetPackage -Id 'Microsoft.VCRedist.2015+.x64' -Quiet:$Quiet)) {
                Say3 '  winget 装不了。请手动装: https://aka.ms/vs/17/release/vc_redist.x64.exe' 'Red'
            } else {
                $vcOk = Test-VcRuntime -MySqlBinDir $binDir
            }
        }
    }

    # ---- 汇总 ------------------------------------------------------------
    $checks = Get-RuntimeChecks -ProjectRoot $ProjectRoot -MySqlBinDir $binDir
    $bad = @($checks | Where-Object { -not $_.Ok })

    Say3 ''
    Say3 '  运行时依赖' 'White'
    Say3 '  ────────────────────────────────────────────────────────────'
    foreach ($c in $checks) {
        $mark = 'OK  '
        $color = 'Green'
        if (-not $c.Ok) { $mark = '缺失'; $color = 'Red' }
        Say3 ("  {0,-28} {1}  {2}" -f $c.Name, $mark, $c.Detail) $color
    }

    if ($bad.Count -gt 0) {
        Say3 ''
        Say3 '  下面这些还没就绪（装完重启 App）：' 'Yellow'
        foreach ($c in $bad) {
            Say3 ("    · {0}" -f $c.Name) 'Yellow'
            Say3 ("        {0}" -f $c.Hint) 'Cyan'
        }
        return $false
    }

    Say3 ''
    Say3 '  全部运行时依赖就绪。' 'Green'
    return $true
}
