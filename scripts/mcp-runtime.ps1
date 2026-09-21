<#
.SYNOPSIS
    ComfyHub 的 MCP 运行时装配：便携版 Python + comfy-cli + comfy-mcp。

.DESCRIPTION
    为什么要这个文件：我们要把**官方 comfy-mcp** 接成 ComfyUI 能力的提供方
    （用户 2026-09-21 定的方向），而 comfy-mcp 是 Python 写的、并且要求
    `comfy-cli`（它自己不捆绑，README 原话："Installing this server does not install comfy-cli"）。
    发布包是**便携式**的，不能要求用户自己去装 Python、开终端敲 pip ——
    所以这里把三样东西按"缺什么补什么"装进**包内**，跟 <根>\jre / <根>\mysql 一个路子。

    与 runtime-deps.ps1 的分工：
      · runtime-deps.ps1  —— **必需**的运行时（Java / MySQL），缺了服务根本起不来
      · 本文件            —— **可选能力**的运行时（MCP），缺了只是"ComfyUI 那批工具用不了"
    所以这里的失败**不应该**拦住 up（这点和 Java 完全不同）：如实报告即可。

    装到哪儿（都在根目录内，便携式）：
      <根>\storage\ai\mcp\python\           便携版 Python 3.12（embeddable + pip）
      <根>\storage\ai\mcp\python\Scripts\    pip 装的 console script（comfy.exe / comfy-mcp.exe）
      <根>\storage\ai\mcp\state.json         本次装配的版本与路径（幂等判据 + doctor 显示）
      根目录不可写时退回 %LOCALAPPDATA%\ComfyHub\mcp（并把位置记进 state.json）

    三条实测结论（2026-09-21 在本机真跑过，不是推断）：
      1. **embeddable zip 能用**：python-3.12.10-embed-amd64.zip（10.6MB）解压后把
         `python312._pth` 里的 `import site` 打开、补一行 `Lib\site-packages`，
        再用官方 get-pip.py 就能装上 pip —— 全程免安装、免管理员。
         注意：embeddable 版**没有 venv 模块**，所以是"整包一个环境"（不建 venv），
         这也正好符合我们要的"跑哪个解释器是可预测的"。
      2. **comfy-cli 1.20.0 + comfy-mcp 0.10.0 + mcp SDK 2.2.0 能一起装上**（共约 300MB）。
         comfy-mcp 0.10.0 需要 mcp SDK **2.x**（2.0.0 把 `mcp.server.fastmcp` 删了），pip 会自动挑对。
      3. **comfy-cli 必须知道 ComfyUI 在哪**：它有一套自己的"工作区"概念，
         状态写在 `%LOCALAPPDATA%\comfy-cli\config.ini`，用 `comfy set-default <path>` 指定。
         实测：装完不设的话，它默认指向 `%USERPROFILE%\Documents\comfy\ComfyUI`（不存在也会这么说），
         于是 `run_workflow` / `search_models` 这些全是错的。所以装配的最后一步必须
         把用户**真实在用的** ComfyUI 目录喂给它（判据复用 scripts\comfy-path.ps1）。

    被谁调用：
      · scripts\comfyhub.ps1 的 Do-Up / Do-Doctor （dot-source 本文件，跟 runtime-deps.ps1 同套路）
      · 单独用：pwsh -File scripts\mcp-runtime.ps1 install | status
#>

# 独立运行的参数（被 dot-source 时这几位保持默认，不影响宿主脚本）。
#
# ⚠ 参数名**绝不能叫 $McpAction / $McpForce / $ComfyHome 之外那些通用名字**：
#   dot-source 是在**调用者的作用域**里执行的，param() 会把宿主脚本里同名的变量覆盖掉。
#   2026-09-21 实测踩到：这里原来叫 $McpAction，于是 `comfyhub.ps1 doctor` 被 dot-source 之后
#   $McpAction 从 'doctor' 变成默认值 'status' —— doctor 静默变成 status（输出只剩状态表，
#   运行期设置里 ComfyUI 那一段整个不打印），查了很久才定位到。
#   所以名字统一带 Mcp 前缀（Action→McpAction、Force→McpForce、Proxy→McpProxy）。
#   判据用 $PSBoundParameters：被 dot-source 时一个参数都没绑，就什么都不做。
param(
    [Parameter(Position = 0)][string]$McpAction = 'status',
    [switch]$McpForce,
    [string]$ComfyHome,
    [string]$McpProxy,
    # status -Deep：真去问 comfy-cli（约 6 秒）拿版本与工作区；不带则读装配记录（不 spawn 进程）
    [switch]$Deep
)
$script:McpRuntimeStateFileName = 'state.json'
$script:McpRuntimeIndexUrls = @()
$script:McpRuntimeSpec = @()
$script:McpRuntimePythonVersion = '3.12.10'

