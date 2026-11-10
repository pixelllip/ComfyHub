<#
.SYNOPSIS
  活实例端到端：项目自带的 inpaint 工作流**真的能在本机 ComfyUI 上跑**，而且**遮罩外不会被重画**。

.DESCRIPTION
  这是"保背景换人物 / 换背景保人物"那条路的验收脚本（2026-09-26）。它验的不是后端接口，
  而是**那份随项目分发的工作流 + "白=重绘区"这套语义**在真机上成不成立：

    ① 取一张真底图（默认取画廊里最新的一张图片产物；也可 -BgImage 指定文件）；
    ② 按 -Region 造一张遮罩（白=重绘区，边界做羽化 —— 与 `comfy_make_mask` 同一套语义）；
    ③ 把底图与遮罩写进 ComfyUI 的 input 目录；
    ④ 直接 POST /prompt 提交 comfyui\workflows\comfyhub_inpaint_*.json（覆盖图 / 尺寸 / 提示词 / seed）；
    ⑤ 等 /history 出产物；
    ⑥ **量化验收**：遮罩内平均像素差要**大**（真的重画了），遮罩外要**小**（背景保住了）。

  为什么"遮罩外不是 0"：非遮罩区也要过一遍 VAE 编码→解码，那一步是有损的，
  所以判据是"远小于遮罩内"，不是"逐像素相同"。实测数字会打印出来。

  产物落在 ComfyUI 的 output 目录，**不写画廊**（这条脚本不碰库）。
  底图只读（拷贝一份进 input）。

.EXAMPLE
  pwsh -File scripts\e2e-inpaint-test.ps1
  pwsh -File scripts\e2e-inpaint-test.ps1 -Model krea2 -Region "0.1,0.1,0.4,0.8" -Prompt "a red mailbox"
#>
[CmdletBinding()]
param(
    # 底图文件；不给就取画廊里最新的一张图片产物
    [string]$BgImage = '',
    # 重绘区（归一化 x,y,w,h）
    [string]$Region = '0.58,0.26,0.36,0.74',
    # 羽化（px，靠"缩小再放大"实现）
    [int]$Feather = 16,
    # 用哪份自带工作流：anima（Danbooru tag 角色）或 krea2（摄影 / 写实）
    [ValidateSet('anima', 'krea2')]
    [string]$Model = 'anima',
    # 提示词；不给就按模型给一句（anima 走 tag，krea2 走自然语言）
    [string]$Prompt = '',
    [int]$TimeoutSeconds = 300,
    # 跑完把 input 目录里那两个临时文件删掉
    [switch]$CleanInput
)

$ErrorActionPreference = 'Stop'
$ProjectRoot = Split-Path -Parent $PSScriptRoot
# 数据目录可能被移动到其他磁盘（`<根>\data-dir.json`，与 lib\core\backend_paths.dart 同一约定）
$DataDir = $ProjectRoot
$DataDirFile = Join-Path $ProjectRoot 'data-dir.json'
if (Test-Path $DataDirFile) {
    try { $customDataDir = (Get-Content $DataDirFile -Raw -Encoding UTF8 | ConvertFrom-Json).dataDir } catch { $customDataDir = '' }
    if ($customDataDir) { $DataDir = $customDataDir }
}
. (Join-Path $PSScriptRoot 'comfy-path.ps1')

function Say([string]$Text, [string]$Color = 'Gray') { Write-Host $Text -ForegroundColor $Color }
function Check([bool]$Ok, [string]$What) {
    $script:Checks++
    if ($Ok) { Say ("  [OK]   " + $What) 'Green' } else { Say ("  [FAIL] " + $What) 'Red'; $script:Failed++ }
}
$script:Failed = 0
$script:Checks = 0   # ★ 跑了多少项（0 项 = 没通过）

$ComfyUrl = if ($env:COMFYHUB_COMFY_URL) { $env:COMFYHUB_COMFY_URL.TrimEnd('/') } else { 'http://127.0.0.1:8188' }
$comfyHome = Resolve-ComfyHome -ProjectRoot $ProjectRoot
if (-not $comfyHome) { throw "没找到 ComfyUI 目录（见 README「ComfyUI 位置」一节，或在设置页「自动捕获」卡点「使用这个目录」）" }
$InputDir = Join-Path $comfyHome 'input'
$detectedOutput = Resolve-ComfyOutputDir -ProjectRoot $ProjectRoot

