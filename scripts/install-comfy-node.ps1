<#
.SYNOPSIS
    把 ComfyHub 的捕获扩展（comfyui\comfyhub_capture）安装到本机 ComfyUI 的 custom_nodes 里，或从那里卸载。

.DESCRIPTION
    安装方式有两种：
      * Junction（默认）：在 <ComfyUI>\custom_nodes\comfyhub_capture 建一个目录联接指向仓库源码。
        改仓库里的代码立刻生效，不用重新安装（要求同一个卷，否则请用 -Mode Copy）。
      * Copy：把节点目录整个复制过去（跨盘、或想固定一份快照时用）。

    安装后需要重启 ComfyUI 才会生效；之后每次跑完工作流，运行记录会自动推到 ComfyHub 后端，
    在 ComfyHub 的「画廊」里就能看到。

    本脚本只写 <ComfyUI>\custom_nodes\comfyhub_capture 这一个位置；卸载时只会删掉
    这个链接/副本本身，绝不会删仓库里的源码。

.PARAMETER ComfyUIPath
    ComfyUI 仓库根目录（里面有 main.py / server.py / custom_nodes），或者直接给 custom_nodes 目录。
    不传则自动探测。

.PARAMETER Uninstall
    卸载：删除 custom_nodes 里的 comfyhub_capture（只删链接或副本，不动源码）。

.PARAMETER Mode
    Copy 或 Junction，默认 Junction。

.PARAMETER Force
    目标已存在时强制覆盖（先确认它是链接/软链再删除，不会删掉用户的真实目录）。

.PARAMETER ListOnly
    只做探测和参数校验，打印探测结果后退出，不做任何写入（用于安全检查）。

.EXAMPLE
    pwsh -File scripts\install-comfy-node.ps1
    pwsh -File scripts\install-comfy-node.ps1 -Uninstall
    pwsh -File scripts\install-comfy-node.ps1 -ComfyUIPath D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI -Mode Copy
#>
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [string]$ComfyUIPath,
    [switch]$Uninstall,
    [ValidateSet('Copy', 'Junction')]
    [string]$Mode = 'Junction',
    [switch]$Force,
    [switch]$ListOnly
)

$ErrorActionPreference = 'Stop'

$ProjectRoot = Split-Path -Parent $PSScriptRoot
$NodeSource = Join-Path $ProjectRoot 'comfyui\comfyhub_capture'
$NodeName = 'comfyhub_capture'
$LogTag = '[ComfyHub]'

# 明确保护的路径：本脚本永远不会往这些地方写东西（防止误操作到正在用的 ComfyUI）
$ProtectedRoots = @(
    'D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI\custom_nodes'
)

function Write-Head($text) { Write-Host $text -ForegroundColor Cyan }
function Write-Ok($text) { Write-Host "  $text" -ForegroundColor Green }
function Write-Warn2($text) { Write-Host "  $text" -ForegroundColor Yellow }
function Write-Err($text) { Write-Host "  $text" -ForegroundColor Red }

# ---------------------------------------------------------------------------
#  探测 ComfyUI 根目录
# ---------------------------------------------------------------------------

function Test-ComfyRoot {
    <# 判定一个目录是不是 ComfyUI 仓库根：必须有 main.py + server.py，且有 custom_nodes #>
    param([string]$Path)
    if (-not $Path) { return $false }
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) { return $false }
    $hasMain = Test-Path -LiteralPath (Join-Path $Path 'main.py') -PathType Leaf
    $hasServer = Test-Path -LiteralPath (Join-Path $Path 'server.py') -PathType Leaf
    $hasNodes = Test-Path -LiteralPath (Join-Path $Path 'custom_nodes') -PathType Container
    return ($hasMain -and $hasServer -and $hasNodes)
}

function Test-CustomNodesDir {
    <# 判定一个目录是不是 custom_nodes：有 websocket_image_save.py，或同级有 main.py #>
    param([string]$Path)
    if (-not $Path) { return $false }
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) { return $false }
    if (Test-Path -LiteralPath (Join-Path $Path 'websocket_image_save.py') -PathType Leaf) { return $true }
    $parent = Split-Path -Parent $Path
    if ($parent -and (Test-Path -LiteralPath (Join-Path $parent 'main.py') -PathType Leaf)) { return $true }
    return $false
}