# 下载源按顺序试。2026-09-21 实测（本机、走系统代理）：
#   · 阿里云    —— 可用（comfy-mcp 的 wheel 能下下来）
#   · 清华 TUNA —— **对 comfy_mcp 返回 HTTP 403**（包元数据能读、wheel 拿不到），
#                  写在这里是为了让"换源"这件事有据可查，它排第二
#   · pypi.org  —— 可用（官方源，兜底）
$script:McpRuntimeIndexUrls = @(
    'https://mirrors.aliyun.com/pypi/simple/',
    'https://pypi.tuna.tsinghua.edu.cn/simple',
    'https://pypi.org/simple'
)
# 钉住的版本范围：comfy-cli 有硬下限（comfy-mcp 要求 >=1.14.0），comfy-mcp 用 0.10.x 这一代
$script:McpRuntimeSpec = @('comfy-mcp>=0.10,<0.11', 'comfy-cli>=1.14.0')

# ===========================================================================
#  位置解析（两种布局：源码树 / 发布包）
# ===========================================================================

function Get-McpRuntimeDir {
    <#
      可选运行时的落点：<根>\storage\ai\mcp（与 storage\ai\skills 同一个 storage 根）。
      根不可写时退回 %LOCALAPPDATA%\ComfyHub\mcp —— 与 runtime-deps.ps1 里
      Java 退到 %LOCALAPPDATA%\ComfyHub\jre 是同一个套路。
    #>
    param([Parameter(Mandatory)][string]$ProjectRoot)

    $preferred = Join-Path $ProjectRoot 'storage\ai\mcp'
    if (Test-Path -LiteralPath $preferred) { return $preferred }
    # 还没建过：看父目录能不能写（不能写就直接退到 LOCALAPPDATA）
    $parent = Join-Path $ProjectRoot 'storage\ai'
    $probeRoot = if (Test-Path -LiteralPath $parent) { $parent } else { $ProjectRoot }
    $writable = if (Get-Command Test-DirWritable -ErrorAction SilentlyContinue) {
        Test-DirWritable -Dir $probeRoot
    } else {
        Test-McpDirWritable -Dir $probeRoot
    }
    if ($writable) { return $preferred }
    return (Join-Path $env:LOCALAPPDATA 'ComfyHub\mcp')
}

function Test-McpDirWritable {
    <#
      Test-DirWritable 定义在 runtime-deps.ps1 里（被 comfyhub.ps1 一起 dot-source）。
      这里留一个同等实现的兜底：单独拷走 mcp-runtime.ps1 时也不该因为少一个函数就崩。
    #>
    param([string]$Dir)
    try {
        if (-not (Test-Path -LiteralPath $Dir)) { return $false }
        $probe = Join-Path $Dir ('.comfyhub-write-test-' + [guid]::NewGuid().ToString('N'))
        [System.IO.File]::WriteAllText($probe, 'x')
        Remove-Item -LiteralPath $probe -Force -ErrorAction SilentlyContinue
        return $true
    } catch { return $false }
}

function Get-McpRuntimeStatePath {
    param([Parameter(Mandatory)][string]$McpDir)
    return (Join-Path $McpDir $script:McpRuntimeStateFileName)
}

function Read-McpRuntimeState {
    <#  读装配状态（不存在 / 读不了都返回 $null，绝不抛）  #>
    param([Parameter(Mandatory)][string]$McpDir)

    $file = Get-McpRuntimeStatePath -McpDir $McpDir
    if (-not (Test-Path -LiteralPath $file)) { return $null }
    try { return (Get-Content -LiteralPath $file -Raw -Encoding UTF8 | ConvertFrom-Json) } catch { return $null }
}

function Write-McpRuntimeState {
    param(
        [Parameter(Mandatory)][string]$McpDir,
        [Parameter(Mandatory)]$State
    )
    New-Item -ItemType Directory -Force -Path $McpDir | Out-Null
    $file = Get-McpRuntimeStatePath -McpDir $McpDir
    ($State | ConvertTo-Json -Depth 6) | Set-Content -LiteralPath $file -Encoding UTF8
    return $file
}