# ⚠ input / output 目录**不能只按安装目录推**：ComfyUI Desktop 把"程序"与"共享数据
# （input / output / models）分在两处，跑着的实例读写的是**共享**那一份。本机实测：
# 安装目录 `…\ComfyUI-Installs\ComfyUI\ComfyUI\input` 有 15 个文件，而实例真正在用
# `…\ComfyUI-Shared\input`（50 个）—— 写错地方的表现是 `Invalid image file`
# （看着像"文件不存在"，其实是写进了另一个目录），产物也会被找错地方。
# 判据与后端 `AiWorkflowSearch.resolveComfyInputDir` 一致：**配置里生效的输出目录**
# （`GET /api/capture/config` 的 outputDir，那是 App 设置里的真源）优先，
# 推不出来才退回脚本探测到的那个。
# ⚠ 注意 `comfy-path.ps1` 的 `Resolve-ComfyOutputDir` 给的是**安装形态**的 output，
# 与实例真正在用的共享目录不是一个地方 —— 所以这里必须去问后端要配置值。
$api = Get-Content (Join-Path $ProjectRoot '.run\api.json') -Raw | ConvertFrom-Json
$configuredOutput = $null
try { $configuredOutput = (Invoke-RestMethod "$($api.url)/api/capture/config" -TimeoutSec 15).outputDir } catch { }
$OutputDir = if ($configuredOutput -and (Test-Path $configuredOutput)) {
    $configuredOutput
} elseif ($detectedOutput) {
    $detectedOutput
} else {
    Join-Path $comfyHome 'output'
}
$siblingInput = Join-Path (Split-Path $OutputDir -Parent) 'input'
if (Test-Path $siblingInput) { $InputDir = $siblingInput }
if (-not (Test-Path $InputDir)) { throw "ComfyUI 的 input 目录不存在：$InputDir" }
Say "实例的 input 目录：$InputDir" 'DarkGray'

Add-Type -AssemblyName System.Drawing

