<#
.SYNOPSIS
    Anima 生图执行器：向本机 ComfyUI (127.0.0.1:8188) 提交一次 Anima 文生图，轮询到完成并返回落盘路径。

.DESCRIPTION
    封装 submit + poll + verify，避免每次手搓 graph。

    与 ComfyUI 用户工作流 "Anima with Lora.json" 对齐：
      UNETLoader -> [PathchSageAttentionKJ] -> LoraLoader(串联 N 个) -> KSampler -> VAEDecode -> SaveImage
      官方参数：16 步 / CFG 4.0 / er_sde / simple / CLIPLoader type=qwen_image / sage_attention=auto

    LoRA 串联：-Lora "名字[=权重]" 可重复；按给定顺序逐个 LoraLoader 串联（顺序有意义：
    先风格 LoRA 后修正/适配 LoRA，与官方工作流一致）。

.EXAMPLE
    # 官方 Anima with Lora 配方（风格 LoRA + 适配 LoRA，16 步，sage attention）
    pwsh -File scripts\anima-gen.ps1 -PromptFile .\p.txt -NegativeFile .\n.txt `
        -Lora "allmm style-step00003000.safetensors=1.0" -Lora "adapter_model.safetensors=1.0" `
        -ClipType qwen_image -SageAttention auto -Steps 16 -Prefix anima_test

.EXAMPLE
    # 单 LoRA，不挂 sage
    pwsh -File scripts\anima-gen.ps1 -PromptFile .\p.txt -Lora "anima-aesthetic-improvement-v1.1.safetensors=0.8"
#>
[CmdletBinding()]
param(
    [string]$Prompt,
    [string]$PromptFile,
    [string]$Negative,
    [string]$NegativeFile,
    [string]$Negative_ = "worst quality, low quality, artist name, blurry, jpeg artifacts, bad anatomy, bad hands, missing fingers, extra digits, fewer digits, fused fingers, watermark, signature, text, 3d, realistic, extra limbs, mirror, reflection, duplicate, lowres, nudity, multiple people, cloned face, spiral eyes, swirly eyes",
    [int]$Width = 768,
    [int]$Height = 1024,
    [int]$Steps = 30,
    [double]$Cfg = 4.0,
    [string]$Sampler = "er_sde",
    [string]$Scheduler = "simple",
    [int]$Seed = -1,
    [string]$Prefix = "anima_out",
    [string]$Model = "anima-base-v1.0.safetensors",
    [string]$ClipName = "qwen_3_06b_base.safetensors",
    [string]$ClipType = "qwen_image",
    [string]$VaeName = "qwen_image_vae.safetensors",
    # LoRA 串联：格式 "文件名[=权重]"，可用逗号/分号分隔多条，或直接传数组。
    # 注意：pwsh -File 调用时命令行上的逗号会被拆成多个 argv，务必用分号分隔。
    [string[]]$Lora = @(),
    [double]$LoraStrength = 0.8,
    # Sage Attention 补丁（需 comfyui-kjnodes + sageattention）；disabled = 不挂节点
    [string]$SageAttention = "disabled",
    [switch]$SageAllowCompile,
    [string]$Host_ = "127.0.0.1:8188",
    [int]$TimeoutSec = 900,
    # ComfyUI 的 output 目录（留空则自动探测，见 scripts\comfy-path.ps1）
    [string]$OutputDir = ""
)

$ErrorActionPreference = 'Stop'
$base = "http://$Host_"

# 0) probe
try {
    $null = Invoke-RestMethod -Uri "$base/system_stats" -TimeoutSec 5
} catch {
    throw "ComfyUI 未运行（$base/system_stats 不可达）。先启动 ComfyUI 再重试。"
}

if ($Seed -lt 0) { $Seed = Get-Random -Minimum 1 -Maximum 2147483646 }

if ($PromptFile)   { $Prompt   = Get-Content -LiteralPath $PromptFile   -Raw -Encoding utf8 }
if ($NegativeFile) { $Negative = Get-Content -LiteralPath $NegativeFile -Raw -Encoding utf8 }
if (-not $Prompt)   { throw "必须给 -Prompt 或 -PromptFile。" }
if (-not $Negative) { $Negative = $Negative_ }

$pos = ([string]$Prompt).Trim()
$neg = ([string]$Negative).Trim()

# 1) 解析 LoRA 链（"名字[=权重]"）
$loraChain = @()
$loraItems = foreach ($entry in $Lora) { if ($entry) { $entry -split '[;,]' } }
foreach ($item in $loraItems) {
    if ([string]::IsNullOrWhiteSpace($item)) { continue }
    $item = $item.Trim()
    $name = $item; $w = $LoraStrength
    $idx = $item.LastIndexOf('=')
    if ($idx -gt 0) {
        $name = $item.Substring(0, $idx).Trim()
        $w = [double]::Parse($item.Substring($idx + 1).Trim(), [Globalization.CultureInfo]::InvariantCulture)
    }
    if ($name -and $name -ne "none") { $loraChain += [pscustomobject]@{ Name = $name; Weight = $w } }
}

$graph = [ordered]@{
    "1" = [ordered]@{ class_type = "UNETLoader";     inputs = [ordered]@{ unet_name = $Model; weight_dtype = "default" } }
    "2" = [ordered]@{ class_type = "CLIPLoader";     inputs = [ordered]@{ clip_name = $ClipName; type = $ClipType } }
    "3" = [ordered]@{ class_type = "VAELoader";      inputs = [ordered]@{ vae_name = $VaeName } }
    "6" = [ordered]@{ class_type = "EmptyLatentImage"; inputs = [ordered]@{ width = $Width; height = $Height; batch_size = 1 } }
    "9" = [ordered]@{ class_type = "VAEDecode";      inputs = [ordered]@{ samples = @("8", 0); vae = @("3", 0) } }
    "10" = [ordered]@{ class_type = "SaveImage";     inputs = [ordered]@{ images = @("9", 0); filename_prefix = $Prefix } }
}

