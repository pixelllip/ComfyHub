# 抠图（matting）质量探针 —— 把"AI 软遮罩 vs 颜色近似"量成数字，而不是凭图说话。
#
# 为什么是 PowerShell 包一层：真正干活的 Python 要跑在 **ComfyUI 的 venv** 里
# （只有它装了 torch / timm / cv2 / einops），而"哪个 python 才是那份 venv"这件事
# 本机有两处候选、且**第一个候选往往没有 torch**（standalone-env 就是）——
# 所以这里不是"取第一个存在的"，而是**取第一个真能 import torch 的**。
#
# ⚠ 它**不需要 ComfyUI 在跑**（只借 venv 与 models\rembg 里的模型文件）。
#
# 用法：
#   pwsh -File scripts\matte-probe.ps1 run -Subject <人物图> [-Models BEN2,isnet-anime] [-Refine] [-Background <底图>]
#   pwsh -File scripts\matte-probe.ps1 measure [-Out <目录>] [-Comparison <底图>]
#
# 判据（详见 scripts\e2e\matte_probe.py 的文件头）：**硬剪影的"软像素占比"恒为 0**，
# 所以"人身上一块块变透明 / 细发丝被吃"看的是**前景占比**，"绿边清没清"看的是
# **平灰底边界溢色**（底不带绿 → 量到的绿只能来自人物自己）。

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [ValidateSet('run', 'measure')]
    [string]$Action,

    # run 用：要抠的那张人物图（真抠图模型都吃 PNG/JPG 路径）
    [string]$Subject,

    # 产物目录（默认放 .run\matte-research —— 这个目录不进版本库）
    [string]$Out = (Join-Path (Get-Location).Path '.run\matte-research'),

    # run 用：模型清单。BEN2 / RMBG-1.4 读 ComfyUI 的 models\rembg（本机已在盘上）；
    # 其余名字走 rembg 包的会话（如 isnet-anime，首次会下到 ~/.rembg\models）
    [string]$Models = 'BEN2',

    # run 用：BEN2 多跑一档 refine_foreground
    [switch]$Refine,

    # 合成用的真背景（可选；给了才会出 comp-*.png）
    [string]$Background,

    # measure 用：并排对照图的背景
    [string]$Comparison,

    # ComfyUI 安装目录（默认自动探测，与后端同一套判据）
    [string]$ComfyHome
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

function Say([string]$Text, [string]$Color = 'Gray') { Write-Host $Text -ForegroundColor $Color }

# --- ① ComfyUI 在哪（与后端 ComfyLocator / scripts\comfy-path.ps1 同一套判据）---
if (-not $ComfyHome) {
    foreach ($c in @($env:COMFYHUB_COMFY_HOME, $env:COMFYUI_HOME, $env:COMFYUI_PATH)) {
        if ($c -and (Test-Path $c)) { $ComfyHome = $c; break }
    }
}
if (-not $ComfyHome) {
    . (Join-Path $PSScriptRoot 'comfy-path.ps1')
    $ComfyHome = Resolve-ComfyHome -ProjectRoot $root
}
if (-not $ComfyHome) {
    Say '找不到 ComfyUI 安装目录（判据：main.py + comfy\，或 output\ + models\|input\）。' 'Red'
    Say '用 -ComfyHome <目录> 或设 COMFYHUB_COMFY_HOME 显式指定。' 'Red'
    exit 1
}
Say "ComfyUI: $ComfyHome"

# --- ② 找那个**真装了 torch** 的 python ---
$venvHint = Get-ChildItem -Path (Join-Path (Split-Path -Parent $ComfyHome) '') -Filter 'python.exe' -Recurse -Depth 3 -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match '\\\.venv\\Scripts\\python\.exe$' } | Select-Object -First 1
$candidates = @(
    $env:COMFYHUB_PYTHON,
    (Get-Command python -ErrorAction SilentlyContinue).Source,
    (Join-Path $ComfyHome '.venv\Scripts\python.exe'),
    (Join-Path (Split-Path -Parent $ComfyHome) '.venv\Scripts\python.exe'),
    (Join-Path (Split-Path -Parent (Split-Path -Parent $ComfyHome)) 'ComfyUI\.venv\Scripts\python.exe'),
    (Join-Path (Split-Path -Parent (Split-Path -Parent $ComfyHome)) 'standalone-env\python.exe'),
    $venvHint.FullName
)
$python = $null
foreach ($c in $candidates) {
    if (-not $c -or -not (Test-Path $c)) { continue }
    # 判据是"能不能 import torch" —— 第一个存在的往往不是能跑模型的那一个
    & $c -c 'import torch, timm' 2>$null | Out-Null
    if ($LASTEXITCODE -eq 0) { $python = $c; break }
}
if (-not $python) {
    Say '找不到装了 torch + timm 的 python（comfy-mcp / rembg 的模型都跑不起来）。' 'Red'
    Say '用 COMFYHUB_PYTHON 显式指定 ComfyUI 的 venv python。' 'Red'
    exit 1
}
Say "python: $python"

$probe = Join-Path $PSScriptRoot 'e2e\matte_probe.py'
if (-not (Test-Path $probe)) { Say "缺 $probe" 'Red'; exit 1 }
if (-not (Test-Path $Out)) { New-Item -ItemType Directory -Path $Out -Force | Out-Null }

$env:PYTHONIOENCODING = 'utf-8'
if ($Action -eq 'run') {
    if (-not $Subject) { Say 'run 要 -Subject <人物图>' 'Red'; exit 1 }
    if (-not (Test-Path $Subject)) { Say "找不到人物图：$Subject" 'Red'; exit 1 }
    $argv = @($probe, 'run', '--subject', (Resolve-Path -LiteralPath $Subject).Path,
        '--out', $Out, '--comfy-home', $ComfyHome, '--models', $Models)
    if ($Refine) { $argv += '--refine' }
    if ($Background) { $argv += @('--background', (Resolve-Path -LiteralPath $Background).Path) }
    & $python @argv
} else {
    $argv = @($probe, 'measure', '--out', $Out)
    if ($Subject) { $argv += @('--subject', (Resolve-Path -LiteralPath $Subject).Path) }
    if ($Comparison) { $argv += @('--comparison', (Resolve-Path -LiteralPath $Comparison).Path) }
    & $python @argv
}
exit $LASTEXITCODE
