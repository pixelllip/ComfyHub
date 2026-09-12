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