# 模型链：UNETLoader -> [sage patch] -> LoraLoader xN
$modelRef = @("1", 0)
$clipRef  = @("2", 0)
$nextId = 20

if ($SageAttention -and $SageAttention -ne "disabled") {
    $sid = "$nextId"; $nextId++
    $graph[$sid] = [ordered]@{
        class_type = "PathchSageAttentionKJ"
        inputs = [ordered]@{ model = $modelRef; sage_attention = $SageAttention; allow_compile = [bool]$SageAllowCompile }
    }
    $modelRef = @($sid, 0)
}

foreach ($l in $loraChain) {
    $lid = "$nextId"; $nextId++
    $graph[$lid] = [ordered]@{
        class_type = "LoraLoader"
        inputs = [ordered]@{
            model = $modelRef; clip = $clipRef; lora_name = $l.Name
            strength_model = $l.Weight; strength_clip = $l.Weight
        }
    }
    $modelRef = @($lid, 0); $clipRef = @($lid, 1)
}

$graph["4"] = [ordered]@{ class_type = "CLIPTextEncode"; inputs = [ordered]@{ text = $pos; clip = $clipRef } }
$graph["5"] = [ordered]@{ class_type = "CLIPTextEncode"; inputs = [ordered]@{ text = $neg; clip = $clipRef } }
$graph["8"] = [ordered]@{
    class_type = "KSampler"
    inputs = [ordered]@{
        model = $modelRef; seed = $Seed; steps = $Steps; cfg = $Cfg
        sampler_name = $Sampler; scheduler = $Scheduler
        positive = @("4", 0); negative = @("5", 0); latent_image = @("6", 0); denoise = 1.0
    }
}

$payload = @{ prompt = $graph; client_id = "anima" } | ConvertTo-Json -Depth 24 -Compress

# UTF-8 字节提交（PS7 默认已是 UTF-8，显式编码防中文乱码）
$bytes = [System.Text.Encoding]::UTF8.GetBytes($payload)
$resp = Invoke-RestMethod -Uri "$base/prompt" -Method Post -Body $bytes -ContentType "application/json; charset=utf-8" -TimeoutSec 60

if ($resp.node_errors -and $resp.node_errors.PSObject.Properties.Count -gt 0) {
    throw "节点校验失败：$($resp.node_errors | ConvertTo-Json -Depth 10)"
}
$pid_ = $resp.prompt_id
$loraDesc = if ($loraChain.Count -gt 0) { ($loraChain | ForEach-Object { "$($_.Name)@$($_.Weight)" }) -join ' -> ' } else { 'none' }
Write-Host "[anima] submitted prompt_id=$pid_ seed=$Seed ${Width}x${Height} steps=$Steps cfg=$Cfg clip=$ClipType sage=$SageAttention"
Write-Host "[anima] loras: $loraDesc"

$sw = [Diagnostics.Stopwatch]::StartNew()
$files = $null
while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
    Start-Sleep -Milliseconds 800
    try { $h = Invoke-RestMethod -Uri "$base/history/$pid_" -TimeoutSec 20 } catch { continue }
    $entry = $h.$pid_
    if (-not $entry) { continue }
    if ($entry.status.status_str -eq "error") {
        throw "生成失败：$($entry.status | ConvertTo-Json -Depth 8)"
    }
    if ($entry.outputs) {
        $files = @()
        foreach ($nid in $entry.outputs.PSObject.Properties.Name) {
            foreach ($img in $entry.outputs.$nid.images) { $files += $img }
        }
        if ($files.Count -gt 0) { break }
    }
}
if (-not $files) { throw "超时（$TimeoutSec s）未拿到输出。" }

# 定位落盘文件（output 根或日期子目录）；subfolder 可能为空串，不能直接 Join-Path
# 输出目录**不再写死**：优先级 = -OutputDir 参数 → COMFYHUB_COMFY_OUTPUT → 探测到的 ComfyUI 的 output/
$outRoot = $OutputDir
if ([string]::IsNullOrWhiteSpace($outRoot)) {
    $helper = Join-Path $PSScriptRoot 'comfy-path.ps1'
    if (Test-Path $helper) {
        . $helper
        $outRoot = Resolve-ComfyOutputDir
    }
}
if ([string]::IsNullOrWhiteSpace($outRoot)) {
    throw "定位不到 ComfyUI 的输出目录。请用 -OutputDir 指定，或设置 COMFYHUB_COMFY_OUTPUT。"
}
foreach ($f in $files) {
    $rel = if ([string]::IsNullOrWhiteSpace($f.subfolder)) { $f.filename } else { Join-Path $f.subfolder $f.filename }
    $p = Join-Path $outRoot $rel
    if (-not (Test-Path $p)) {
        $hit = Get-ChildItem $outRoot -Recurse -Filter $f.filename -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($hit) { $p = $hit.FullName }
    }
    $size = if (Test-Path $p) { (Get-Item $p).Length } else { 0 }
    Write-Host ("[anima] DONE {0}  ({1:N2} MB)  {2:N1}s" -f $p, ($size / 1MB), $sw.Elapsed.TotalSeconds)
    Write-Output $p
}