# ---------------------------------------------------------------------------
#  ① 底图
# ---------------------------------------------------------------------------
if (-not $BgImage) {
    $api = Get-Content (Join-Path $ProjectRoot '.run\api.json') -Raw | ConvertFrom-Json
    $media = Invoke-RestMethod "$($api.url)/api/media?size=20" -TimeoutSec 20
    # 挑一张真的底图：**跳过本脚本自己前几次跑出来的产物** ——
    # 捕获轮询会把它们收进画廊，不跳的话第二次跑就会"拿上一次的输出当底图"，
    # 越跑越小（实测：896x1152 → 320x848 → 112x624）。
    $pick = $media.items |
        Where-Object { $_.kind -eq 'IMAGE' -and -not $_.title.StartsWith('comfyhub_inpaint_e2e') } |
        Select-Object -First 1
    if (-not $pick) { throw "画廊里没有任何图片产物 —— 用 -BgImage 指定一张底图" }
    $BgImage = Join-Path $DataDir ("storage\media\" + $pick.storedName)
    Say "底图：画廊产物 #$($pick.id)「$($pick.title)」$($pick.width)x$($pick.height)" 'Cyan'
}
if (-not (Test-Path $BgImage)) { throw "底图不存在：$BgImage" }

$src = [System.Drawing.Bitmap]::FromFile((Resolve-Path $BgImage).Path)
# ⚠ 变量名一定要分清：PowerShell 的变量名**大小写不敏感**，`$W` 与 `$w` 是同一个变量 ——
# 早先这里用 $W/$H 当画布、$w/$h 当重绘区，结果画布被区域尺寸覆盖，
# 提交上去的 ImageScale 变成了"区域大小"（产物 323x852 而不是 896x1152）。
$canvasW = [int]([Math]::Floor($src.Width / 16) * 16)
$canvasH = [int]([Math]::Floor($src.Height / 16) * 16)
Say "画布：${canvasW}x${canvasH}（区域 $Region，羽化 $Feather）"

$bgName = 'comfyhub_e2e_bg.png'
$maskName = 'comfyhub_e2e_mask.png'
$bgPath = Join-Path $InputDir $bgName
$maskPath = Join-Path $InputDir $maskName

$canvas = New-Object System.Drawing.Bitmap($canvasW, $canvasH, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
$g = [System.Drawing.Graphics]::FromImage($canvas)
$g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
$g.DrawImage($src, 0, 0, $canvasW, $canvasH)
$g.Dispose()
$canvas.Save($bgPath, [System.Drawing.Imaging.ImageFormat]::Png)

# ---------------------------------------------------------------------------
#  ② 遮罩（白 = 重绘区；羽化 = 小尺寸画好再放大）
# ---------------------------------------------------------------------------
$parts = $Region.Split(',') | ForEach-Object { [double]$_.Trim() }
if ($parts.Count -ne 4) { throw "-Region 要四个归一化数字：x,y,w,h" }
$regionX = [int]($parts[0] * $canvasW); $regionY = [int]($parts[1] * $canvasH)
$regionW = [int]($parts[2] * $canvasW); $regionH = [int]($parts[3] * $canvasH)
$smallW = [Math]::Max(8, [int]($canvasW / 8)); $smallH = [Math]::Max(8, [int]($canvasH / 8))
$small = New-Object System.Drawing.Bitmap($smallW, $smallH, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
$sg = [System.Drawing.Graphics]::FromImage($small)
$sg.Clear([System.Drawing.Color]::Black)
$sg.FillRectangle([System.Drawing.Brushes]::White, [int]($regionX / 8), [int]($regionY / 8), [Math]::Max(1, [int]($regionW / 8)), [Math]::Max(1, [int]($regionH / 8)))
$sg.Dispose()
$mask = New-Object System.Drawing.Bitmap($canvasW, $canvasH, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
$mg = [System.Drawing.Graphics]::FromImage($mask)
$mg.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
$mg.DrawImage($small, 0, 0, $canvasW, $canvasH)
$mg.Dispose()
$mask.Save($maskPath, [System.Drawing.Imaging.ImageFormat]::Png)
$small.Dispose()
Say "遮罩：$maskName（白区 $regionX,$regionY,${regionW}x${regionH}）"

# ---------------------------------------------------------------------------
#  ③ 提交自带工作流
# ---------------------------------------------------------------------------
$workflowPath = Join-Path $ProjectRoot "comfyui\workflows\comfyhub_inpaint_$Model.json"
if (-not (Test-Path $workflowPath)) { throw "找不到工作流：$workflowPath" }
$graph = Get-Content $workflowPath -Raw | ConvertFrom-Json

function Set-Input([string]$NodeId, [string]$Key, $Value) {
    $node = $graph.PSObject.Properties[$NodeId].Value
    if (-not $node) { throw "工作流里没有节点 $NodeId" }
    $node.inputs.$Key = $Value
}

# anima 那份的提示词/采样器节点是 15/17，krea2 那份是 13/15
$textNode = if ($Model -eq 'krea2') { '13' } else { '15' }
$samplerNode = if ($Model -eq 'krea2') { '15' } else { '17' }
$prefix = "comfyhub_inpaint_e2e_$Model"

if (-not $Prompt) {
    $Prompt = if ($Model -eq 'krea2') {
        'A photograph of the same scene with a different person standing in that spot, matching the original lighting, color and perspective, sharp focus, 35mm film look.'
    } else {
        'masterpiece, best quality, newest, 1girl, solo, silver hair, blue eyes, white dress, standing, waving hand, looking at viewer'
    }
}

Set-Input '10' 'image' $bgName
Set-Input '20' 'image' $bgName     # 纯提示词重绘：人物图 = 底图自己（等于不做合成）
Set-Input '12' 'image' $maskName
Set-Input '11' 'width' $canvasW
Set-Input '11' 'height' $canvasH
Set-Input '21' 'width' $canvasW
Set-Input '21' 'height' $canvasH
Set-Input $textNode 'text' $Prompt
Set-Input $samplerNode 'seed' (Get-Random -Minimum 1 -Maximum 2000000000)
Set-Input $samplerNode 'denoise' 1.0
$graph.PSObject.Properties['19'].Value.inputs.filename_prefix = $prefix

$body = @{ prompt = $graph; client_id = 'comfyhub-e2e-inpaint' } | ConvertTo-Json -Depth 32 -Compress
Say "提交 $([System.IO.Path]::GetFileName($workflowPath)) → $ComfyUrl" 'Cyan'
$resp = Invoke-RestMethod -Method Post -Uri "$ComfyUrl/prompt" -ContentType 'application/json' -Body $body -TimeoutSec 60
$promptId = $resp.prompt_id
if (-not $promptId) { throw "提交失败：$($resp | ConvertTo-Json -Depth 6 -Compress)" }
if ($resp.node_errors -and ($resp.node_errors.PSObject.Properties | Measure-Object).Count -gt 0) {
    throw "ComfyUI 报了节点错误：$($resp.node_errors | ConvertTo-Json -Depth 8 -Compress)"
}
Say "prompt_id = $promptId，等它跑完（最多 $TimeoutSeconds 秒）…"

# ---------------------------------------------------------------------------
#  ④ 等 /history
# ---------------------------------------------------------------------------
$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
$images = $null
while ((Get-Date) -lt $deadline) {
    Start-Sleep -Seconds 2
    $hist = Invoke-RestMethod "$ComfyUrl/history/$promptId" -TimeoutSec 20
    $entry = $hist.PSObject.Properties[$promptId].Value
    if ($entry -and $entry.outputs) {
        foreach ($nodeId in $entry.outputs.PSObject.Properties.Name) {
            $outs = $entry.outputs.$nodeId
            if ($outs.images -and $outs.images.Count -gt 0) { $images = $outs.images; break }
        }
        if ($images) { break }
    }
    if ($entry -and $entry.status -and $entry.status.status_str -eq 'error') {
        throw "这次运行报错了：$($entry.status | ConvertTo-Json -Depth 6 -Compress)"
    }
}
if (-not $images) { throw "等不到产物（$TimeoutSeconds 秒）——去 ComfyUI 的控制台看这次运行" }

$file = $images[0].filename
$sub = $images[0].subfolder
$outPath = if ($sub) { Join-Path (Join-Path $OutputDir $sub) $file } else { Join-Path $OutputDir $file }
Say "产物：$outPath" 'Cyan'
Check (Test-Path $outPath) "产物文件真的落在 output 目录"

# ---------------------------------------------------------------------------
#  ⑤ 量化验收：遮罩内大变、遮罩外基本不变
# ---------------------------------------------------------------------------
$out = [System.Drawing.Bitmap]::FromFile($outPath)
Check ($out.Width -eq $canvasW -and $out.Height -eq $canvasH) `
    "产物尺寸就是画布尺寸（${canvasW}x${canvasH}，实测 $($out.Width)x$($out.Height)）"
# 比对以**两者中较小**的尺寸为准：尺寸不一致时也不该抛异常（那句 Check 已经红过了）
$cmpW = [Math]::Min($canvasW, $out.Width)
$cmpH = [Math]::Min($canvasH, $out.Height)
$stride = 3
$inSum = 0.0; $inN = 0
$outSum = 0.0; $outN = 0
for ($py = 0; $py -lt $cmpH; $py += $stride) {
    for ($px = 0; $px -lt $cmpW; $px += $stride) {
        $a = $canvas.GetPixel($px, $py)
        $b = $out.GetPixel($px, $py)
        $d = ([Math]::Abs($a.R - $b.R) + [Math]::Abs($a.G - $b.G) + [Math]::Abs($a.B - $b.B)) / 3.0
        # 羽化带（边界 ±Feather）既不算内也不算外
        $inner = ($px -ge $regionX + $Feather) -and ($px -lt $regionX + $regionW - $Feather) -and ($py -ge $regionY + $Feather) -and ($py -lt $regionY + $regionH - $Feather)
        $outer = ($px -lt $regionX - $Feather) -or ($px -ge $regionX + $regionW + $Feather) -or ($py -lt $regionY - $Feather) -or ($py -ge $regionY + $regionH + $Feather)
        if ($inner) { $inSum += $d; $inN++ } elseif ($outer) { $outSum += $d; $outN++ }
    }
}
$inAvg = if ($inN) { [Math]::Round($inSum / $inN, 2) } else { -1 }
$outAvg = if ($outN) { [Math]::Round($outSum / $outN, 2) } else { -1 }
$out.Dispose(); $canvas.Dispose(); $mask.Dispose(); $src.Dispose()
if ($CleanInput) { Remove-Item $bgPath, $maskPath -ErrorAction SilentlyContinue }

Say ""
Say "遮罩内平均像素差（采样步长 $stride）：$inAvg / 255" 'White'
Say "遮罩外平均像素差（采样步长 $stride）：$outAvg / 255" 'White'
Say "  （遮罩外不是 0 是对的：非遮罩区也要过一遍 VAE 编码→解码，那一步有损）"
Check ($inAvg -gt 12) "遮罩内**真的被重画了**（平均差 > 12）"
Check ($outAvg -ge 0 -and $outAvg -lt 8) "遮罩外**基本没动**（平均差 < 8）"
Check ($inAvg -gt $outAvg * 2) "内外差异拉开（遮罩内至少是遮罩外的 2 倍）"

Say ""
if ($script:Checks -le 0) { Say '  ✗ 一项检查都没跑（活实例没起？前置条件没过？）—— 这不叫通过。' 'Red'; exit 1 }
if ($script:Failed -gt 0) { Say "$($script:Failed) 项没通过" 'Red'; exit 1 }
Say "自带 inpaint 工作流在活实例上跑通，且没有动到遮罩外的画面。" 'Green'


