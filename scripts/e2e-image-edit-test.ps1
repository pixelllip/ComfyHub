<#
.SYNOPSIS
  ComfyHub「AI 图像编辑」端到端自测：**真的让 AI 调三件工具**把一张参考照片改掉，
  并用像素差证明"该换的换了、该保的一个像素都没动"。

.DESCRIPTION
  这是"保背景换人物 / 换背景保人物"那条路的**活实例**验收（真 ComfyUI + 真后端 + 真图）。
  与 `e2e-inpaint-test.ps1` 的分工：那一份验的是**工作流本身**在活实例上跑得对不对
  （它自己造遮罩、自己 POST /prompt，绕开我们的工具）；这一份验的是**AI 那条链路**：
  模型能不能只凭工具描述就正确地"投放 → 遮罩 → 提交"，以及工具写进去的东西是不是
  实例真正在读的那份目录。

  链路（假网关逐轮发出工具调用，等价于模型的行为）：
    ① `comfy_use_attachment` 把参考照片投进 ComfyUI 的 input 目录；
    ② `comfy_make_mask` 照**刚投进去那张图的尺寸**生成"白=重绘区"的遮罩；
    ③ `comfy_submit` 提交项目自带的 inpaint 工作流，覆盖底图 / 遮罩 / ImageScale 尺寸 /
       提示词 / seed / denoise —— 三轮都要用户在审批卡上点"批准"（出厂档是 ask）。

  量化验收（与 e2e-inpaint 同一套判据）：
    · 遮罩内平均像素差要**大**（真的重画了）；
    · 遮罩外要**小**（背景保住了 —— 不是 0 是对的：非遮罩区也要过一遍 VAE，那一步有损）。

  另外两件只有端到端才抓得住的事：
    · **投放的目录就是实例在读的那份**：ComfyUI Desktop 把程序与共享数据分在两处，
      写错地方的表现是 `Invalid image file`（看着像"文件不存在"，其实是写进了另一个目录）；
      所以断言里直接比对 `inputDir` 与实例配置的 `input`，并逐字节比对投放上去的文件。
    · 投放的文件名要能**直接喂给下一件工具**（`comfy_make_mask(baseFilename=…)`）——
      真实模型就是照工具返回的 filename 写的参数。

  前置条件：`pwsh -File scripts\comfyhub.ps1 up`（后端）+ **ComfyUI 实例在跑**（8188）
  + 本机有 python。参考照片默认取 `ref_photos\ref_bg_1.jpg`（3072x4096，脚本会缩到长边 1024
  并向下取 16 的倍数 —— 8GB 显存上直接跑 3072x4096 会 OOM）。

.EXAMPLE
  pwsh -File scripts\e2e-image-edit-test.ps1
  pwsh -File scripts\e2e-image-edit-test.ps1 -RefPhoto ref_photos\ref_bg_1.jpg -Model krea2 -Region "0.1,0.2,0.4,0.7"
#>
[CmdletBinding()]
param(
    # 后端地址；不给就按后端记在案的端口算（`.run\api.json`）
    [string]$ApiBase = '',
    # 参考照片（默认 <项目根>\ref_photos\ref_bg_1.jpg）
    [string]$RefPhoto = '',
    # 缩放到长边多少像素（再向下取 16 的倍数；8GB 显存上 1024 是稳妥值）
    [int]$LongEdge = 1024,
    # 重绘区（归一化 x,y,w,h）
    [string]$Region = '0.58,0.26,0.36,0.74',
    # 羽化（px）
    [int]$Feather = 16,
    # 用哪份自带工作流：anima（Danbooru tag 角色）或 krea2（摄影 / 写实）
    [ValidateSet('anima', 'krea2')]
    [string]$Model = 'anima',
    # 提示词；不给就按模型给一句
    [string]$Prompt = '',
    # 先投底图、再按遮罩重绘时用多大 denoise（1.0 = 遮罩区整块重画）
    [double]$Denoise = 1.0,
    # **两阶段编辑的第二阶段**：阶段一生成的人物图（画廊里的产物 id）。
    # 给了就多一轮 `comfy_use_media` 把人物图投进 ComfyUI，并把 `20.image` 指向它、
    # 用 -PersonDenoise（默认 0.5）融合 —— 这就是用户原话里
    # "保持背景不变，更换有具体动作的人物并保持画风"的那条路。
    [int]$PersonMediaId = 0,
    # **两阶段编辑的第一阶段**：先生成人物（t2i），再进第二阶段把它贴进底图。
    # 用户原话："这时候就应该先生成人物，然后再用图像编辑把人换上去吧？"
    [switch]$TwoStage,
    # 阶段一用哪份 t2i 工作流（默认项目自带的 Krea2 turbo t2i，白底出图好贴）
    [string]$PersonWorkflow = '',
    # 第二阶段用哪份 inpaint 工作流（默认按 -Model 选；贴人物图推荐
    # `comfyhub_paste_person_anima.json` —— 它有两个 LoadImageMask，能"只融接缝、不重画人物"）
    [string]$Workflow = '',
    # **只在人物外面一圈重绘**（像素）。>0 时必须用有两遮罩的那份工作流；
    # 这一档人物像素与画风一个都不动，模型只融接缝 —— "嵌进去会改变画风"的正解。
    [int]$Ring = 0,
    # 摆放：人物剪影缩放多少倍（1 = 不缩放）
    [double]$PlaceScale = 1.0,
    # 摆放：人物剪影中心放到归一化坐标（0~1）；不给就保持原位
    [double]$PlaceX = -1,
    [double]$PlaceY = -1,
    # 阶段一的**背景色**（`green` / `magenta` / `#00b140`）—— 抠图最稳的一档：
    # 背景色与主体不同，判据干净（白底 + 白裙子在像素上无法区分，见 skill 里的实测）
    [string]$BgColor = '',
    # 阶段一提示词里要写的背景描述（默认按 -BgColor 推）
    [string]$PersonBackground = '',
    # **按背景环境改新图层的光照/阴影方向**（`auto` = 从底图估；也可 `left`/`right`/`top`/`bottom`/`none`）
    [string]$Light = 'none',
    # **AI 软遮罩那一档**（2026-09-27）：贴人物图的首选路 —— 遮罩由工作流里的
    # `easy imageRemBg` 现算（连续 alpha），Rust 侧只把人物图那份像素洗干净。
    # 它**需要人物图**（没有 subjectFilename 就没得抠），所以必须走两阶段 / 给 -PersonMediaId。
    [switch]$Matte,
    # 阶段一的提示词（要人物 + 干净背景）
    [string]$PersonPrompt = '',
    # 贴人物图时的融合档（0.4~0.6；高了人物会变样，低了接缝明显）
    [double]$PersonDenoise = 0.5,
    # **cfg**（0 = 不动，用工作流自己写死的值）。
    # ⚠ 两条自带工作流都写死 `cfg=1.0`（无引导档，Turbo 的常规跑法）——
    #   此时提示词对画面的约束很弱，"贴进来的背景"很难被重画掉。要提高就得显式给。
    [double]$Cfg = 0,
    [int]$GatewayPort = 8797,
    [int]$TimeoutSeconds = 900,
    # 跑完把测试产生的会话 / 产物 / 提示词 / input 里的临时文件删掉
    [switch]$KeepData
)