function Resolve-ComfyRoot {
    <# 依次尝试候选路径，返回 @{ Root = <仓库根>; Why = <为什么选它> }，找不到返回 $null #>
    $candidates = New-Object System.Collections.Generic.List[object]
    $seen = New-Object System.Collections.Generic.HashSet[string]

    $add = {
        param($path, $why)
        if (-not $path) { return }
        $full = $null
        try { $full = [System.IO.Path]::GetFullPath($path) } catch { return }
        if ($seen.Add($full.ToLowerInvariant())) {
            $candidates.Add([pscustomobject]@{ Path = $full; Why = $why }) | Out-Null
        }
    }

    & $add $env:COMFYHUB_COMFYUI '环境变量 COMFYHUB_COMFYUI'
    & $add 'D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI' 'ComfyUI Desktop 默认安装位置'
    & $add 'D:\Comfy-Desktop\ComfyUI' 'ComfyUI Desktop 常见位置'
    & $add 'E:\ComfyUI' '常见自定义位置'
    & $add 'C:\ComfyUI' '常见自定义位置'
    if ($env:APPDATA) { & $add (Join-Path $env:APPDATA 'ComfyUI') 'ComfyUI Desktop 的 %APPDATA% 目录' }
    if ($env:USERPROFILE) { & $add (Join-Path $env:USERPROFILE 'ComfyUI') '用户目录下的 ComfyUI' }

    foreach ($c in $candidates) {
        if (Test-ComfyRoot -Path $c.Path) {
            return [pscustomobject]@{ Root = $c.Path; Why = $c.Why }
        }
        # 传进来的可能就是 custom_nodes 或带 ComfyUI 子目录的父目录
        if (Test-CustomNodesDir -Path $c.Path) {
            return [pscustomobject]@{ Root = (Split-Path -Parent $c.Path); Why = "$($c.Why)（给的是 custom_nodes）" }
        }
        $nested = Join-Path $c.Path 'ComfyUI'
        if (Test-ComfyRoot -Path $nested) {
            return [pscustomobject]@{ Root = $nested; Why = "$($c.Why)（下一级 ComfyUI 子目录）" }
        }
    }

    # 兜底：扫 D:\ / C:\ / E:\ 一层深，找带 websocket_image_save.py 的 custom_nodes
    foreach ($drive in @('D:\', 'C:\', 'E:\')) {
        if (-not (Test-Path -LiteralPath $drive -PathType Container)) { continue }
        $tops = @()
        try { $tops = Get-ChildItem -LiteralPath $drive -Directory -Force -ErrorAction SilentlyContinue } catch { continue }
        foreach ($top in $tops) {
            $cn = Join-Path $top.FullName 'custom_nodes'
            if (Test-CustomNodesDir -Path $cn) {
                return [pscustomobject]@{ Root = $top.FullName; Why = "扫描 $drive 一层发现 custom_nodes（含 websocket_image_save.py）" }
            }
        }
    }

    return $null
}

function Get-LinkInfo {
    <# 判断一个已存在的目标是不是链接 / 联接 / 软链；返回 @{ Exists; IsLink; Kind } #>
    param([string]$Path)
    $info = [pscustomobject]@{ Exists = $false; IsLink = $false; Kind = '' }
    if (-not (Test-Path -LiteralPath $Path)) { return $info }
    $info.Exists = $true
    try {
        $item = Get-Item -LiteralPath $Path -Force
        if ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) {
            $info.IsLink = $true
            $linkType = $null
            try { $linkType = $item.LinkType } catch { $linkType = $null }
            if (-not $linkType) { $linkType = 'ReparsePoint' }
            $info.Kind = $linkType
        }
    } catch {
        Write-Warn2 "无法读取 $Path 的属性: $($_.Exception.Message)"
    }
    return $info
}

function Test-Protected {
    param([string]$Path)
    foreach ($p in $ProtectedRoots) {
        if ($Path.ToLowerInvariant().StartsWith($p.ToLowerInvariant())) { return $p }
    }
    return $null
}

# ---------------------------------------------------------------------------
#  前置检查
# ---------------------------------------------------------------------------

Write-Head "$LogTag ComfyHub 捕获扩展 · 安装/卸载"
Write-Host ""

if (-not (Test-Path -LiteralPath $NodeSource -PathType Container)) {
    Write-Err "找不到节点源码目录: $NodeSource"
    exit 2
}
if (-not (Test-Path -LiteralPath (Join-Path $NodeSource '__init__.py') -PathType Leaf)) {
    Write-Err "节点目录里缺少 __init__.py: $NodeSource"
    exit 2
}
if (-not (Test-Path -LiteralPath (Join-Path $NodeSource 'capture_core.py') -PathType Leaf)) {
    Write-Err "节点目录里缺少 capture_core.py: $NodeSource"
    exit 2
}
Write-Ok "节点源码: $NodeSource"