function Get-PythonRuntimePath {
    <#
      找"能跑 comfy-mcp 的 python.exe"。顺序（前者优先）：
        1. state.json 里记的（上次装配的就是它）
        2. <McpDir>\python\python.exe
        3. <LOCALAPPDATA>\ComfyHub\mcp\python\python.exe（根不可写时退到的那份）
    #>
    param(
        [Parameter(Mandatory)][string]$McpDir,
        [string]$McpProxy
    )

    $state = Read-McpRuntimeState -McpDir $McpDir
    $cands = New-Object System.Collections.ArrayList
    if ($state -and $state.pythonExe) { [void]$cands.Add([string]$state.pythonExe) }
    [void]$cands.Add((Join-Path $McpDir 'python\python.exe'))
    [void]$cands.Add((Join-Path $env:LOCALAPPDATA 'ComfyHub\mcp\python\python.exe'))

    foreach ($c in $cands) {
        if ($c -and (Test-Path -LiteralPath $c)) { return $c }
    }
    return $null
}

# ===========================================================================
#  检测
# ===========================================================================

function Get-McpRuntimeStatus {
    <#
      体检 MCP 运行时。返回一个对象，doctor 与 up 都用它：
        Ok             服务器件齐不齐（python + comfy-mcp + comfy-cli 都在，且工作区可用）
        PythonExe / ComfyMcpExe / ComfyExe
        PythonVersion / ComfyMcpVersion / ComfyCliVersion
        Workspace      comfy-cli 认的 ComfyUI 工作区（没设时为 $null）
        WorkspaceOk    那个目录真的像个 ComfyUI 吗
        Deep           这次是真去问了 comfy-cli，还是读的装配记录
        Dir / State

      **为什么分快慢两档**（2026-09-21 实测）：`& python -m comfy_cli env` 一次要 **6.1 秒**
      （comfy-cli 的 import + typer 启动就是这么慢），而 `up` 每次冷启动都会调这个函数 ——
      那就是白等 6 秒。所以：
        · 默认（快档，**不 spawn 任何进程**）：exe 在不在（文件检查）+ 装配记录里的版本与工作区
        · -Deep（慢档）：真去问 python 包版本与 `comfy_cli env`（装配完 / doctor 里想看真相时用）
    #>
    param(
        [Parameter(Mandatory)][string]$ProjectRoot,
        [switch]$Deep
    )

    $dir = Get-McpRuntimeDir -ProjectRoot $ProjectRoot
    $state = Read-McpRuntimeState -McpDir $dir
    $py = Get-PythonRuntimePath -McpDir $dir
    $result = [pscustomobject]@{
        Dir             = $dir
        State           = $state
        Deep            = [bool]$Deep
        Ok              = $false
        PythonExe       = $py
        PythonVersion   = $(if ($state) { $state.pythonVersion } else { $null })
        ComfyMcpExe     = $null
        ComfyMcpVersion = $(if ($state) { $state.comfyMcpVersion } else { $null })
        ComfyExe        = $null
        ComfyCliVersion = $(if ($state) { $state.comfyCliVersion } else { $null })
        Workspace       = $(if ($state) { $state.workspace } else { $null })
        WorkspaceOk     = $false
        Problems        = @()
    }

    $problems = New-Object System.Collections.ArrayList
    if (-not $py) {
        [void]$problems.Add('没有便携版 Python（还没装配过？）')
        $result.Problems = $problems
        return $result
    }

    $scripts = Join-Path (Split-Path -Parent $py) 'Scripts'
    $mcpExe = Join-Path $scripts 'comfy-mcp.exe'
    $comfyExe = Join-Path $scripts 'comfy.exe'
    if (Test-Path -LiteralPath $mcpExe) { $result.ComfyMcpExe = $mcpExe } else { [void]$problems.Add('缺 comfy-mcp.exe（pip 没装成？）') }
    if (Test-Path -LiteralPath $comfyExe) { $result.ComfyExe = $comfyExe } else { [void]$problems.Add('缺 comfy.exe（comfy-cli 没装成？）') }

    if ($Deep) {
        # 真问 python（只在这条路上 spawn 进程）
        try {
            $out = (& $py -c "import sys;print('%d.%d.%d'%sys.version_info[:3])" 2>&1 | Out-String).Trim()
            if ($out -match '^\d+\.\d+\.\d+$') { $result.PythonVersion = $out }
        } catch { }
        foreach ($probe in @(
                @{ Name = 'ComfyMcpVersion'; Mod = 'comfy-mcp' },
                @{ Name = 'ComfyCliVersion'; Mod = 'comfy-cli' }
            )) {
            try {
                $v = (& $py -c "import importlib.metadata as m;print(m.version('$($probe.Mod)'))" 2>&1 | Out-String).Trim()
                if ($v -match '^\d') { $result.($probe.Name) = $v }
            } catch { }
        }
        try {
            $json = (& $py -m comfy_cli --json env 2>$null | Out-String)
            $start = $json.IndexOf('{')
            if ($start -ge 0) {
                $obj = ($json.Substring($start) | ConvertFrom-Json)
                $ws = [string]$obj.data.workspace.path
                if ($ws) { $result.Workspace = $ws }
            }
        } catch { }
    }

    # 工作区是"存在性"判断（文件检查），快慢两档都做
    if (-not $result.Workspace) {
        [void]$problems.Add('comfy-cli 还不知道 ComfyUI 在哪（没设工作区）')
    } else {
        $result.WorkspaceOk = (Test-ComfyHome $result.Workspace)
        if (-not $result.WorkspaceOk) {
            [void]$problems.Add("comfy-cli 的工作区不像一个 ComfyUI：$($result.Workspace)")
        }
    }

    $result.Ok = ($problems.Count -eq 0)
    $result.Problems = $problems
    return $result
}