$ErrorActionPreference = 'Continue'
$ProjectRoot = Split-Path -Parent $PSScriptRoot
if (-not $ApiBase) {
    . (Join-Path $PSScriptRoot 'api-port.ps1')
    $portState = Read-ApiPortState -ProjectRoot $ProjectRoot
    $port = if ($portState -and $portState.Port -gt 0) { $portState.Port } else { 8080 }
    $ApiBase = "http://127.0.0.1:$port"
}
$E2eDir = Join-Path $PSScriptRoot 'e2e'
$WorkDir = Join-Path $ProjectRoot '.run\e2e-image-edit'
$ProviderId = 'e2e-image-edit-gw'
$ComfyUrl = if ($env:COMFYHUB_COMFY_URL) { $env:COMFYHUB_COMFY_URL.TrimEnd('/') } else { 'http://127.0.0.1:8188' }

$script:Failed = 0
$script:Checks = 0
$script:Gateway = $null
$script:OldConfig = $null
$script:PermissionBefore = $null
$script:CreatedMediaIds = @()
$script:CreatedPromptIds = @()
$script:CreatedConversationIds = @()
$script:AttachmentId = $null
$script:InputDir = ''
$script:PlacedFile = ''

function Say([string]$msg, [string]$color = 'Gray') { Write-Host $msg -ForegroundColor $color }

function Check([string]$name, $ok, [string]$detail = '') {
    # `$ok` 故意不强类型：`-match` / `-contains` 会返回数组，强类型参数当场抛绑定错误，
    # 而那个错误会让后面所有断言**静默跳过**、脚本还报"全部通过"（踩过）。
    $passed = $false
    if ($ok -is [array]) { $passed = $ok.Count -gt 0 } else { $passed = [bool]$ok }
    $script:Checks++
    if ($passed) {
        Say ("  [OK]   " + $name) 'Green'
    } else {
        Say ("  [FAIL] " + $name + $(if ($detail) { "  -> $detail" } else { '' })) 'Red'
        $script:Failed++
    }
}