# 解析 ComfyUI 根目录
$resolved = $null
if ($ComfyUIPath) {
    $given = $ComfyUIPath
    if (-not (Test-Path -LiteralPath $given -PathType Container)) {
        Write-Err "-ComfyUIPath 指向的目录不存在: $given"
        exit 2
    }
    $givenFull = [System.IO.Path]::GetFullPath($given)
    if (Test-ComfyRoot -Path $givenFull) {
        $resolved = [pscustomobject]@{ Root = $givenFull; Why = '来自 -ComfyUIPath' }
    } elseif (Test-CustomNodesDir -Path $givenFull) {
        $resolved = [pscustomobject]@{ Root = (Split-Path -Parent $givenFull); Why = '来自 -ComfyUIPath（给的是 custom_nodes）' }
    } else {
        $nested = Join-Path $givenFull 'ComfyUI'
        if (Test-ComfyRoot -Path $nested) {
            $resolved = [pscustomobject]@{ Root = $nested; Why = '来自 -ComfyUIPath（下一级 ComfyUI）' }
        } else {
            Write-Err "-ComfyUIPath 不是有效的 ComfyUI 目录（需要同时有 main.py / server.py / custom_nodes）: $givenFull"
            exit 2
        }
    }
} else {
    $resolved = Resolve-ComfyRoot
}

Write-Host ""
if ($resolved) {
    Write-Ok "ComfyUI 根目录: $($resolved.Root)"
    Write-Ok "选择依据    : $($resolved.Why)"
} else {
    Write-Err "没能自动找到 ComfyUI，请用 -ComfyUIPath 明确指定，例如："
    Write-Err '  pwsh -File scripts\install-comfy-node.ps1 -ComfyUIPath D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI'
    exit 2
}

$customNodes = Join-Path $resolved.Root 'custom_nodes'
if (-not (Test-Path -LiteralPath $customNodes -PathType Container)) {
    Write-Err "custom_nodes 目录不存在: $customNodes"
    exit 2
}

$target = Join-Path $customNodes $NodeName
Write-Ok "安装目标    : $target"
Write-Ok "安装方式    : $Mode"
Write-Host ""

# 安全检查：自动探测到"你正在用的那个 ComfyUI"时，不默默往里写。
# 显式给了 -ComfyUIPath 或加了 -Force 就照做（用户已经表达清楚了意图）。
$protected = Test-Protected -Path $target
$explicitPath = [bool]$ComfyUIPath
if ($protected -and -not $ListOnly) {
    if ($explicitPath -or $Force) {
        Write-Warn2 "注意：$target 位于 $protected。"
        Write-Warn2 "装完需要重启 ComfyUI 才会加载；卸载用 -Uninstall。"
    } else {
        Write-Err "自动探测到 $protected，为安全起见没有直接写入：$target"
        Write-Err "确认要装到这里的话，加 -Force 再跑一次："
        Write-Err '  pwsh -File scripts\install-comfy-node.ps1 -Force'
        exit 3
    }
}

if ($ListOnly) {
    Write-Warn2 "-ListOnly：只做探测，不写入任何文件。"
    exit 0
}

# ---------------------------------------------------------------------------
#  卸载
# ---------------------------------------------------------------------------

if ($Uninstall) {
    $info = Get-LinkInfo -Path $target
    if (-not $info.Exists) {
        Write-Warn2 "没有找到 $target，无需卸载。"
        exit 0
    }
    if ($info.IsLink) {
        Write-Ok "目标是 $($info.Kind)，只删除链接本身，不动源码。"
        if ($PSCmdlet.ShouldProcess($target, '删除链接')) {
            # 目录联接用 cmd 的 rmdir 最稳（Remove-Item 对联接有时会递归删源目录）
            if ($info.Kind -eq 'Junction') {
                & cmd.exe /c rmdir "$target" | Out-Null
            } else {
                Remove-Item -LiteralPath $target -Force -Recurse -ErrorAction SilentlyContinue
            }
        }
    } else {
        Write-Warn2 "目标是一个真实目录（不是链接），可能是之前用 -Mode Copy 装的。"
        if ($PSCmdlet.ShouldProcess($target, '删除副本')) {
            Remove-Item -LiteralPath $target -Recurse -Force
        }
    }
    if (Test-Path -LiteralPath $target) {
        Write-Err "删除失败，请检查权限或是否有进程占用：$target"
        exit 1
    }
    Write-Host ""
    Write-Ok "已卸载：$target"
    Write-Ok "重启 ComfyUI 后生效。"
    exit 0
}