# ===========================================================================
#  安装：便携版 Python
# ===========================================================================

function Install-PortablePython {
    <#
      下载 python-3.12.x-embed-amd64.zip 解压到 <McpDir>\python，再补 pip。
      步骤与实测结论见文件头 ①②。
    #>
    param(
        [Parameter(Mandatory)][string]$McpDir,
        [string]$McpProxy,
        [switch]$Quiet
    )

    function SayP([string]$m, [string]$c = 'Gray') { if (-not $Quiet) { Write-Host $m -ForegroundColor $c } }

    Initialize-Tls12

    $ver = $script:McpRuntimePythonVersion
    $dest = Join-Path $McpDir 'python'
    $url = "https://www.python.org/ftp/python/$ver/python-$ver-embed-amd64.zip"
    $cache = Join-Path ([System.IO.Path]::GetTempPath()) 'comfyhub-runtime-cache'
    New-Item -ItemType Directory -Force -Path $cache | Out-Null
    $zip = Join-Path $cache "python-$ver-embed-amd64.zip"

    SayP ("  下载便携版 Python {0}（约 10MB，只此一次）…" -f $ver) 'Yellow'
    if (-not (Test-UrlReachable -Url $url -Proxy $McpProxy)) {
        throw "连不上 $url（探测超时或被拒绝）"
    }
    if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip -Force }
    Invoke-FileDownload -Url $url -OutFile $zip -Proxy $McpProxy
    $len = (Get-Item -LiteralPath $zip).Length
    if ($len -lt 5MB) { throw "下下来的文件只有 $([math]::Round($len/1KB)) KB，不像 Python 安装包" }

    # 解压到临时目录，装配完整了再整体搬过去（中途失败不留半残）
    $stage = Join-Path ([System.IO.Path]::GetTempPath()) ('comfyhub-mcp-stage-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $stage | Out-Null
    try {
        Add-Type -AssemblyName System.IO.Compression.FileSystem -ErrorAction SilentlyContinue
        [System.IO.Compression.ZipFile]::ExtractToDirectory($zip, $stage)

        $pyExe = Join-Path $stage 'python.exe'
        if (-not (Test-Path -LiteralPath $pyExe)) { throw "解压出来的包里没有 python.exe" }

        # --- 打开 site-packages -----------------------------------------
        # embeddable 版的 ._pth 默认把 `import site` 注释掉，且不含 site-packages，
        # 于是 pip 装的东西 import 不到。两行都要改。
        $pth = Get-ChildItem -LiteralPath $stage -Filter 'python*._pth' | Select-Object -First 1
        if (-not $pth) { throw "找不到 python*._pth（embeddable 版应该有）" }
        $lines = @(Get-Content -LiteralPath $pth.FullName)
        $out = New-Object System.Collections.ArrayList
        $sawSite = $false
        foreach ($l in $lines) {
            if ($l -match '^\s*#\s*import\s+site\s*$') { [void]$out.Add('import site'); $sawSite = $true; continue }
            [void]$out.Add($l)
        }
        if (-not $sawSite) { [void]$out.Add('import site') }
        [void]$out.Add('Lib\site-packages')
        Set-Content -LiteralPath $pth.FullName -Value ($out -join "`r`n") -Encoding ASCII
        New-Item -ItemType Directory -Force -Path (Join-Path $stage 'Lib\site-packages') | Out-Null

        # --- 装 pip -----------------------------------------------------
        SayP '  装 pip（get-pip.py）…' 'Cyan'
        $getpip = Join-Path $cache 'get-pip.py'
        if (-not (Test-Path -LiteralPath $getpip)) {
            Invoke-FileDownload -Url 'https://bootstrap.pypa.io/get-pip.py' -OutFile $getpip -Proxy $McpProxy
        }
        & $pyExe $getpip --no-warn-script-location --disable-pip-version-check 2>&1 |
            Where-Object { $_ -notmatch '^\s*$' } | Select-Object -Last 3 | ForEach-Object { SayP "    $_" 'DarkGray' }
        $pipOk = (& $pyExe -m pip --version 2>&1 | Out-String)
        if ($pipOk -notmatch 'pip \d') { throw "pip 没装上：$($pipOk.Trim())" }

        # --- 搬到位 -----------------------------------------------------
        # 安全闸（承 runtime-deps.ps1 那次 $home 事故的教训）：只允许搬临时目录里的东西
        if (-not $stage.StartsWith([System.IO.Path]::GetTempPath(), [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "内部错误：staging 目录 '$stage' 不在系统临时目录内，已拒绝执行。"
        }
        New-Item -ItemType Directory -Force -Path $McpDir | Out-Null
        $backup = $null
        if (Test-Path -LiteralPath $dest) {
            $backup = "$dest.old-" + [guid]::NewGuid().ToString('N').Substring(0, 8)
            Move-Item -LiteralPath $dest -Destination $backup -Force
        }
        try {
            Move-Item -LiteralPath $stage -Destination $dest -Force
        } catch {
            if ($backup -and (Test-Path -LiteralPath $backup) -and -not (Test-Path -LiteralPath $dest)) {
                Move-Item -LiteralPath $backup -Destination $dest -Force
            }
            throw
        }
        if ($backup) { Remove-Item -LiteralPath $backup -Recurse -Force -ErrorAction SilentlyContinue }

        $finalPy = Join-Path $dest 'python.exe'
        if (-not (Test-Path -LiteralPath $finalPy)) { throw "搬完了却找不到 $finalPy" }
        SayP ("  便携版 Python 就绪: {0}" -f $finalPy) 'Green'
        return $finalPy
    } finally {
        Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function Install-McpPackages {
    <#
      pip 装 comfy-mcp + comfy-cli。
      国内源先试（清华 TUNA），失败换官方源 —— 与 Java 那套"多源按顺序试"一致。
    #>
    param(
        [Parameter(Mandatory)][string]$PythonExe,
        [string]$McpProxy,
        [switch]$Quiet
    )

    function SayQ([string]$m, [string]$c = 'Gray') { if (-not $Quiet) { Write-Host $m -ForegroundColor $c } }

    $lastErr = $null
    foreach ($index in $script:McpRuntimeIndexUrls) {
        try {
            SayQ ("  用 {0} 装 comfy-mcp + comfy-cli（首次约 300MB，慢一点）…" -f $index) 'Cyan'
            # 注意：别把这个变量叫 $args —— PowerShell 的 $args 是自动变量（大小写不敏感），
            # 覆盖它会连带影响别的写法（AGENTS §8 那张"别起的变量名"清单里就有它）。
            $pipArgs = @('-m', 'pip', 'install', '--no-warn-script-location', '--disable-pip-version-check',
                '--no-input', '--index-url', $index) + $script:McpRuntimeSpec
            $out = (& $PythonExe @pipArgs 2>&1 | Out-String)
            if ($LASTEXITCODE -ne 0) {
                # 把 pip 的**真实错因**带出来（403 / 404 / 连不上，各是各的处理），
                # 只说一句"这个源不行"等于把最有用的诊断信息扔了。
                $errLine = ($out -split "`r?`n" |
                    Where-Object { $_ -match 'ERROR:|error:|SSLError|ProxyError|ConnectionError' } |
                    Select-Object -First 1)
                throw ("pip 退出码 {0}{1}" -f $LASTEXITCODE, $(if ($errLine) { "：$($errLine.Trim())" } else { '' }))
            }
            $tail = ($out -split "`r?`n" | Where-Object { $_ -match 'Successfully installed' } | Select-Object -Last 1)
            if ($tail) { SayQ "    $($tail.Trim())" 'DarkGray' }
            return $true
        } catch {
            $lastErr = $_.Exception.Message
            SayQ ("    这个源不行（{0}），换下一个" -f $lastErr) 'Yellow'
        }
    }
    throw "pip 装 comfy-mcp / comfy-cli 失败（所有源都不行）。最后一个错误: $lastErr"
}

function Set-ComfyDefaultWorkspace {
    <#
      把用户**真实在用**的 ComfyUI 目录告诉 comfy-cli（`comfy set-default`）。

      为什么必须做：comfy-cli 有自己的"工作区"概念，状态写在
      %LOCALAPPDATA%\comfy-cli\config.ini。实测：不设的话它默认报
      %USERPROFILE%\Documents\comfy\ComfyUI（哪怕那个目录不存在），
      于是 MCP 那边 run_workflow / search_models / nodes 全是错的。

      判据复用 scripts\comfy-path.ps1（与后端 ComfyLocator.kt 同一套）——
      找不到就**如实返回 $false**，绝不瞎指一个目录过去。
    #>
    param(
        [Parameter(Mandatory)][string]$PythonExe,
        [string]$ComfyHome,
        [string]$ProjectRoot,
        [string]$McpProxy,
        [switch]$Quiet
    )

    function SayW([string]$m, [string]$c = 'Gray') { if (-not $Quiet) { Write-Host $m -ForegroundColor $c } }

    if (-not $ComfyHome) {
        $scriptDir = $PSScriptRoot
        $pathScript = Join-Path $scriptDir 'comfy-path.ps1'
        if ((Test-Path -LiteralPath $pathScript)) {
            . $pathScript
            $ComfyHome = Resolve-ComfyHome -ProjectRoot $ProjectRoot
        }
    }
    if (-not $ComfyHome -or -not (Test-ComfyHome $ComfyHome)) {
        SayW '  没找到用户真实在用的 ComfyUI 目录，**没有**动 comfy-cli 的工作区设置。' 'Yellow'
        SayW '   （可以手工指定：pwsh -File scripts\mcp-runtime.ps1 install -ComfyHome <ComfyUI 目录>）' 'DarkGray'
        return $false
    }

    $out = (& $PythonExe -m comfy_cli set-default $ComfyHome 2>&1 | Out-String)
    if ($LASTEXITCODE -ne 0) {
        SayW ("  设置 comfy-cli 工作区失败：{0}" -f $out.Trim()) 'Yellow'
        return $false
    }
    SayW ("  comfy-cli 工作区已指向: {0}" -f $ComfyHome) 'Green'
    return $true
}

# ===========================================================================
#  入口
# ===========================================================================

function Install-McpRuntime {
    <#
      完整装配：Python → pip 包 → comfy-cli 工作区 → 写 state.json。
      返回 status 对象（与 Get-McpRuntimeStatus 同形状）。
    #>
    param(
        [Parameter(Mandatory)][string]$ProjectRoot,
        [string]$McpProxy,
        [string]$ComfyHome,
        [switch]$McpForce,
        [switch]$Quiet
    )

    function SayI([string]$m, [string]$c = 'Gray') { if (-not $Quiet) { Write-Host $m -ForegroundColor $c } }

    $dir = Get-McpRuntimeDir -ProjectRoot $ProjectRoot
    New-Item -ItemType Directory -Force -Path $dir | Out-Null

    SayI ''
    SayI '  装配 MCP 运行时（便携版 Python + comfy-cli + comfy-mcp）' 'White'
    SayI ("  目标目录: {0}" -f $dir) 'DarkGray'

    $py = $null
    if (-not $McpForce) { $py = Get-PythonRuntimePath -McpDir $dir }
    if (-not $py) {
        $py = Install-PortablePython -McpDir $dir -Proxy $McpProxy -Quiet:$Quiet
    } else {
        # 有 python 但可能没装包（上次 pip 失败）：下面按"缺什么补什么"处理
        SayI ("  已有便携版 Python: {0}" -f $py) 'DarkGray'
    }

    $scripts = Join-Path (Split-Path -Parent $py) 'Scripts'
    $needPkgs = $McpForce -or
        (-not (Test-Path -LiteralPath (Join-Path $scripts 'comfy-mcp.exe'))) -or
        (-not (Test-Path -LiteralPath (Join-Path $scripts 'comfy.exe')))
    if ($needPkgs) {
        Install-McpPackages -PythonExe $py -Proxy $McpProxy -Quiet:$Quiet | Out-Null
    } else {
        SayI '  comfy-mcp / comfy-cli 已装好，跳过 pip' 'DarkGray'
    }

    $wsOk = Set-ComfyDefaultWorkspace -PythonExe $py -ComfyHome $ComfyHome -ProjectRoot $ProjectRoot -Proxy $McpProxy -Quiet:$Quiet

    # 装配时**必须**走 -Deep：state.json 里的版本与工作区就是这一步探到的真值，
    # 之后 up 的快档全靠它（否则记录里全是 null，快档就永远判"未就绪"）。
    $st = Get-McpRuntimeStatus -ProjectRoot $ProjectRoot -Deep
    $state = [pscustomobject]@{
        schema          = 'comfyhub-mcp-runtime/1'
        installedAt     = (Get-Date).ToString('o')
        pythonExe       = $st.PythonExe
        pythonVersion   = $st.PythonVersion
        comfyMcpExe     = $st.ComfyMcpExe
        comfyMcpVersion = $st.ComfyMcpVersion
        comfyExe        = $st.ComfyExe
        comfyCliVersion = $st.ComfyCliVersion
        workspace       = $st.Workspace
        workspaceOk     = $wsOk
        specs           = $script:McpRuntimeSpec
    }
    $stateFile = Write-McpRuntimeState -McpDir $dir -State $state
    SayI ("  装配状态写入: {0}" -f $stateFile) 'DarkGray'

    $st = Get-McpRuntimeStatus -ProjectRoot $ProjectRoot -Deep
    if ($st.Ok) { SayI '  MCP 运行时就绪。' 'Green' } else { SayI ("  MCP 运行时还没就绪：{0}" -f ($st.Problems -join '；')) 'Yellow' }
    return $st
}

function Ensure-McpRuntime {
    <#
      comfyhub.ps1 up 用的"够用就好"入口（对标 Ensure-JavaRuntime）：
        · 已就绪 → 直接返回，**不联网、不装包**
        · 没装   → 装配一次（这一步会联网下载约 300MB）
        · 失败   → **不抛异常**，返回带 Problems 的 status
      这是**可选能力**：装不上不该拦住 up（与 Java 完全不同）。
    #>
    param(
        [Parameter(Mandatory)][string]$ProjectRoot,
        [string]$McpProxy,
        [switch]$Quiet,
        [switch]$McpForce,
        [string]$ComfyHome
    )

    $st = Get-McpRuntimeStatus -ProjectRoot $ProjectRoot
    if ($st.Ok -and -not $McpForce) {
        if (-not $Quiet) { Write-Host ("  MCP 运行时已就绪: comfy-mcp {0} / comfy-cli {1}" -f $st.ComfyMcpVersion, $st.ComfyCliVersion) -ForegroundColor Green }
        return $st
    }

    try {
        return (Install-McpRuntime -ProjectRoot $ProjectRoot -Proxy $McpProxy -ComfyHome $ComfyHome -Force:$McpForce -Quiet:$Quiet)
    } catch {
        Write-Host "  !! MCP 运行时没能装好: $($_.Exception.Message)" -ForegroundColor Yellow
        Write-Host '     （这不影响 App 与后端启动；只是 AI 少了一批 ComfyUI 工具）' -ForegroundColor DarkGray
        return (Get-McpRuntimeStatus -ProjectRoot $ProjectRoot)
    }
}

function Show-McpRuntimeReport {
    <#
      体检表输出。**刻意不返回对象** —— 只往控制台写。
      （原因：这函数是被 `... | Out-Null` 调的，一旦它返回 status 对象，
        那个对象会顺着管道跑到调用者的 stdout 上，把 doctor 的输出糊掉一屏。）
      需要 status 对象请直接调 Get-McpRuntimeStatus。
    #>
    param(
        [Parameter(Mandatory)][string]$ProjectRoot,
        [switch]$OnlyProblems,
        # -Deep：真去问 comfy-cli（6 秒）。doctor 要看真相，up 走快档。
        [switch]$Deep
    )

    $st = Get-McpRuntimeStatus -ProjectRoot $ProjectRoot -Deep:$Deep
    $mark = if ($st.Ok) { 'OK  ' } else { '未就绪' }
    $color = if ($st.Ok) { 'Green' } else { 'Yellow' }
    Write-Host ("  {0,-28} {1}  {2}" -f 'MCP 运行时 (comfy-mcp)', $mark, $st.Dir) -ForegroundColor $color
    if ($st.PythonExe) { Write-Host ("  {0,-28}      python {1}" -f '', $st.PythonVersion) -ForegroundColor DarkGray }
    if ($st.ComfyMcpVersion) { Write-Host ("  {0,-28}      comfy-mcp {1} / comfy-cli {2}" -f '', $st.ComfyMcpVersion, $st.ComfyCliVersion) -ForegroundColor DarkGray }
    if ($st.Workspace) {
        $wsColor = if ($st.WorkspaceOk) { 'DarkGray' } else { 'Yellow' }
        Write-Host ("  {0,-28}      工作区 {1}" -f '', $st.Workspace) -ForegroundColor $wsColor
    }
    if ($st.Problems.Count -gt 0 -and -not ($OnlyProblems -and $st.Ok)) {
        foreach ($p in $st.Problems) { Write-Host ("      · {0}" -f $p) -ForegroundColor Yellow }
        Write-Host '      装配/修复: pwsh -File scripts\mcp-runtime.ps1 install' -ForegroundColor Cyan
    }
}

# ===========================================================================
#  独立入口：pwsh -File scripts\mcp-runtime.ps1 <action>
# ===========================================================================

function Invoke-McpRuntimeCli {
    param(
        [Parameter(Mandatory)][string]$ProjectRoot,
        [string]$McpAction = 'status',
        [string]$McpProxy,
        [string]$ComfyHome,
        [switch]$McpForce,
        # status 默认走快档；`status -Deep` 真去问 comfy-cli（6 秒，但看得到真相）
        [switch]$Deep
    )

    switch ($McpAction.ToLowerInvariant()) {
        'status' {
            Show-McpRuntimeReport -ProjectRoot $ProjectRoot -Deep:$Deep
            return 0
        }
        'install' {
            try {
                $st = Install-McpRuntime -ProjectRoot $ProjectRoot -Proxy $McpProxy -ComfyHome $ComfyHome -Force:$McpForce
                return $(if ($st.Ok) { 0 } else { 1 })
            } catch {
                Write-Host "  装配失败: $($_.Exception.Message)" -ForegroundColor Red
                return 1
            }
        }
        default {
            Write-Host "用法: pwsh -File scripts\mcp-runtime.ps1 [status|install] [-Force] [-ComfyHome <目录>] [-Proxy <url>]" -ForegroundColor Yellow
            return 2
        }
    }
}

# ===========================================================================
#  直接跑本文件时（`pwsh -File scripts\mcp-runtime.ps1 install`）才执行；
#  被 comfyhub.ps1 dot-source 时**不能**自动跑 —— 那是加载函数，不是执行动作。
#
#  判定用 $PSBoundParameters：dot-source 时不绑任何参数（空），
#  `pwsh -File` 时位置参数/开关至少绑进一个。比看 $MyInvocation.InvocationName
#  可靠 —— 后者在"被别的脚本 dot-source 再间接调用"这类写法下会变。
# ===========================================================================
if ($PSBoundParameters.Count -gt 0) {
    $rtRoot = Split-Path -Parent (Split-Path -Parent $PSCommandPath)

    # 让 Resolve-ComfyHome / Test-ComfyHome 可用（本文件不自己重复一份判据）
    $rtPathScript = Join-Path (Split-Path -Parent $PSCommandPath) 'comfy-path.ps1'
    if (Test-Path -LiteralPath $rtPathScript) { . $rtPathScript }

    # 复用 runtime-deps.ps1 里的 Test-DirWritable / Invoke-FileDownload / Test-UrlReachable /
    # Initialize-Tls12 —— 同一个仓库里不该有两份下载与可写性判定。
    $rtDeps = Join-Path (Split-Path -Parent $PSCommandPath) 'runtime-deps.ps1'
    if (Test-Path -LiteralPath $rtDeps) { . $rtDeps }

    $rtProxy = if ($McpProxy) { $McpProxy } else { $env:COMFYHUB_HTTPS_PROXY }
    exit (Invoke-McpRuntimeCli -ProjectRoot $rtRoot -McpAction $McpAction -McpProxy $rtProxy -ComfyHome $ComfyHome -McpForce:$McpForce -Deep:$Deep)
}