function Invoke-Api {
    param([string]$Method, [string]$Path, $Body)
    $uri = "$ApiBase$Path"
    if ($null -eq $Body) { return Invoke-RestMethod -Method $Method -Uri $uri -TimeoutSec 120 }
    return Invoke-RestMethod -Method $Method -Uri $uri -TimeoutSec 300 `
        -ContentType 'application/json; charset=utf-8' -Body ($Body | ConvertTo-Json -Depth 12 -Compress)
}

function Find-Python {
    $cands = @($env:COMFYHUB_PYTHON)
    $cmd = Get-Command python -ErrorAction SilentlyContinue
    if ($cmd) { $cands += $cmd.Source }
    $cands += @(
        'D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\standalone-env\python.exe',
        'D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI\.venv\Scripts\python.exe'
    )
    foreach ($c in $cands) { if ($c -and (Test-Path $c)) { return $c } }
    return $null
}

# ComfyUI Desktop 的实例在用的 input / output 目录（settings.json 是那份配置的真源）。
# ⚠ 不能只按安装目录推：Desktop 把"程序"与"共享数据（input / output / models）"分在两处。
function Get-ComfyDesktopDirs {
    $settings = Join-Path $env:APPDATA 'Comfy Desktop\settings.json'
    if (-not (Test-Path $settings)) { return $null }
    try {
        $json = Get-Content $settings -Raw | ConvertFrom-Json
        if ($json.inputDir -and $json.outputDir) {
            return @{ Input = $json.inputDir; Output = $json.outputDir }
        }
    } catch { }
    return $null
}

# 等一次"待批准"的调用出现并批准它（R8：审批闸门是真的，没人点就不执行）。
function Approve-PendingCalls([string]$RunId, [int]$TimeoutSeconds = 120) {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    $approved = 0
    while ((Get-Date) -lt $deadline) {
        $pending = $null
        try { $pending = Invoke-Api 'GET' '/api/ai/pending' $null } catch { }
        $mine = @()
        if ($pending -and $pending.approvals) {
            $mine = @($pending.approvals | Where-Object { $_.runId -eq $RunId })
        }
        if ($mine.Count -gt 0) {
            foreach ($call in $mine) {
                try {
                    $r = Invoke-Api 'POST' "/api/ai/tool-calls/$($call.callId)/approve" @{}
                    if ($r.accepted -eq $true) { $approved++; Say ("  已批准 " + $call.name) 'DarkGray' }
                } catch { Say ("  批准失败: " + $_.Exception.Message) 'Yellow' }
            }
            return $approved
        }
        Start-Sleep -Milliseconds 300
    }
    return $approved
}

Say ''
Say '  ComfyHub「AI 图像编辑」· 活实例端到端自测' 'White'
Say '  ────────────────────────────────────────────────────────────'

$python = Find-Python
if (-not $python) { Say '  找不到 python（可用环境变量 COMFYHUB_PYTHON 指定）。' 'Red'; exit 1 }

try {
    # --- 0. 前置条件 ------------------------------------------------------
    $health = Invoke-Api 'GET' '/api/health' $null
    Check '后端可用（db=ok）' ($health.database -eq 'ok')
    if ($script:Failed -gt 0) { exit 1 }

    # 工具必须在**这台后端**上真的实现了 —— 顺带证明二进制是新鲜的
    $tools = @(Invoke-Api 'GET' '/api/ai/tools' $null)
    $toolNames = @($tools | ForEach-Object { $_.name })
    foreach ($name in @('comfy_use_attachment', 'comfy_make_mask', 'comfy_submit')) {
        Check "后端实现了 $name" ($toolNames -contains $name) "当前后端没有这个工具（二进制可能是旧的）"
    }

    $stats = $null
    try { $stats = Invoke-RestMethod "$ComfyUrl/system_stats" -TimeoutSec 10 } catch { }
    if (-not $stats) {
        Say "  ComfyUI 没在跑（$ComfyUrl）—— 这一幕需要活实例：先起 ComfyUI 再来。" 'Yellow'
        exit 1
    }
    Say ("  ComfyUI: " + (($stats.devices | Select-Object -First 1).name) + "  显存空闲 " +
        [math]::Round((($stats.devices | Select-Object -First 1).vram_free / 1GB), 1) + "GB") 'DarkGray'

    $workflowPath = Join-Path $ProjectRoot "comfyui\workflows\comfyhub_inpaint_$Model.json"
    if ($Matte) {
        # **AI 软遮罩那一档**（2026-09-27）：贴人物图的首选路。
        # 遮罩由工作流里的 `easy imageRemBg` 现算，所以这份工作流**没有** 12 / 26
        # 两个 LoadImageMask —— 一个遮罩覆盖都不该写（这一档要验的正是这件事）。
        $workflowPath = Join-Path $ProjectRoot 'comfyui\workflows\comfyhub_paste_person_matte_anima.json'
    }
    Check "自带工作流在（$(if ($Matte) { 'matte' } else { $Model })）" (Test-Path $workflowPath) $workflowPath
    if (-not (Test-Path $workflowPath)) { exit 1 }
    if ($Matte -and -not $TwoStage -and $PersonMediaId -le 0) {
        # matte 档要抠的就是**人物图** —— 没有它就没得抠（`comfy_make_mask` 会如实报缺 subjectFilename）
        Say '  -Matte 要有人物图：加 -TwoStage（先生成人物）或 -PersonMediaId <画廊产物 id>' 'Red'
        exit 1
    }

    if (-not $RefPhoto) {
        # 默认用仓库里那份参考照片；没有就退回"画廊里最新的一张图片产物"
        # （不去造一张假的：遮罩内的判据是"被重画了没有"，底图必须有真实内容）
        $candidate = Join-Path $ProjectRoot 'ref_photos\ref_bg_1.jpg'
        if (Test-Path $candidate) {
            $RefPhoto = $candidate
        } else {
            $media = Invoke-Api 'GET' '/api/media?size=20' $null
            $pick = @($media.items) | Where-Object { $_.kind -eq 'IMAGE' } | Select-Object -First 1
            if (-not $pick) { Say '  没有参考照片，也找不到可用的画廊产物 —— 用 -RefPhoto 指定一张。' 'Red'; exit 1 }
            $RefPhoto = Join-Path (Join-Path $ProjectRoot 'storage\media') $pick.storedName
            Say "  底图：画廊产物 #$($pick.id)「$($pick.title)」" 'DarkGray'
        }
    }
    if (-not (Test-Path $RefPhoto)) { Say "  参考照片不存在：$RefPhoto" 'Red'; exit 1 }
    if (-not $Prompt) {
        $Prompt = if ($Model -eq 'krea2') {
            'A photograph of the same scene with a different person standing in that spot, matching the original lighting, color and perspective, sharp focus, 35mm film look.'
        } else {
            'masterpiece, best quality, newest, 1girl, solo, silver hair, blue eyes, white dress, standing, waving hand, looking at viewer'
        }
    }

    # --- 1. 把参考照片缩成画布尺寸的底图 ----------------------------------
    # 尺寸必须是 16 的倍数（VAE 下采样 8、采样又按 2 分组），否则潜空间会补边、
    # 遮罩与底图就对不齐（Kotlin 那次实测踩过：323x852 的产物就是这么来的）。
    New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null
    Add-Type -AssemblyName System.Drawing
    $srcImage = [System.Drawing.Image]::FromFile((Resolve-Path $RefPhoto).Path)
    $scale = [Math]::Min(1.0, $LongEdge / [Math]::Max($srcImage.Width, $srcImage.Height))
    $canvasW = [int]([Math]::Floor(($srcImage.Width * $scale) / 16) * 16)
    $canvasH = [int]([Math]::Floor(($srcImage.Height * $scale) / 16) * 16)
    $bgPath = Join-Path $WorkDir 'edit-bg.png'
    $canvas = New-Object System.Drawing.Bitmap($canvasW, $canvasH, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
    $g = [System.Drawing.Graphics]::FromImage($canvas)
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.DrawImage($srcImage, 0, 0, $canvasW, $canvasH)
    $g.Dispose()
    $canvas.Save($bgPath, [System.Drawing.Imaging.ImageFormat]::Png)
    $srcImage.Dispose()
    Say "  参考照片 $((Get-Item $RefPhoto).Name) → 画布 ${canvasW}x${canvasH}（$([math]::Round($scale * 100))%）" 'Cyan'
    Check '画布尺寸是 16 的倍数' (($canvasW % 16) -eq 0 -and ($canvasH % 16) -eq 0) "${canvasW}x${canvasH}"

    # --- 2. 实例真正在用的 input / output 目录 -----------------------------
    $desktop = Get-ComfyDesktopDirs
    if (-not $desktop) { Say '  读不到 ComfyUI Desktop 的 settings.json —— 需要它来确认实例在用的目录' 'Red'; exit 1 }
    $instanceInput = $desktop.Input
    $instanceOutput = $desktop.Output
    $script:InputDir = $instanceInput
    Say "  实例的 input 目录：$instanceInput" 'DarkGray'

    # --- 3. 把底图作为**附件**上传（用户随消息发来的图就是这条路）---------
    $upload = Invoke-RestMethod -Method Post -Uri "$ApiBase/api/ai/attachments" -Form @{ files = Get-Item $bgPath } -TimeoutSec 120
    $attachment = @($upload.items) | Select-Object -First 1
    Check '参考照片作为附件上传成功' ($null -ne $attachment -and $attachment.id) ($upload | ConvertTo-Json -Compress)
    if (-not $attachment) { exit 1 }
    $script:AttachmentId = $attachment.id
    Say "  附件 id = $($script:AttachmentId)（$($attachment.kind)）" 'DarkGray'

    # --- 4. 捕获配置指向实例的输出目录 + 权限档钉成 ask --------------------
    # ⚠ 这两步都是**临时**的，finally 里原样放回去。
    #   捕获配置的输出目录就是"实例在用的那份"的真源（我们的工具也从它推 input）——
    #   写错地方的表现是 `Invalid image file`。
    $script:OldConfig = Invoke-Api 'GET' '/api/capture/config' $null
    Invoke-Api 'PUT' '/api/capture/config' @{
        enabled = $false
        comfyUrl = $ComfyUrl
        outputDir = $instanceOutput
        pollSeconds = 4
        autoTag = 'ComfyUI'
        maxPerPoll = 20
        downloadFallback = $true
    } | Out-Null
    $script:PermissionBefore = (Invoke-Api 'GET' '/api/ai/tools/policy' $null).permissionMode
    if ($script:PermissionBefore -ne 'ask') {
        Invoke-Api 'PUT' '/api/ai/tools/policy' @{ permissionMode = 'ask' } | Out-Null
        Say "  权限档临时从 '$($script:PermissionBefore)' 切成 'ask'（结束时还原）" 'DarkGray'
    }

    # --- 5. 假网关 + 临时 Provider ----------------------------------------
    $script:Gateway = Start-Process -FilePath $python -PassThru -WindowStyle Hidden `
        -ArgumentList @((Join-Path $E2eDir 'fake_title_gateway.py'), '--port', "$GatewayPort")
    $ready = $false
    foreach ($i in 1..40) {
        try { $null = Invoke-RestMethod "http://127.0.0.1:$GatewayPort/v1/models" -TimeoutSec 2; $ready = $true; break }
        catch { Start-Sleep -Milliseconds 300 }
    }
    Check '假网关就绪（loopback，不出网）' $ready
    if (-not $ready) { exit 1 }

    try { Invoke-Api 'DELETE' "/api/ai/providers/$ProviderId" $null | Out-Null } catch { }
    Invoke-Api 'POST' '/api/ai/providers' @{
        id = $ProviderId; displayName = 'E2E 图像编辑网关'; api = 'openai-completions'
        baseURL = "http://127.0.0.1:$GatewayPort/v1"; credentialRef = 'E2E_IMAGE_EDIT_KEY'
        endpointTrust = 'loopback'
    } | Out-Null
    Invoke-Api 'PUT' "/api/ai/providers/$ProviderId/credentials" @{ value = 'sk-fake-not-a-real-key' } | Out-Null
    Invoke-Api 'PUT' "/api/ai/providers/$ProviderId/models" @{
        models = @(@{ providerId = $ProviderId; id = 'fake-edit-model'; displayName = 'E2E 假模型'
                      inputModalities = @('text'); tools = $true; reasoning = $false
                      capabilitySource = 'manual'; enabled = $true })
    } | Out-Null

    # --- 6. 两阶段：先生成人物（t2i）--------------------------------------
    if ($TwoStage -and $PersonMediaId -le 0) {
        if (-not $PersonWorkflow) { $PersonWorkflow = Join-Path $ProjectRoot 'comfyui\krea2_sage_turbo_t2i.json' }
        if (-not (Test-Path $PersonWorkflow)) { Say "  阶段一的工作流不存在：$PersonWorkflow" 'Red'; exit 1 }
        if (-not $PersonPrompt) {
            $PersonPrompt = 'A photograph of a young woman with long black hair, wearing a red dress, ' +
                'standing straight and waving her hand, full body, soft studio lighting, ' +
                'clean flat white background, sharp focus, 35mm film look.'
        }
        Say ''
        Say '  [阶段一] 先生成人物（t2i → 产物入库 → 拿到 mediaId）' 'Cyan'
        $convA = Invoke-Api 'POST' '/api/ai/conversations' @{ providerId = $ProviderId; modelId = 'fake-edit-model' }
        $script:CreatedConversationIds += $convA.id
        $runA = Invoke-Api 'POST' "/api/ai/conversations/$($convA.id)/runs" @{
            text = "生成人物 $PersonWorkflow|$PersonPrompt"; providerId = $ProviderId; modelId = 'fake-edit-model'
        }
        $assistantA = $null
        $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
        while ((Get-Date) -lt $deadline) {
            $null = Approve-PendingCalls $runA.runId -TimeoutSeconds 3
            $msgsA = Invoke-Api 'GET' "/api/ai/conversations/$($convA.id)/messages" $null
            $assistantA = @($msgsA | Where-Object { $_.id -eq $runA.assistantMessageId }) | Select-Object -First 1
            if ($assistantA -and ($assistantA.status -eq 'complete' -or $assistantA.status -eq 'failed')) { break }
            Start-Sleep -Milliseconds 700
        }
        Check '阶段一 Run 结束（complete）' ($assistantA -and $assistantA.status -eq 'complete') $assistantA.status
        $submitA = @($assistantA.parts | Where-Object { $_.type -eq 'tool_result' -and $_.jsonPayload.name -eq 'comfy_submit' }) |
            Select-Object -First 1
        $stage1 = if ($submitA) { $submitA.text | ConvertFrom-Json } else { $null }
        Check '阶段一产物入库（mediaIds 非空）' ($stage1 -and @($stage1.mediaIds).Count -ge 1) ($stage1 | ConvertTo-Json -Compress)
        if ($stage1 -and $stage1.mediaIds) {
            $PersonMediaId = [int]@($stage1.mediaIds)[0]
            $script:CreatedMediaIds += @($stage1.mediaIds)
            if ($stage1.capturedPromptId) { $script:CreatedPromptIds += $stage1.capturedPromptId }
            Say "  阶段一产物：mediaId=$PersonMediaId" 'DarkGray'
        } else {
            Say '  阶段一没拿到产物 —— 第二阶段没法继续' 'Red'
        }
    }

    # --- 7. 发一次「编辑图片」，三轮工具调用都要批准 -----------------------
    Say ''
    Say '  [1/3] AI 投放底图 → 生成遮罩 → 提交（每步都要用户批准）' 'Cyan'
    $conv = Invoke-Api 'POST' '/api/ai/conversations' @{ providerId = $ProviderId; modelId = 'fake-edit-model' }
    $script:CreatedConversationIds += $conv.id
    # 两阶段模式下 denoise 用**融合档**（贴人物图），单阶段用 -Denoise（默认 1.0 纯重绘）
    $effectiveDenoise = if ($PersonMediaId -gt 0) { $PersonDenoise } else { $Denoise }
    $workflowField = if ($Workflow) { $Workflow } else { $workflowPath }
    $cfgField = if ($Cfg -gt 0) { "$Cfg" } else { '' }
    $placeXField = if ($PlaceX -ge 0) { "$PlaceX" } else { '' }
    $placeYField = if ($PlaceY -ge 0) { "$PlaceY" } else { '' }
    $matteField = if ($Matte) { '1' } else { '' }
    $runText = "编辑图片 $($script:AttachmentId)|$workflowField|$effectiveDenoise|$Region|$Feather|$Prompt|$PersonMediaId|$cfgField|$Ring|$PlaceScale|$placeXField|$placeYField|$BgColor|$Light|$matteField"
    $run = Invoke-Api 'POST' "/api/ai/conversations/$($conv.id)/runs" @{
        text = $runText; providerId = $ProviderId; modelId = 'fake-edit-model'
    }

    $assistant = $null
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    $approvals = 0
    while ((Get-Date) -lt $deadline) {
        $approvals += Approve-PendingCalls $run.runId -TimeoutSeconds 3
        $msgs = Invoke-Api 'GET' "/api/ai/conversations/$($conv.id)/messages" $null
        $assistant = @($msgs | Where-Object { $_.id -eq $run.assistantMessageId }) | Select-Object -First 1
        if ($assistant -and ($assistant.status -eq 'complete' -or $assistant.status -eq 'failed')) { break }
        Start-Sleep -Milliseconds 700
    }
    Check 'Run 结束（complete）' ($assistant -and $assistant.status -eq 'complete') $assistant.status
    $expectedTools = if ($PersonMediaId -gt 0) {
        'comfy_use_attachment,comfy_use_media,comfy_make_mask,comfy_submit'
    } else {
        'comfy_use_attachment,comfy_make_mask,comfy_submit'
    }
    $expectedCount = if ($PersonMediaId -gt 0) { 4 } else { 3 }
    Check "工具调用都经过审批闸门（$expectedCount 轮）" ($approvals -eq $expectedCount) "批准了 $approvals 次"

    # --- 7. 三个工具的结果 ------------------------------------------------
    Say ''
    Say '  [2/3] 工具结果与落盘的证据' 'Cyan'
    $results = @($assistant.parts | Where-Object { $_.type -eq 'tool_result' })
    $names = @($results | ForEach-Object { $_.jsonPayload.name })
    Check '工具调用顺序 = 投放 →（人物图）→ 遮罩 → 提交' `
        (($names -join ',') -eq $expectedTools) ($names -join ',')

    $byName = @{}
    foreach ($r in $results) { $byName[$r.jsonPayload.name] = $r }
    $placed = $null; $mask = $null; $submitted = $null
    if ($byName.ContainsKey('comfy_use_attachment')) { $placed = $byName['comfy_use_attachment'].text | ConvertFrom-Json }
    if ($byName.ContainsKey('comfy_make_mask')) { $mask = $byName['comfy_make_mask'].text | ConvertFrom-Json }
    if ($byName.ContainsKey('comfy_submit')) { $submitted = $byName['comfy_submit'].text | ConvertFrom-Json }

    Check '投放结果里给了 filename' ($placed -and $placed.filename) ($placed | ConvertTo-Json -Compress)
    # ★ 这条是"写对了目录"的硬证据：实例真正在读的就是这一份
    Check '投放写进了实例在用的 input 目录' `
        ($placed -and $placed.inputDir -and ($placed.inputDir.TrimEnd('\') -ieq $instanceInput.TrimEnd('\'))) `
        "工具说 $($placed.inputDir)，实例在用 $instanceInput"
    $script:PlacedFile = if ($placed) { $placed.filename } else { '' }
    $placedPath = Join-Path $instanceInput $script:PlacedFile
    Check '投放的文件真的在盘上' (Test-Path $placedPath) $placedPath
    if (Test-Path $placedPath) {
        $same = (Get-FileHash $placedPath -Algorithm SHA256).Hash -eq (Get-FileHash $bgPath -Algorithm SHA256).Hash
        Check '投放的文件与上传的底图**逐字节相同**' $same '拷贝过程改了内容'
    }

    if ($Matte) {
        # ★ **AI 软遮罩那一档**（2026-09-27）：工具**不写遮罩** —— 真正的 alpha 由工作流里的
        #   `easy imageRemBg` 现算（`refine_foreground=true`，模型自己做前景估计），它只回 `personFilename`。
        #   所以这里一条遮罩文件断言都不该有，取而代之的是"它**没**写遮罩、人物图**一个像素都没改**"。
        Check 'matte 档如实报了 matteSource=model' ($mask -and $mask.matteSource -eq 'model') ($mask | ConvertTo-Json -Compress)
        Check 'matte 档**不写**合成遮罩（maskGenerated=false）' ($mask.maskGenerated -eq $false) "$($mask.maskGenerated)"
        Check 'matte 档不给遮罩文件名' (-not $mask.filename) "$($mask.filename)"
        Check 'matte 档回了 personFilename（那才是 20.image 该填的）' ($mask.personFilename) "$($mask.personFilename)"
        Check 'matte 档的 hint 指向 matte 工作流' ($mask.hint -match 'comfyhub_paste_person_matte_anima\.json') $mask.hint
        Check 'matte 档的 hint 明说**不要**覆盖 LoadImageMask' ($mask.hint -match '不要' -and $mask.hint -match 'LoadImageMask') $mask.hint
        Check 'matte 档尺寸 = 画布尺寸（ImageScale 照它覆盖）' `
            ($mask.width -eq $canvasW -and $mask.height -eq $canvasH) "$($mask.width)x$($mask.height)"
        Say "  遮罩来源：AI 软遮罩 + 模型自己做前景估计（工作流里的抠图节点）｜人物图（原样交给模型）：$($mask.personFilename)" 'White'
    } else {
        Check '遮罩尺寸 = 画布尺寸' ($mask -and $mask.width -eq $canvasW -and $mask.height -eq $canvasH) `
            "遮罩 $($mask.width)x$($mask.height) / 画布 ${canvasW}x${canvasH}"
        Check '遮罩如实报了白区语义（白 = 重绘区）' ($mask -and $mask.whiteMeans) $mask.whiteMeans
        $maskPath = Join-Path $instanceInput $mask.filename
        Check '遮罩文件真的在盘上' (Test-Path $maskPath) $maskPath
        if (Test-Path $maskPath) {
            $maskImage = [System.Drawing.Bitmap]::FromFile($maskPath)
            $parts = $Region.Split(',') | ForEach-Object { [double]$_.Trim() }
            $inX = [int]($parts[0] * $canvasW) + [int]($parts[2] * $canvasW / 2)
            $inY = [int]($parts[1] * $canvasH) + [int]($parts[3] * $canvasH / 2)
            $insidePixel = $maskImage.GetPixel($inX, $inY).R
            $outsidePixel = $maskImage.GetPixel(2, 2).R
            $maskImage.Dispose()
            if ($mask.mode -eq 'subject') {
                # 剪影档：形状由人物图决定，**不能**再按 region 矩形取点
                # （实测：拿矩形中心当"重绘区"，那里可能正好是剪影边缘 → 假红）
                Check '剪影遮罩有内容（占比在 0.5%~90% 之间）' `
                    ($mask.subjectRatio -gt 0.005 -and $mask.subjectRatio -lt 0.9) "subjectRatio=$($mask.subjectRatio)"
                Check '剪影遮罩如实报了背景色与来源图' `
                    ($mask.background -and $mask.subjectFilename) ($mask | ConvertTo-Json -Compress)
                Check '剪影遮罩在画布四角是黑的（白底没漏进来）' ($outsidePixel -lt 40) "角落像素值 $outsidePixel"
            } else {
                Check '遮罩在重绘区是白的' ($insidePixel -gt 200) "重绘区像素值 $insidePixel"
                Check '遮罩在保留区是黑的' ($outsidePixel -lt 40) "保留区像素值 $outsidePixel"
            }
        }
    }

    # `ring`：只圈人物外面一圈（人物像素一个都不动）—— 文件要在，而且只能是**薄薄一圈**
    # ⚠ matte 那一档不传 ring（它的 ring 由工作流里的 GrowMask+MaskComposite 现算）
    if ($Ring -gt 0 -and -not $Matte) {
        $ringPath = Join-Path $instanceInput $mask.ringFilename
        Check '环形遮罩文件真的在盘上' ($mask.ringFilename -and (Test-Path $ringPath)) "$($mask.ringFilename)"
        if (Test-Path $ringPath) {
            $ringImage = [System.Drawing.Bitmap]::FromFile($ringPath)
            $white = 0; $total = 0
            for ($py = 0; $py -lt $ringImage.Height; $py += 4) {
                for ($px = 0; $px -lt $ringImage.Width; $px += 4) {
                    if ($ringImage.GetPixel($px, $py).R -gt 127) { $white++ }
                    $total++
                }
            }
            $ringImage.Dispose()
            $ringRatio = [Math]::Round($white / [Math]::Max(1, $total), 4)
            Say ("  环形遮罩占比：$([Math]::Round($ringRatio * 100, 1))%（只该是人物边缘那一圈）") 'White'
            # 环的大小取决于**剪影的周长**（尖发/大裙子的剪影周长很长，12px 环就大），
            # 所以判据是"比剪影大、但不到离谱" —— 绝对值卡死会在正常图上假红
            Check '环形遮罩是绕在剪影外面的一圈' `
                ($ringRatio -gt 0 -and $ringRatio -lt ($mask.subjectRatio * 3 + 0.05)) `
                "ring=$ringRatio silhouette=$($mask.subjectRatio)"
        }
    }

    Check '提交成功（ok=true）' ($submitted -and $submitted.status -eq 'success') `
        ($byName['comfy_submit'].text)
    Check '提交结果里带 mediaIds（界面据此贴画廊入口卡）' `
        ($submitted -and @($submitted.mediaIds).Count -ge 1) ($submitted | ConvertTo-Json -Compress)

    # 两阶段：人物图真的被投放了，而且 `20.image` / `denoise` 的覆盖**真的生效**
    # （`appliedOverrides` 是工具自己报的"我改了哪几个键"，比正文里的叙述硬）
    if ($PersonMediaId -gt 0) {
        $person = $null
        if ($byName.ContainsKey('comfy_use_media')) { $person = $byName['comfy_use_media'].text | ConvertFrom-Json }
        Check '阶段一产物被投放进了 input 目录' `
            ($person -and $person.filename -and (Test-Path (Join-Path $instanceInput $person.filename))) `
            ($person | ConvertTo-Json -Compress)
        $applied = @($submitted.appliedOverrides)
        # ⚠ **摆放之后人物图换文件名了**（工具把像素与遮罩一起搬，回了 `placedSubjectFilename`）：
        #   断言要比这个新名字，拿原图名去比就是假红（实测踩过）。
        #   matte 那一档回的是 `personFilename`（**一个字都没改**的那一份 —— 抠图交给工作流里的模型）。
        $expectPerson = if ($mask.personFilename) { $mask.personFilename }
                        elseif ($mask.placedSubjectFilename) { $mask.placedSubjectFilename }
                        else { $person.filename }
        $personKey = @($applied | Where-Object { $_ -match ('^\d+\.image ← ' + [regex]::Escape($expectPerson) + '$') })
        Check '人物图覆盖写进了工作流（LoadImage 两份：底图 + 人物图）' ($personKey.Count -ge 1) ($applied -join ' | ')
        if ($Matte) {
            # ★ 这一档的**核心断言**：matte 工作流里**没有 LoadImageMask**（遮罩是节点输出），
            #   所以任何一条 `N.image ← …` 只允许落在 LoadImage 节点（10 / 20）上。
            #   要是有人把 `12.image` / `26.image` 也写进 overrides，说明它还在走旧路。
            $imageOverrides = @($applied | Where-Object { $_ -match '^\d+\.image ← ' })
            $onlyLoadImage = @($imageOverrides | Where-Object { $_ -notmatch '^(10|20)\.image ← ' })
            Check 'matte 档只覆盖 LoadImage（10 / 20），一个 LoadImageMask 都没覆盖' `
                ($onlyLoadImage.Count -eq 0) ($imageOverrides -join ' | ')
            Check "20.image 指向的就是那份人物图 $expectPerson" `
                (@($applied | Where-Object { $_ -match ('^20\.image ← ' + [regex]::Escape($expectPerson) + '$') }).Count -ge 1) `
                ($applied -join ' | ')
        }
        if ($mask.placedSubjectFilename) {
            Check '摆放用的是搬过的新人物图（不是原图）' `
                (@($applied | Where-Object { $_ -match ('^\d+\.image ← ' + [regex]::Escape($mask.placedSubjectFilename) + '$') }).Count -ge 1) `
                ($applied -join ' | ')
        }
        if ($Ring -gt 0 -and -not $Matte) {
            Check '重绘区用的是 ring 遮罩（第二个 LoadImageMask）' `
                (@($applied | Where-Object { $_ -match ('^\d+\.image ← ' + [regex]::Escape($mask.ringFilename) + '$') }).Count -ge 1) `
                ($applied -join ' | ')
        }
        Check "denoise 被覆盖成 $PersonDenoise（贴人物图的融合档）" `
            (@($applied | Where-Object { $_ -match ('\.denoise ← ' + [regex]::Escape("$PersonDenoise") + '$') }).Count -ge 1) `
            ($applied -join ' | ')
        if ($Cfg -gt 0) {
            # ⚠ PowerShell 的 `"$Cfg"` 对 4.0 给的是 "4"（而工具回的是 "4.0"），
            #   所以这里允许一个可选的小数尾巴
            Check "cfg 被覆盖成 $Cfg（提示词约束变强）" `
                (@($applied | Where-Object { $_ -match ('\.cfg ← ' + [regex]::Escape("$Cfg") + '(\.0+)?$') }).Count -ge 1) `
                ($applied -join ' | ')
        }
    }
    if ($submitted -and $submitted.mediaIds) { $script:CreatedMediaIds += @($submitted.mediaIds) }
    if ($submitted -and $submitted.capturedPromptId) { $script:CreatedPromptIds += $submitted.capturedPromptId }
    $editMediaId = if ($submitted -and $submitted.mediaIds) { @($submitted.mediaIds)[0] } else { 0 }

    # --- 8. 量化验收：遮罩内大变、遮罩外基本没动 --------------------------
    Say ''
    Say '  [3/3] 像素级验收（该换的换了、该保的保住了）' 'Cyan'
    if ($editMediaId) {
        $outFile = Join-Path $WorkDir 'edit-out.png'
        Invoke-WebRequest -Uri "$ApiBase/api/media/$editMediaId/file" -OutFile $outFile -TimeoutSec 120 | Out-Null
        Check '产物能从画廊取回' (Test-Path $outFile) $outFile
        $out = [System.Drawing.Bitmap]::FromFile($outFile)
        Check '产物尺寸 = 画布尺寸' ($out.Width -eq $canvasW -and $out.Height -eq $canvasH) `
            "产物 $($out.Width)x$($out.Height)"
        $cmpW = [Math]::Min($canvasW, $out.Width)
        $cmpH = [Math]::Min($canvasH, $out.Height)
        if ($Matte) {
            # matte 档**没有遮罩文件**（真正的 alpha 在工作流里）—— 量尺跟着换，但仍然量化：
            #   ① 用**人物图**现推一个近似剪影（同"颜色离背景多远"那套口径，**只当验收用**）；
            #   ② 人物区域必须**明显变了**（人真的被贴上去了）；
            #   ③ **远处背景**必须基本没动 —— ⚠ 近处背景本来就会变：这一档要重绘
            #      人物外面 14 像素那圈（ring 由 GrowMask+MaskComposite 现算），拿近处当判据是假红。
            $personPath = Join-Path $instanceInput $expectPerson
            $personBmp = if (Test-Path $personPath) { [System.Drawing.Bitmap]::FromFile($personPath) } else { $null }
            Check '人物图（原样交给模型的那份）在盘上，能当量尺' ($null -ne $personBmp) $personPath
            $bgCorner = if ($personBmp) { $personBmp.GetPixel(1, 1) } else { $null }
            $stride = 4
            $gw = [int][Math]::Ceiling($cmpW / $stride)
            $gh = [int][Math]::Ceiling($cmpH / $stride)
            # ① 近似剪影 + ② 把它**膨胀 24 像素**（> ring 的 14）——"远处背景"就是膨胀之外的地方。
            # ⚠ **不能用"画面外圈 15%"当远处背景**：这个人物是**全长双马尾**，头发几乎顶到左右两边，
            #   外圈那条带里有一大半其实是人物 / 环 —— 实测假红（远处背景 12.27）。
            $sil = New-Object 'bool[,]' $gw, $gh
            if ($personBmp) {
                for ($gy = 0; $gy -lt $gh; $gy++) {
                    for ($gx = 0; $gx -lt $gw; $gx++) {
                        $px = [Math]::Min($cmpW - 1, $gx * $stride)
                        $py = [Math]::Min($cmpH - 1, $gy * $stride)
                        $pc = $personBmp.GetPixel($px, $py)
                        $dist = [Math]::Abs($pc.R - $bgCorner.R) + [Math]::Abs($pc.G - $bgCorner.G) + [Math]::Abs($pc.B - $bgCorner.B)
                        if ($dist -gt 90) { $sil[$gx, $gy] = $true }
                    }
                }
            }
            # 便宜的可分离膨胀（横一遍、竖一遍）：半径 R 格 = R*stride 像素
            $R = [int][Math]::Ceiling(24 / $stride)
            $tmp = New-Object 'bool[,]' $gw, $gh
            for ($gy = 0; $gy -lt $gh; $gy++) {
                for ($gx = 0; $gx -lt $gw; $gx++) {
                    $hit = $false
                    for ($k = -$R; $k -le $R -and -not $hit; $k++) {
                        $nx = $gx + $k
                        if ($nx -ge 0 -and $nx -lt $gw -and $sil[$nx, $gy]) { $hit = $true }
                    }
                    $tmp[$gx, $gy] = $hit
                }
            }
            $near = New-Object 'bool[,]' $gw, $gh
            for ($gx = 0; $gx -lt $gw; $gx++) {
                for ($gy = 0; $gy -lt $gh; $gy++) {
                    $hit = $false
                    for ($k = -$R; $k -le $R -and -not $hit; $k++) {
                        $ny = $gy + $k
                        if ($ny -ge 0 -and $ny -lt $gh -and $tmp[$gx, $ny]) { $hit = $true }
                    }
                    $near[$gx, $gy] = $hit
                }
            }
            $pSum = 0.0; $pN = 0; $farSum = 0.0; $farN = 0; $allSum = 0.0; $allN = 0
            for ($py = 0; $py -lt $cmpH; $py += $stride) {
                $gy = [int]($py / $stride)
                for ($px = 0; $px -lt $cmpW; $px += $stride) {
                    $gx = [int]($px / $stride)
                    $a = $canvas.GetPixel($px, $py)
                    $b = $out.GetPixel($px, $py)
                    $d = ([Math]::Abs($a.R - $b.R) + [Math]::Abs($a.G - $b.G) + [Math]::Abs($a.B - $b.B)) / 3.0
                    $allSum += $d; $allN++
                    if ($sil[$gx, $gy]) { $pSum += $d; $pN++ }
                    if (-not $near[$gx, $gy]) { $farSum += $d; $farN++ }
                }
            }
            if ($personBmp) { $personBmp.Dispose() }
            $allAvg = [Math]::Round($allSum / [Math]::Max(1, $allN), 2)
            $pAvg = if ($pN) { [Math]::Round($pSum / $pN, 2) } else { -1 }
            $farAvg = if ($farN) { [Math]::Round($farSum / $farN, 2) } else { -1 }
            Say ("  人物区域平均像素差：$pAvg / 255（近似剪影 $pN 个采样）") 'White'
            Say ("  远处背景平均像素差：$farAvg / 255（离剪影 >24 像素，$farN 个采样）") 'White'
            Check '远处背景采样够多（判据不是空跑）' ($farN -gt 500) $farN
            Check '产物与底图不同（真的跑了一遍）' ($allAvg -gt 0.5) $allAvg
            Check '人物区域**真的被贴上去了**（平均差 > 12）' ($pAvg -gt 12) $pAvg
            Check '远处背景**基本没动**（平均差 < 8；matte 只重绘人物外 14 像素那圈）' ($farAvg -ge 0 -and $farAvg -lt 8) $farAvg
            $out.Dispose()
        } else {
        # ⚠ **按遮罩本身**分内外，不按 region 矩形：`mode="subject"` 的修改区是**人物剪影**
        #   （比矩形宽/窄都可能），拿矩形当判据会得到"遮罩外变了 68.95"这种假红（实测踩过）。
        #   这也正是这个特性唯一的不变量：**白 = 重绘区、黑 = 原样保留**。
        $maskBitmap = [System.Drawing.Bitmap]::FromFile($maskPath)
        # ⚠ 有 ring 时**重绘区 = 剪影 ∪ 环**：只用剪影分内外，环里那些被重画的地方
        #   会被算成"外"（实测假红：遮罩外 12.2/255）。所以把环也并进来。
        $ringBitmap = $null
        if ($Ring -gt 0 -and $mask.ringFilename) {
            $ringPathForMetric = Join-Path $instanceInput $mask.ringFilename
            if (Test-Path $ringPathForMetric) { $ringBitmap = [System.Drawing.Bitmap]::FromFile($ringPathForMetric) }
        }
        $stride = 3
        $inSum = 0.0; $inN = 0; $outSum = 0.0; $outN = 0
        for ($py = 0; $py -lt $cmpH; $py += $stride) {
            for ($px = 0; $px -lt $cmpW; $px += $stride) {
                $a = $canvas.GetPixel($px, $py)
                $b = $out.GetPixel($px, $py)
                $d = ([Math]::Abs($a.R - $b.R) + [Math]::Abs($a.G - $b.G) + [Math]::Abs($a.B - $b.B)) / 3.0
                # 羽化带（遮罩里的中间值）既不算内也不算外
                $mv = $maskBitmap.GetPixel($px, $py).R
                $rv = if ($ringBitmap) { $ringBitmap.GetPixel($px, $py).R } else { 0 }
                $repaint = [Math]::Max($mv, $rv)
                if ($mv -ge 200) { $inSum += $d; $inN++ } elseif ($repaint -le 30) { $outSum += $d; $outN++ }
            }
        }
        $maskBitmap.Dispose()
        if ($ringBitmap) { $ringBitmap.Dispose() }
        $inAvg = if ($inN) { [Math]::Round($inSum / $inN, 2) } else { -1 }
        $outAvg = if ($outN) { [Math]::Round($outSum / $outN, 2) } else { -1 }
        Say ("  遮罩内平均像素差：$inAvg / 255（采样步长 $stride）") 'White'
        Say ("  遮罩外平均像素差：$outAvg / 255") 'White'
        Say '  （遮罩外不是 0 是对的：非遮罩区也要过一遍 VAE 编码→解码，那一步有损）' 'DarkGray'
        Check '遮罩内**真的被重画了**（平均差 > 12）' ($inAvg -gt 12) $inAvg
        Check '遮罩外**基本没动**（平均差 < 8）' ($outAvg -ge 0 -and $outAvg -lt 8) $outAvg
        Check '内外差异拉开（遮罩内至少是遮罩外的 2 倍）' ($inAvg -gt $outAvg * 2) "内 $inAvg / 外 $outAvg"
        $out.Dispose()
        }
    }

    # --- 9. 画廊产物也能投进 input（两阶段编辑的第二阶段）-----------------
    if ($editMediaId) {
        Say ''
        Say '  [加演] 画廊产物投放（comfy_use_media）' 'Cyan'
        $conv2 = Invoke-Api 'POST' '/api/ai/conversations' @{ providerId = $ProviderId; modelId = 'fake-edit-model' }
        $script:CreatedConversationIds += $conv2.id
        $run2 = Invoke-Api 'POST' "/api/ai/conversations/$($conv2.id)/runs" @{
            text = "投放产物 $editMediaId"; providerId = $ProviderId; modelId = 'fake-edit-model'
        }
        $assistant2 = $null
        $deadline = (Get-Date).AddSeconds(120)
        while ((Get-Date) -lt $deadline) {
            $null = Approve-PendingCalls $run2.runId -TimeoutSeconds 3
            $msgs2 = Invoke-Api 'GET' "/api/ai/conversations/$($conv2.id)/messages" $null
            $assistant2 = @($msgs2 | Where-Object { $_.id -eq $run2.assistantMessageId }) | Select-Object -First 1
            if ($assistant2 -and ($assistant2.status -eq 'complete' -or $assistant2.status -eq 'failed')) { break }
            Start-Sleep -Milliseconds 500
        }
        $r2 = @($assistant2.parts | Where-Object { $_.type -eq 'tool_result' }) | Select-Object -First 1
        Check '画廊产物投放工具被调到了' ($r2 -and $r2.jsonPayload.name -eq 'comfy_use_media') $r2.jsonPayload.name
        if ($r2) {
            $placed2 = $r2.text | ConvertFrom-Json
            Check '投放的画廊产物带原图尺寸' ($placed2.width -eq $canvasW -and $placed2.height -eq $canvasH) `
                "$($placed2.width)x$($placed2.height) / 画布 ${canvasW}x${canvasH}"
            Check '画廊产物也落进了实例在用的 input 目录' (Test-Path (Join-Path $instanceInput $placed2.filename)) $placed2.filename
        }
    }

    $canvas.Dispose()
} catch {
    Say ("  脚本异常: " + $_.Exception.Message) 'Red'
    $script:Failed++
} finally {
    # ---------------------------------------------------------------------
    #  收尾：**无论怎么退出都要还原**（捕获配置 / 权限档 / 临时数据）
    # ---------------------------------------------------------------------
    if ($script:Gateway) { try { Stop-Process -Id $script:Gateway.Id -Force -ErrorAction SilentlyContinue } catch { } }
    if ($script:OldConfig) {
        try {
            Invoke-Api 'PUT' '/api/capture/config' @{
                enabled = $script:OldConfig.enabled; comfyUrl = $script:OldConfig.comfyUrl
                outputDir = $script:OldConfig.outputDir; pollSeconds = $script:OldConfig.pollSeconds
                autoTag = $script:OldConfig.autoTag; maxPerPoll = $script:OldConfig.maxPerPoll
                downloadFallback = $script:OldConfig.downloadFallback
            } | Out-Null
            Say '  已还原自动捕获配置' 'DarkGray'
        } catch { Say "  还原配置失败: $($_.Exception.Message)" 'Yellow' }
    }
    if ($script:PermissionBefore) {
        try {
            Invoke-Api 'PUT' '/api/ai/tools/policy' @{ permissionMode = $script:PermissionBefore } | Out-Null
            Say "  已把权限档还原成 '$($script:PermissionBefore)'" 'DarkGray'
        } catch { Say "  还原权限档失败: $($_.Exception.Message)" 'Yellow' }
    }
    try { Invoke-Api 'DELETE' "/api/ai/providers/$ProviderId" $null | Out-Null } catch { }

    if ($KeepData) {
        Say '  -KeepData：保留了测试产生的数据' 'Yellow'
    } else {
        foreach ($id in $script:CreatedConversationIds) {
            try { Invoke-Api 'DELETE' "/api/ai/conversations/$id" $null | Out-Null } catch { }
        }
        foreach ($id in ($script:CreatedMediaIds | Select-Object -Unique)) {
            try { Invoke-Api 'DELETE' "/api/media/$id" $null | Out-Null } catch { }
        }
        foreach ($id in ($script:CreatedPromptIds | Select-Object -Unique)) {
            try { Invoke-Api 'DELETE' "/api/prompts/$id" $null | Out-Null } catch { }
        }
        if ($script:AttachmentId) {
            try { Invoke-Api 'DELETE' "/api/ai/attachments/$($script:AttachmentId)" $null | Out-Null } catch { }
        }
        # input 目录里那几个临时文件（投放的底图 / 遮罩 / 画廊产物）也清掉，别在用户的 ComfyUI 里攒垃圾。
        # ⚠ 用通配：投放的文件名是 **`<我们想要的名字>-<来源 id 前 8 位>.png`**（防重名那一段后缀），
        #   所以 `edit-bg.png` / `edit-stage1.png` 落盘之后叫 `edit-bg-00000000.png` /
        #   `edit-stage1-m235.png` —— 只按"想要的名字"删会一个都删不掉（实测留了一地）。
        foreach ($pattern in @('edit-bg*.png', 'edit-mask*.png', 'edit-person*.png', 'edit-stage1*.png')) {
            if ($script:InputDir) {
                Get-ChildItem -Path $script:InputDir -Filter $pattern -File -ErrorAction SilentlyContinue |
                    Remove-Item -Force -ErrorAction SilentlyContinue
            }
        }
        Remove-Item $WorkDir -Recurse -Force -ErrorAction SilentlyContinue
        Say ("  已清理：{0} 个产物 / {1} 条提示词 / {2} 个会话" -f `
                ($script:CreatedMediaIds | Select-Object -Unique).Count, $script:CreatedPromptIds.Count,
            $script:CreatedConversationIds.Count) 'DarkGray'
    }
}

Say ''
if ($script:Failed -gt 0) {
    Say ("  ✗ 有 $script:Failed 项没通过，请看上面的 [FAIL]") 'Red'
    exit 1
}
Say '  ✓ 全部通过：AI 投放底图 → 生成遮罩 → 提交重绘；遮罩内大变、遮罩外没动' 'Green'
Say ("  （{0} 项检查）" -f $script:Checks) 'DarkGray'
Say ''
exit 0
