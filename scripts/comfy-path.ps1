# ComfyUI 安装位置解析（用户"其他建议"第 3 条：发行版怎么自动找到 comfy 在哪）
#
# 与后端 `ComfyLocator.kt` 是**同一套判据**（顺序也一致）：脚本与 App 各自要能用，
# 但两边必须给出一致答案，否则会出现"App 说找到了、脚本说找不到"这种最难查的问题。
#
# 被 dot-source 使用：
#   . scripts\comfy-path.ps1
#   $comfy = Resolve-ComfyHome            # 找不到返回 $null
#   $out   = Resolve-ComfyOutputDir       # 找不到返回 $null
#
# 判据刻意严：**只有特征文件齐全才算**。认错目录的代价是"捕获一条也收不到"，
# 而用户完全不知道为什么，所以宁可返回 $null 让他手填。

function Test-ComfyHome([string]$Path) {
    if ([string]::IsNullOrWhiteSpace($Path)) { return $false }
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) { return $false }
    $hasMain = Test-Path -LiteralPath (Join-Path $Path 'main.py') -PathType Leaf
    $hasPkg = Test-Path -LiteralPath (Join-Path $Path 'comfy') -PathType Container
    $hasOutput = Test-Path -LiteralPath (Join-Path $Path 'output') -PathType Container
    $hasModels = Test-Path -LiteralPath (Join-Path $Path 'models') -PathType Container
    $hasInput = Test-Path -LiteralPath (Join-Path $Path 'input') -PathType Container
    # 源码形态：main.py + comfy/；任何形态（Desktop / 整合包）：output/ 且（models/ 或 input/）
    return (($hasMain -and $hasPkg) -or ($hasOutput -and ($hasModels -or $hasInput)))
}

