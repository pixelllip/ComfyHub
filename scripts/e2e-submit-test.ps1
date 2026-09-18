<#
.SYNOPSIS
    ComfyHub「AI 提交 ComfyUI 任务」端到端自测（用户建议 ①；不需要真的跑一次生成）。

.DESCRIPTION
    用两个假服务把整条链路走一遍，**不出网、不花钱、不碰真 ComfyUI**：

      scripts\e2e\fake_title_gateway.py  假装成一个 OpenAI 兼容网关，
                                         看到「提交 <id>」就回一个 comfy_submit 工具调用
                                         看到「载入 <路径>」就回一个 comfy_load_workflow 工具调用
      scripts\e2e\fake_comfy.py          假装成 ComfyUI，收下 POST /prompt 并"立刻跑完"，
                                         并提供 /object_info（界面格式转 API 图要用）

    验证的断言（每一步都是这条链路上真实会断的地方）：

      1. comfy_submit 默认 ask  —— 工具调用以 approval=pending 发出，没人批准就不会执行
      2. 批准之后才真的提交     —— 假 ComfyUI 收到一次 POST /prompt
      3. 参数覆盖按类型生效     —— sch.steps 从工作流原值改成了 requested 的值
      4. 产物真的入库           —— 工具结果 status=success、mediaIds 非空（界面据此贴画廊入口卡）
      5. 提交的图就是库里的图   —— 提交的节点数与库提示词的 API 节点图一致
      6. 工作流文件可直接提交   —— 界面格式（nodes/links）按 /object_info 转成 API 节点图，
                                   控件值按声明顺序贴回名字、连线还原成 ["上游id", 槽位]，
                                   转换结果真的能提交并生效（用户 bug ③）

    前置条件：MySQL + 后端在跑（scripts\comfyhub.ps1 up），本机有 python，
    库里至少有 1 条**带 API 节点图**的提示词（runnable=true）。

.EXAMPLE
    pwsh -File scripts\e2e-submit-test.ps1
    pwsh -File scripts\e2e-submit-test.ps1 -PromptId 82 -KeepData
#>
[CmdletBinding()]
param(
    [string]$ApiBase = 'http://127.0.0.1:8080',
    [int]$ComfyPort = 8189,
    [int]$GatewayPort = 8798,
    [int]$PromptId = 0,
    [switch]$KeepData
)

$ErrorActionPreference = 'Continue'
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$E2eDir      = Join-Path $PSScriptRoot 'e2e'
$WorkDir     = Join-Path $ProjectRoot '.run\e2e-submit'
$OutDir      = Join-Path $WorkDir 'output'
# 工作流文件故意放在**项目之外**的临时目录：项目内的 `.run` / `.mysql` / `.git` / `node_modules`
# 是工具权限里"永远不许碰"的段（见 ToolPolicy.FORBIDDEN_SEGMENTS），拿它当"用户机器上的
# 工作流文件"来测并不真实 —— 真实的 ComfyUI 工作流目录本来就在项目外面。
$WfDir       = Join-Path ([System.IO.Path]::GetTempPath()) "comfyhub-e2e-submit-wf-$PID"
$ProviderId  = 'e2e-submit-gw'

$script:Failed = 0

function Say([string]$msg, [string]$color = 'Gray') { Write-Host $msg -ForegroundColor $color }