# ---------------------------------------------------------------------------
#  安装
# ---------------------------------------------------------------------------

$existing = Get-LinkInfo -Path $target
if ($existing.Exists) {
    if (-not $Force) {
        Write-Err "目标已存在：$target"
        if ($existing.IsLink) {
            Write-Err "它是 $($existing.Kind)。要覆盖请加 -Force（只会删这个链接，不会删源码）。"
        } else {
            Write-Err "它是一个真实目录（不是链接）。要覆盖请加 -Force，或先手动处理。"
        }
        exit 4
    }
    Write-Warn2 "目标已存在，-Force 生效：先移除旧的 $($existing.Kind)。"
    if ($existing.IsLink) {
        if ($existing.Kind -eq 'Junction') {
            & cmd.exe /c rmdir "$target" | Out-Null
        } else {
            Remove-Item -LiteralPath $target -Force -Recurse -ErrorAction SilentlyContinue
        }
    } else {
        # 真实目录同样只在 -Force 下删；这里再确认它不是仓库源码本身
        $srcFull = [System.IO.Path]::GetFullPath($NodeSource).ToLowerInvariant()
        if ([System.IO.Path]::GetFullPath($target).ToLowerInvariant() -eq $srcFull) {
            Write-Err "目标是源码目录本身，拒绝删除。"
            exit 3
        }
        Remove-Item -LiteralPath $target -Recurse -Force
    }
    if (Test-Path -LiteralPath $target) {
        Write-Err "旧目标删除失败（可能被占用）：$target"
        exit 1
    }
}

if ($Mode -eq 'Junction') {
    $srcRoot = [System.IO.Path]::GetPathRoot([System.IO.Path]::GetFullPath($NodeSource))
    $dstRoot = [System.IO.Path]::GetPathRoot([System.IO.Path]::GetFullPath($target))
    if ($srcRoot -ne $dstRoot) {
        Write-Err "Junction 要求源和目标在同一个卷（源码在 $srcRoot，目标是 $dstRoot）。"
        Write-Err "请改用 -Mode Copy（复制过去，跨卷可用）。"
        exit 5
    }
    if ($PSCmdlet.ShouldProcess($target, "创建目录联接 -> $NodeSource")) {
        New-Item -ItemType Junction -Path $target -Target $NodeSource -ErrorAction Stop | Out-Null
    }
    Write-Ok "已创建目录联接（改仓库代码立即生效，无需重装）。"
} else {
    if ($PSCmdlet.ShouldProcess($target, "复制 $NodeSource")) {
        Copy-Item -LiteralPath $NodeSource -Destination $target -Recurse -Force
        # 复制模式不需要把 __pycache__ 带过去
        $cache = Join-Path $target '__pycache__'
        if (Test-Path -LiteralPath $cache) { Remove-Item -LiteralPath $cache -Recurse -Force -ErrorAction SilentlyContinue }
    }
    Write-Ok "已复制节点目录（改仓库代码需要重新执行本脚本才生效）。"
}

if (-not (Test-Path -LiteralPath (Join-Path $target '__init__.py') -PathType Leaf)) {
    Write-Err "安装后没有在 $target 看到 __init__.py，请检查。"
    exit 1
}

Write-Host ""
Write-Ok "安装完成：$target"
Write-Host ""
Write-Head "$LogTag 接下来"
Write-Host "  1) 重启 ComfyUI（扩展在启动时加载，热改不生效）。"
Write-Host "  2) 启动日志里应出现一行： [ComfyHub] 捕获扩展已启用 -> http://127.0.0.1:8080"
Write-Host "  3) 之后每跑完一个工作流，运行记录会自动推给 ComfyHub，在 ComfyHub 的「画廊」里查看。"
Write-Host "  4) 后端没开也没关系：推送失败只写日志，ComfyUI 行为完全不变。"
Write-Host ""
Write-Warn2 "想改后端地址：设置环境变量 COMFYHUB_URL，或把 config.example.json 复制成 config.json 再改。"
Write-Warn2 "想临时关掉：设 COMFYHUB_CAPTURE=0 后重启 ComfyUI。"
Write-Warn2 "不想用了：pwsh -File scripts\install-comfy-node.ps1 -Uninstall"
Write-Host ""