function Get-FixedDriveRoots {
    <#  这台机器上**真正存在**的固定盘根（`C:\`、`D:\`、`E:\` …）。

        ⚠ 别写死盘符：以前这里是 `D:\ComfyUI` / `D:\Comfy-Desktop` / 整个 `D:\` 顶层，
        只有**开发机**的布局能被找到；换台机器（ComfyUI 装在 C: 或 E:）就永远"找不到"。
        用 .NET 的 DriveInfo 只留 `Fixed`：网络映射盘 / 光驱不碰（断开的网络盘会卡）。 #>
    try {
        return @(
            [System.IO.DriveInfo]::GetDrives() |
                Where-Object { $_.DriveType -eq [System.IO.DriveType]::Fixed } |
                ForEach-Object { $_.RootDirectory.FullName }
        )
    } catch {
        return @()
    }
}

function Resolve-ComfyHome {
    [CmdletBinding()]
    param([string]$ProjectRoot = $null)

    # 1) 显式参数 / 环境变量
    foreach ($candidate in @($env:COMFYHUB_COMFY_HOME, $env:COMFYUI_HOME, $env:COMFYUI_PATH)) {
        if (Test-ComfyHome $candidate) { return (Resolve-Path -LiteralPath $candidate).Path }
    }

    # 2) 项目内的 comfyui（源码树 / 发布包都会带上这一份）
    if ($ProjectRoot) {
        $local = Join-Path $ProjectRoot 'comfyui'
        if (Test-ComfyHome $local) { return (Resolve-Path -LiteralPath $local).Path }
    }

    # 3) 常见安装位置（**每个固定盘**的盘根 + 用户目录，最后才在几个顶层目录里翻一层）
    $fixed = @()
    foreach ($drive in (Get-FixedDriveRoots)) {
        $fixed += (Join-Path $drive 'ComfyUI')
        $fixed += (Join-Path $drive 'ComfyUI_windows_portable\ComfyUI')
    }
    $fixed += @(
        (Join-Path $env:USERPROFILE 'Documents\ComfyUI'),
        (Join-Path $env:USERPROFILE 'ComfyUI'),
        # ComfyUI Desktop（新版）常把源码检出放在 Documents\comfy\ComfyUI
        (Join-Path $env:USERPROFILE 'Documents\comfy\ComfyUI'),
        (Join-Path $env:LOCALAPPDATA 'Programs\@comfyorgcomfyui-electron')
    )
    foreach ($candidate in $fixed) {
        if (Test-ComfyHome $candidate) { return (Resolve-Path -LiteralPath $candidate).Path }
    }

    # 4) ComfyUI **Desktop** 的嵌套布局（2026-09-21 实测踩到）
    #    Desktop 把源码检出放在 <应用目录>\ComfyUI-Installs\<实例>\ComfyUI 下，
    #    只在顶层翻一层永远找不到它 —— 而用户的 ComfyUI 恰恰就是这么装的。
    $desktopRoots = @(
        (Join-Path $env:USERPROFILE 'AppData\Local\Programs\@comfyorgcomfyui-electron'),
        (Join-Path $env:USERPROFILE 'Documents\comfy'),
        (Join-Path $env:LOCALAPPDATA 'Comfy-Desktop')
    )
    foreach ($drive in (Get-FixedDriveRoots)) {
        $desktopRoots += (Join-Path $drive 'Comfy-Desktop')
    }
    foreach ($root in $desktopRoots) {
        if (-not (Test-Path -LiteralPath $root -PathType Container)) { continue }
        $installs = Join-Path $root 'ComfyUI-Installs'
        foreach ($base in @($installs, $root)) {
            if (-not (Test-Path -LiteralPath $base -PathType Container)) { continue }
            $hit = Get-ChildItem -LiteralPath $base -Directory -ErrorAction SilentlyContinue |
                ForEach-Object { Join-Path $_.FullName 'ComfyUI' } |
                Where-Object { Test-ComfyHome $_ } |
                Select-Object -First 1
            if ($hit) { return (Resolve-Path -LiteralPath $hit).Path }
        }
    }

    # 5) 秋叶整合包 / 便携包常见的 *comfy* 顶层目录（只翻一层，且**每个固定盘的盘根**都看）
    $topRoots = @(
        (Join-Path $env:USERPROFILE 'Desktop'),
        (Join-Path $env:USERPROFILE 'Documents')
    ) + (Get-FixedDriveRoots)
    foreach ($root in $topRoots) {
        if (-not (Test-Path -LiteralPath $root -PathType Container)) { continue }
        $hit = Get-ChildItem -LiteralPath $root -Directory -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -match 'comfy' } |
            Where-Object { Test-ComfyHome $_.FullName } |
            Select-Object -First 1
        if ($hit) { return $hit.FullName }
    }
    return $null
}

function Find-ComfyPython {
    <#  找一份能跑 ComfyUI 相关小脚本的 python.exe。

        顺序：`$env:COMFYHUB_PYTHON` → PATH 里的 python → **ComfyUI 自己带的**那份
        （ComfyUI 装在哪由 [`Resolve-ComfyHome`] 现探，**不写死盘符** —— 以前这里列的是
        `D:\Comfy-Desktop\...`，换台机器就找不到，2026-11-10 复查改掉）。 #>
    [CmdletBinding()]
    param([string]$ProjectRoot = $null)

    if ($env:COMFYHUB_PYTHON -and (Test-Path -LiteralPath $env:COMFYHUB_PYTHON -PathType Leaf)) {
        return $env:COMFYHUB_PYTHON
    }
    $cmd = Get-Command python -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }

    $comfy = Resolve-ComfyHome -ProjectRoot $ProjectRoot
    if (-not $comfy) { return $null }
    $parent = Split-Path -Parent $comfy
    $cands = @(
        (Join-Path $comfy 'python_embeded\python.exe'),          # 便携包（python 在根下）
        (Join-Path $comfy 'python_embeded\python_embeded\python.exe'),
        (Join-Path $comfy 'venv\Scripts\python.exe'),
        (Join-Path $comfy '.venv\Scripts\python.exe'),
        (Join-Path $comfy 'standalone-env\python.exe'),          # Desktop 的独立环境
        (Join-Path $parent 'python_embeded\python.exe')
    )
    foreach ($c in $cands) {
        if ($c -and (Test-Path -LiteralPath $c -PathType Leaf)) { return $c }
    }
    return $null
}

function Resolve-ComfyOutputDir {
    [CmdletBinding()]
    param([string]$ProjectRoot = $null)

    # 显式配置优先（App 里填过就用它）
    if (-not [string]::IsNullOrWhiteSpace($env:COMFYHUB_COMFY_OUTPUT)) {
        if (Test-Path -LiteralPath $env:COMFYHUB_COMFY_OUTPUT -PathType Container) {
            return (Resolve-Path -LiteralPath $env:COMFYHUB_COMFY_OUTPUT).Path
        }
    }
    $home_ = Resolve-ComfyHome -ProjectRoot $ProjectRoot
    if (-not $home_) { return $null }
    $out = Join-Path $home_ 'output'
    if (Test-Path -LiteralPath $out -PathType Container) { return (Resolve-Path -LiteralPath $out).Path }
    return $null
}