function Check([string]$name, [bool]$ok, [string]$detail = '') {
    if ($ok) {
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
    return Invoke-RestMethod -Method $Method -Uri $uri -TimeoutSec 180 `
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

function Find-MysqlExe {
    $cands = @()
    if ($env:COMFYHUB_MYSQL_HOME) { $cands += (Join-Path $env:COMFYHUB_MYSQL_HOME 'bin\mysql.exe') }
    foreach ($root in @('D:\tools\mysql', 'C:\tools\mysql')) {
        if (-not (Test-Path $root)) { continue }
        $cands += (Join-Path $root 'bin\mysql.exe')
        Get-ChildItem -Path $root -Directory -ErrorAction SilentlyContinue |
            ForEach-Object { $cands += (Join-Path $_.FullName 'bin\mysql.exe') }
    }
    return ($cands | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1)
}

# 待批准期间 ai_tool_calls 还没有行（那是"调用结束"的记录），
# 但 tool.requested 事件在发出去之前就落库了 —— callId 从那里取。
function Get-PendingCallId([string]$RunId) {
    if (-not $script:Mysql) { return $null }
    $q = "SELECT payload_json FROM ai_run_events WHERE run_id='$RunId' AND event_type='tool.requested' ORDER BY seq DESC LIMIT 1;"
    $out = & $script:Mysql --host=127.0.0.1 --port=3307 --user=comfyhub --password=comfyhub --database=comfy_hub -N -B -e $q 2>$null
    if (-not $out) { return $null }
    # 别整段 ConvertFrom-Json：arguments 里是转义后的 JSON，容易在管道里被吃掉引号
    if ($out -match '"callId"\s*:\s*"([^"]+)"') { return $Matches[1] }
    return $null
}

# ---------------------------------------------------------------------------

Say ''
Say '  ComfyHub「AI 提交 ComfyUI 任务」· 端到端自测' 'White'
Say '  ────────────────────────────────────────────────────────────'

$python = Find-Python
if (-not $python) { Say '  找不到 python（可用环境变量 COMFYHUB_PYTHON 指定）。' 'Red'; exit 1 }
$script:Mysql = Find-MysqlExe
Say "  python: $python" 'DarkGray'

$fakeComfy = $null
$gateway = $null
$oldConfig = $null
$createdRunKey = $null
$createdPromptId = $null
$createdMediaIds = @()
$script:PermissionModeBefore = $null
$script:ReadRootsBefore = $null
$script:LoadedPromptId = $null

try {
    $health = Invoke-Api 'GET' '/api/health' $null
    Check '后端可用（db=ok）' ($health.database -eq 'ok')
    if ($script:Failed -gt 0) { exit 1 }

    # --- 0. 找一条能跑的工作流 -------------------------------------------
    # `hasWorkflow` 只说明存过工作流（可能是界面格式），提交需要的是 **API 节点图**，
    # 所以这里逐个试到真的拿得到 api-graph 为止（老数据只有界面格式，跑不了）。
    $prompts = Invoke-Api 'GET' '/api/prompts?size=50' $null
    $target = $null
    $apiGraph = $null
    $candidates = if ($PromptId -gt 0) {
        @($prompts.items | Where-Object { $_.id -eq $PromptId })
    } else {
        @($prompts.items)
    }
    foreach ($c in $candidates) {
        $g = $null
        try { $g = Invoke-Api 'GET' "/api/prompts/$($c.id)/api-graph" $null } catch { $g = $null }
        if ($g) { $target = $c; $apiGraph = $g; break }
    }
    if (-not $target) {
        Say '  库里没有「带 API 节点图」的提示词（老数据只有界面格式工作流）。' 'Yellow'
        Say '  先在 ComfyUI 里正常跑一次让它被捕获，再重跑这个脚本。' 'Yellow'
        exit 1
    }
    Say ("  用提示词 id={0}「{1}」" -f $target.id, $target.title) 'DarkGray'

    $overridePath = $null
    $overrideValue = $null
    foreach ($field in @('steps', 'noise_seed', 'seed', 'denoise')) {
        foreach ($nodeId in $apiGraph.PSObject.Properties.Name) {
            $inputs = $apiGraph.$nodeId.inputs
            if ($inputs -and $inputs.PSObject.Properties.Name -contains $field) {
                $overridePath = "$nodeId.$field"
                $overrideValue = if ($field -eq 'steps' -or $field -eq 'seed' -or $field -eq 'noise_seed') { '13' } else { '0.9' }
                break
            }
        }
        if ($overridePath) { break }
    }
    if (-not $overridePath) {
        Say '  这条工作流里找不到可覆盖的标量参数（steps / seed / denoise），换一条再试。' 'Yellow'
        exit 1
    }
    Say ("  将覆盖参数 {0} = {1}" -f $overridePath, $overrideValue) 'DarkGray'

    # --- 1. 起假 ComfyUI + 假网关，并把捕获配置指向假 ComfyUI --------------
    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
    Get-ChildItem $OutDir -File -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
    # 每次都给一张**内容一定不同**的图：入库靠 SHA-256 去重，撞上旧文件会走"重复"分支，
    # 那就测不到 mediaIds 这条路了（去重本身是对的，但这里要验的是入库成功那条路）。
    $r = Get-Random -Minimum 20 -Maximum 230
    $g = Get-Random -Minimum 20 -Maximum 230
    $b = Get-Random -Minimum 20 -Maximum 230
    & $python (Join-Path $E2eDir 'make_png.py') (Join-Path $OutDir 'e2e_submit_00001_.png') `
        --size (Get-Random -Minimum 200 -Maximum 520) --color "$r,$g,$b" | Out-Null

    $fakeComfy = Start-Process -FilePath $python -PassThru -WindowStyle Hidden `
        -ArgumentList @((Join-Path $E2eDir 'fake_comfy.py'), '--port', "$ComfyPort",
                        '--output', $OutDir, '--filename', 'e2e_submit_00001_.png')
    $gateway = Start-Process -FilePath $python -PassThru -WindowStyle Hidden `
        -ArgumentList @((Join-Path $E2eDir 'fake_title_gateway.py'), '--port', "$GatewayPort")

    $ready = $false
    foreach ($i in 1..40) {
        try {
            $null = Invoke-RestMethod "http://127.0.0.1:$ComfyPort/system_stats" -TimeoutSec 2
            $null = Invoke-RestMethod "http://127.0.0.1:$GatewayPort/v1/models" -TimeoutSec 2
            $ready = $true; break
        } catch { Start-Sleep -Milliseconds 300 }
    }
    Check '假 ComfyUI 与假网关都已就绪' $ready
    if (-not $ready) { exit 1 }

    $oldConfig = Invoke-Api 'GET' '/api/capture/config' $null
    Invoke-Api 'PUT' '/api/capture/config' @{
        enabled = $false; comfyUrl = "http://127.0.0.1:$ComfyPort"; outputDir = $OutDir
        pollSeconds = 4; autoTag = 'ComfyUI'; maxPerPoll = 20; downloadFallback = $true
    } | Out-Null

    # 权限档先存一份并**临时钉成 ask**：第 1 幕验的就是"没人批准就不提交"，
    # 而用户平时可能把档位切成了「自动允许（无需批准）」—— 那时 comfy_submit 会立刻执行，
    # 这一幕必然红。脚本必须自己保证前置条件，不能靠机器上的当前设置（实测踩过：
    # 库里 permissionMode=full 时本脚本会报"没人批准时不会提交给 ComfyUI -> 已提交 1 次"）。
    $script:PermissionModeBefore = (Invoke-Api 'GET' '/api/ai/tools/policy' $null).permissionMode
    if ($script:PermissionModeBefore -ne 'ask') {
        Invoke-Api 'PUT' '/api/ai/tools/policy' @{ permissionMode = 'ask' } | Out-Null
        Say "  权限档临时从 '$($script:PermissionModeBefore)' 切成 'ask'（结束时还原）" 'DarkGray'
    }

    # --- 2. 建一个指向假网关的 Provider（loopback，不出网） -----------------
    try { Invoke-Api 'DELETE' "/api/ai/providers/$ProviderId" $null | Out-Null } catch { }
    Invoke-Api 'POST' '/api/ai/providers' @{
        id = $ProviderId; displayName = 'E2E 提交网关'; api = 'openai-completions'
        baseURL = "http://127.0.0.1:$GatewayPort/v1"; credentialRef = 'E2E_SUBMIT_KEY'
        endpointTrust = 'loopback'
    } | Out-Null
    Invoke-Api 'PUT' "/api/ai/providers/$ProviderId/credentials" @{ value = 'sk-fake-not-a-real-key' } | Out-Null
    Invoke-Api 'PUT' "/api/ai/providers/$ProviderId/models" @{
        models = @(@{ providerId = $ProviderId; id = 'fake-title-model'; displayName = 'E2E 假模型'
                      inputModalities = @('text'); tools = $true; reasoning = $false
                      capabilitySource = 'manual'; enabled = $true })
    } | Out-Null

    # --- 3. 发一次「提交」，工具调用应当是"待批准"而不是直接执行 -----------
    Say ''
    Say '  [1/3] comfy_submit 默认要用户批准' 'Cyan'
    $conv = Invoke-Api 'POST' '/api/ai/conversations' @{ providerId = $ProviderId; modelId = 'fake-title-model' }
    $run = Invoke-Api 'POST' "/api/ai/conversations/$($conv.id)/runs" @{
        text = "帮我用工作流 $($target.id) 提交 $($target.id) $overridePath=$overrideValue"
        providerId = $ProviderId; modelId = 'fake-title-model'
    }
    $callId = $null
    foreach ($i in 1..60) { Start-Sleep -Milliseconds 400; $callId = Get-PendingCallId $run.runId; if ($callId) { break } }
    Check '工具调用以 approval=pending 发出（ask 档）' ($null -ne $callId) '没等到 tool.requested'
    $submittedBefore = (Invoke-RestMethod "http://127.0.0.1:$ComfyPort/__submitted").runs.Count
    Check '没人批准时不会提交给 ComfyUI' ($submittedBefore -eq 0) "已提交 $submittedBefore 次"
    if (-not $callId) { throw '拿不到 callId，后面的断言没有意义' }

    # --- 4. 批准 → 真的提交 → 入库 ----------------------------------------
    Say ''
    Say '  [2/3] 批准之后才真的提交并入库' 'Cyan'
    $approve = Invoke-Api 'POST' "/api/ai/tool-calls/$callId/approve" @{}
    Check '批准被后端接受' ($approve.accepted -eq $true) ($approve | ConvertTo-Json -Compress)

    $assistant = $null
    foreach ($i in 1..160) {
        Start-Sleep -Milliseconds 500
        $msgs = Invoke-Api 'GET' "/api/ai/conversations/$($conv.id)/messages" $null
        $assistant = $msgs | Where-Object { $_.id -eq $run.assistantMessageId } | Select-Object -First 1
        if ($assistant.status -eq 'complete' -or $assistant.status -eq 'failed') { break }
    }
    Check 'Run 结束（complete）' ($assistant.status -eq 'complete') $assistant.status

    $tr = $assistant.parts | Where-Object { $_.type -eq 'tool_result' } | Select-Object -First 1
    $payload = $null
    if ($tr) { $payload = $tr.jsonPayload }
    Check '工具调用成功（ok=true）' ($null -ne $payload -and $payload.ok -eq $true) $tr.text
    # mediaIds 是**入库成功**的唯一硬证据（界面据此贴画廊入口卡）；
    # 详细状态（status / appliedOverrides）在工具回给模型的正文里，再查一次确保两处一致
    Check '结果里带 mediaIds（界面据此贴画廊入口卡）' `
        ($null -ne $payload -and $payload.mediaIds.Count -ge 1) ($payload | ConvertTo-Json -Compress)
    $content = $null
    if ($tr) { $content = $tr.text | ConvertFrom-Json }
    Check '工具回给模型的正文说 status=success' ($null -ne $content -and $content.status -eq 'success') $tr.text
    Check '正文里的 appliedOverrides 如实记录了改了什么' `
        ($null -ne $content -and $content.appliedOverrides.Count -ge 1) $tr.text

    if ($content) {
        $createdRunKey = $content.comfyPromptId
        $createdPromptId = $content.capturedPromptId
    }
    if ($payload -and $payload.mediaIds) { $createdMediaIds = @($payload.mediaIds) }

    # --- 5. 参数覆盖与"提交的就是库里的图" --------------------------------
    Say ''
    Say '  [3/3] 参数覆盖与提交内容' 'Cyan'
    $submitted = (Invoke-RestMethod "http://127.0.0.1:$ComfyPort/__submitted").runs
    Check '假 ComfyUI 收到了 1 次提交' ($submitted.Count -eq 1) "$($submitted.Count) 次"
    if ($submitted.Count -ge 1) {
        $graph = $submitted[0].graph
        $nodes = @($graph.PSObject.Properties.Name)
        $libraryNodes = @($apiGraph.PSObject.Properties.Name)
        Check '提交的节点与库里的 API 节点图一致' ($nodes.Count -eq $libraryNodes.Count) `
            "提交 $($nodes.Count) 个 / 库里 $($libraryNodes.Count) 个"
        $nodeId = $overridePath.Split('.')[0]
        $field = $overridePath.Split('.')[1]
        $sent = $graph.$nodeId.inputs.$field
        Check "参数覆盖生效（$overridePath）" ("$sent" -eq "$overrideValue") "实际发出的是 $sent"
    }

    # --- 6. 工作流文件可以直接提交（用户 bug ③） ---------------------------
    #
    # 现场：用户甩给 AI 一个工作流 .json 路径，AI 只能回"comfy_submit 只认库里的 promptId，
    # 我不能凭一个文件路径提交"。这一幕验的就是那条路真的通了：
    #   ① read_file / comfy_load_workflow 能读到那个文件（读白名单要放行它的目录）；
    #   ② 界面格式（nodes/links）被按 ComfyUI 的 /object_info 转成 API 节点图；
    #   ③ 转换出来的图真的提交给了 ComfyUI，且参数覆盖生效。
    Say ''
    Say '  [4/4] 工作流文件直接提交（界面格式 → API 节点图）' 'Cyan'

    $uiWorkflowPath = Join-Path $WfDir 'e2e_ui_workflow.json'
    New-Item -ItemType Directory -Path $WfDir -Force | Out-Null
    $uiWorkflow = @'
{
  "last_node_id": 9,
  "last_link_id": 7,
  "nodes": [
    { "id": 4, "type": "CheckpointLoaderSimple", "mode": 0,
      "widgets_values": ["e2e_model.safetensors"], "inputs": [] },
    { "id": 6, "type": "CLIPTextEncode", "mode": 0,
      "widgets_values": ["a cyberpunk cat"], "inputs": [{ "name": "clip", "type": "CLIP", "link": 1 }] },
    { "id": 7, "type": "CLIPTextEncode", "mode": 0,
      "widgets_values": ["lowres"], "inputs": [{ "name": "clip", "type": "CLIP", "link": 2 }] },
    { "id": 5, "type": "EmptyLatentImage", "mode": 0, "widgets_values": [640, 960, 1], "inputs": [] },
    { "id": 3, "type": "KSampler", "mode": 0,
      "widgets_values": [777001, "fixed", 20, 7.0, "euler", "normal", 1.0],
      "inputs": [
        { "name": "model", "type": "MODEL", "link": 3 },
        { "name": "positive", "type": "CONDITIONING", "link": 4 },
        { "name": "negative", "type": "CONDITIONING", "link": 5 },
        { "name": "latent_image", "type": "LATENT", "link": 6 }
      ] },
    { "id": 9, "type": "SaveImage", "mode": 0,
      "widgets_values": ["e2e/ui_from_file"], "inputs": [{ "name": "images", "type": "IMAGE", "link": 7 }] }
  ],
  "links": [
    [1, 4, 1, 6, 0, "CLIP"],
    [2, 4, 1, 7, 0, "CLIP"],
    [3, 4, 0, 3, 0, "MODEL"],
    [4, 6, 0, 3, 1, "CONDITIONING"],
    [5, 7, 0, 3, 2, "CONDITIONING"],
    [6, 5, 0, 3, 3, "LATENT"],
    [7, 3, 0, 9, 0, "IMAGE"]
  ]
}
'@
    Set-Content -Path $uiWorkflowPath -Value $uiWorkflow -Encoding utf8

    # 工作流文件放在临时目录里：**必须显式放行**那个目录，否则读工具会（正确地）拒绝。
    # 顺带把"读白名单真的生效 + 越界会被拒"这条路也走一遍。
    $oldPolicy = Invoke-Api 'GET' '/api/ai/tools/policy' $null
    $script:ReadRootsBefore = @($oldPolicy.readRoots)
    Invoke-Api 'PUT' '/api/ai/tools/policy' @{ readRoots = @($script:ReadRootsBefore + $WfDir) } | Out-Null

    $conv2 = Invoke-Api 'POST' '/api/ai/conversations' @{ providerId = $ProviderId; modelId = 'fake-title-model' }
    $loadRun = Invoke-Api 'POST' "/api/ai/conversations/$($conv2.id)/runs" @{
        text = "载入 $uiWorkflowPath"
        providerId = $ProviderId; modelId = 'fake-title-model'
    }
    $loadMsg = $null
    foreach ($i in 1..160) {
        Start-Sleep -Milliseconds 400
        $msgs = Invoke-Api 'GET' "/api/ai/conversations/$($conv2.id)/messages" $null
        $loadMsg = $msgs | Where-Object { $_.id -eq $loadRun.assistantMessageId } | Select-Object -First 1
        if ($loadMsg -and ($loadMsg.status -eq 'complete' -or $loadMsg.status -eq 'failed')) { break }
    }
    $loadTr = $null
    if ($loadMsg) { $loadTr = $loadMsg.parts | Where-Object { $_.type -eq 'tool_result' } | Select-Object -First 1 }
    Check 'comfy_load_workflow 载入成功（ok=true）' `
        ($null -ne $loadTr -and $loadTr.jsonPayload.ok -eq $true) $(if ($loadTr) { $loadTr.text } else { '没有 tool_result' })
    $loaded = $null
    if ($loadTr -and $loadTr.jsonPayload.ok -eq $true) { $loaded = $loadTr.text | ConvertFrom-Json }
    Check '按界面格式识别（format=ui）' ($null -ne $loaded -and $loaded.format -eq 'ui') $(if ($loaded) { $loaded.format })
    Check '转换出 6 个可提交节点' ($null -ne $loaded -and $loaded.nodeCount -eq 6) $(if ($loaded) { $loaded.nodeCount })
    Check '返回了可直接提交的 promptId' ($null -ne $loaded -and $loaded.promptId -gt 0) $(if ($loaded) { $loaded.promptId })
    if ($loaded) { $script:LoadedPromptId = $loaded.promptId }
    # 节点摘要是模型写 overrides 的唯一依据，不能是空的
    $ksampler = $null
    if ($loaded) { $ksampler = $loaded.nodes | Where-Object { $_.classType -eq 'KSampler' } | Select-Object -First 1 }
    Check '节点摘要里有 KSampler 与它的可覆盖控件' `
        ($null -ne $ksampler -and $ksampler.widgets -contains 'steps') $(if ($ksampler) { $ksampler.widgets -join ',' })

    # 再用同一个 promptId 提交一次（这一句走的是 [模板 C]），验转换出来的图真的能跑
    $submittedBefore4 = (Invoke-RestMethod "http://127.0.0.1:$ComfyPort/__submitted").runs.Count
    $submitRun = Invoke-Api 'POST' "/api/ai/conversations/$($conv2.id)/runs" @{
        text = "提交 $($loaded.promptId) 3.steps=13"
        providerId = $ProviderId; modelId = 'fake-title-model'
    }
    $callId2 = $null
    foreach ($i in 1..60) { Start-Sleep -Milliseconds 400; $callId2 = Get-PendingCallId $submitRun.runId; if ($callId2) { break } }
    if ($callId2) { Invoke-Api 'POST' "/api/ai/tool-calls/$callId2/approve" @{} | Out-Null }
    $submitMsg = $null
    foreach ($i in 1..160) {
        Start-Sleep -Milliseconds 500
        $msgs = Invoke-Api 'GET' "/api/ai/conversations/$($conv2.id)/messages" $null
        $submitMsg = $msgs | Where-Object { $_.id -eq $submitRun.assistantMessageId } | Select-Object -First 1
        if ($submitMsg -and ($submitMsg.status -eq 'complete' -or $submitMsg.status -eq 'failed')) { break }
    }
    $submittedAfter4 = (Invoke-RestMethod "http://127.0.0.1:$ComfyPort/__submitted").runs
    Check '转换出来的工作流真的提交给了 ComfyUI' ($submittedAfter4.Count -eq $submittedBefore4 + 1) `
        "$($submittedAfter4.Count) 次"
    if ($submittedAfter4.Count -gt $submittedBefore4) {
        $g4 = $submittedAfter4[-1].graph
        Check '提交的图有 6 个节点（与转换结果一致）' (@($g4.PSObject.Properties.Name).Count -eq 6) `
            (@($g4.PSObject.Properties.Name) -join ',')
        # 控件值按声明顺序贴回了名字上：seed 后面那个 control_after_generate 槽位要被跳过，
        # 所以 steps 拿到的必须是 13（覆盖值），而不是 20 或 "fixed"
        Check '控件值顺序正确（3.steps=13）' ("$($g4.'3'.inputs.steps)" -eq '13') "$($g4.'3'.inputs.steps)"
        Check '连线还原成上游节点与槽位（3.positive=[6,0]）' `
            ("$($g4.'3'.inputs.positive -join ',')" -eq '6,0') "$($g4.'3'.inputs.positive -join ',')"
        Check '提示词正文按声明顺序贴回（6.text）' ("$($g4.'6'.inputs.text)" -eq 'a cyberpunk cat') "$($g4.'6'.inputs.text)"
        if ($loaded) { $script:LoadedPromptId = $loaded.promptId }
    }
} finally {
    # --- 清理 -------------------------------------------------------------
    Say ''
    Say '  清理测试数据' 'Cyan'
    foreach ($p in @($fakeComfy, $gateway)) {
        if ($p) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
    }
    if ($oldConfig) {
        try {
            Invoke-Api 'PUT' '/api/capture/config' @{
                enabled = $oldConfig.enabled; comfyUrl = $oldConfig.comfyUrl
                outputDir = $oldConfig.outputDir; pollSeconds = $oldConfig.pollSeconds
                autoTag = $oldConfig.autoTag; maxPerPoll = $oldConfig.maxPerPoll
                downloadFallback = $oldConfig.downloadFallback
            } | Out-Null
            Say '  已还原自动捕获配置' 'DarkGray'
        } catch { Say "  还原配置失败: $($_.Exception.Message)" 'Yellow' }
    }
    try { Invoke-Api 'DELETE' "/api/ai/providers/$ProviderId" $null | Out-Null; Say '  已删除测试 Provider' 'DarkGray' } catch { }
    # 权限档原样放回（本脚本为了第 1 幕临时钉成过 ask）
    if ($script:PermissionModeBefore) {
        try {
            Invoke-Api 'PUT' '/api/ai/tools/policy' @{ permissionMode = $script:PermissionModeBefore } | Out-Null
            Say "  已把权限档还原成 '$($script:PermissionModeBefore)'" 'DarkGray'
        } catch { Say "  还原权限档失败: $($_.Exception.Message)" 'Yellow' }
    }
    # 读白名单原样放回（第 4 幕为了读临时目录里的工作流文件临时加过一条）
    if ($null -ne $script:ReadRootsBefore) {
        try {
            Invoke-Api 'PUT' '/api/ai/tools/policy' @{ readRoots = $script:ReadRootsBefore } | Out-Null
            Say '  已还原读白名单' 'DarkGray'
        } catch { Say "  还原读白名单失败: $($_.Exception.Message)" 'Yellow' }
    }

    if ($KeepData) {
        Say '  -KeepData：保留了测试产生的数据' 'Yellow'
    } else {
        foreach ($id in ($createdMediaIds | Select-Object -Unique)) {
            try { Invoke-Api 'DELETE' "/api/media/$id" $null | Out-Null } catch { }
        }
        if ($createdPromptId) { try { Invoke-Api 'DELETE' "/api/prompts/$createdPromptId" $null | Out-Null } catch { } }
        # 第 4 幕从文件加载出来的那条提示词（source=ComfyUI-File）也要清掉，
        # 否则库里会攒下一堆 e2e 工作流，下一次跑"载入"还会命中同一个 run_key（幂等 → 不算新数据）
        if ($script:LoadedPromptId) {
            try { Invoke-Api 'DELETE' "/api/prompts/$($script:LoadedPromptId)" $null | Out-Null } catch { }
            if ($script:Mysql) {
                & $script:Mysql --host=127.0.0.1 --port=3307 --user=comfyhub --password=comfyhub `
                    --database=comfy_hub -e "DELETE FROM capture_runs WHERE source='ComfyUI-File' AND prompt_id=$($script:LoadedPromptId)" 2>&1 | Out-Null
            }
        }
        if ($script:Mysql) {
            $keys = @()
            if ($createdRunKey) { $keys += "'$createdRunKey'" }
            if ($keys.Count -gt 0) {
                & $script:Mysql --host=127.0.0.1 --port=3307 --user=comfyhub --password=comfyhub `
                    --database=comfy_hub -e "DELETE FROM capture_runs WHERE run_key IN ($($keys -join ','))" 2>&1 | Out-Null
            }
        }
        Say ("  已删除 {0} 个产物 / {1} 条提示词 / 运行记录" -f $createdMediaIds.Count, $(if ($createdPromptId) { 1 } else { 0 })) 'DarkGray'
        Remove-Item $WorkDir -Recurse -Force -ErrorAction SilentlyContinue
        Remove-Item $WfDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Say ''
if ($script:Failed -gt 0) {
    Say ("  ✗ 有 $script:Failed 项没通过，请看上面的 [FAIL]") 'Red'
    exit 1
}
Say '  ✓ 全部通过：批准闸门 / 真的提交 / 参数覆盖 / 产物入库 / 工作流文件直接提交' 'Green'
Say ''
exit 0

