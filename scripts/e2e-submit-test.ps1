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

# 断言计数：脚本里任何一处意外抛错都会让后面的 Check **静默不执行**、最后却报"全部通过"。
# 打印出来，数目对不上就是有断言被跳过了。
$script:Checks = 0

function Say([string]$msg, [string]$color = 'Gray') { Write-Host $msg -ForegroundColor $color }

function Check([string]$name, $ok, [string]$detail = '') {
    # `$ok` 故意不强类型（`[bool]`）：`-match` / `-contains` 这类表达式可能返回**数组**，
    # 强类型参数会当场抛绑定错误 —— 而那个错误会让后面所有断言**静默跳过**、脚本还报"全部通过"
    # （这个坑真踩过）。这里统一按"非空即真"处理。
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

# 测试自己造出来的数据：假 ComfyUI 的 prompt_id 一律长成 `e2e-submit-0001`，
# 于是被自动捕获轮询收进来的那些提示词（source_ref）与运行记录（run_key）都带这个前缀。
#
# 开头也要清一次（不只是结尾）：`CaptureRepo.beginRun` 对 `success` 的记录永远不再抢占
# —— 那是幂等的正确行为，但脚本被 Ctrl+C 打断时清理跑不到，残留就会攒下来。
function Clear-SubmitTestCaptures {
    if (-not $script:Mysql) { return 0 }
    $done = 0
    $ids = & $script:Mysql --host=127.0.0.1 --port=3307 --user=comfyhub --password=comfyhub `
        --database=comfy_hub -N -B -e "SELECT GROUP_CONCAT(id) FROM prompts WHERE source_ref LIKE 'e2e-submit-%';" 2>$null
    if ($ids -and "$ids" -ne 'NULL') {
        foreach ($id in ("$ids" -split ',')) {
            if (-not $id) { continue }
            try { Invoke-Api 'DELETE' "/api/prompts/$id" $null | Out-Null; $done++ } catch { }
        }
    }
    & $script:Mysql --host=127.0.0.1 --port=3307 --user=comfyhub --password=comfyhub `
        --database=comfy_hub -e "DELETE FROM capture_runs WHERE run_key LIKE 'e2e-submit-%';" 2>&1 | Out-Null
    return $done
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
# 第 5 幕从（另一份）文件加载出来的提示词
$script:GapPromptId = $null

try {
    $health = Invoke-Api 'GET' '/api/health' $null
    Check '后端可用（db=ok）' ($health.database -eq 'ok')
    if ($script:Failed -gt 0) { exit 1 }

    # 上一轮遗留的测试数据先清掉（见 Clear-SubmitTestCaptures 的说明）
    $stale = Clear-SubmitTestCaptures
    if ($stale -gt 0) { Say "  已清掉上一轮遗留的 $stale 条测试提示词与运行记录" 'DarkGray' }

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
    Say '  [1/5] comfy_submit 默认要用户批准' 'Cyan'
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
    Say '  [2/5] 批准之后才真的提交并入库' 'Cyan'
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
    Say '  [3/5] 参数覆盖与提交内容' 'Cyan'
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
    Say '  [4/5] 工作流文件直接提交（界面格式 → API 节点图）' 'Cyan'

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
    # 这一跑出来的产物也要登记进清理清单（不然画廊里会攒下 e2e 的图）
    $submitTr4 = $null
    if ($submitMsg) {
        $submitTr4 = $submitMsg.parts | Where-Object { $_.jsonPayload.name -eq 'comfy_submit' } | Select-Object -First 1
    }
    if ($submitTr4) { $createdMediaIds += @($submitTr4.jsonPayload.mediaIds) }
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

    # --- 7. 前端节点转不出来时：模型拿缺口清单 → 补线 → 提交（用户建议 ②）----
    #
    # 现场：krea2 那份工作流里有一堆纯前端节点（Anything Everywhere 之类），
    # 服务端不敢猜着改写，于是整件事卡在"这条工作流转不了"。
    # 现在的处理是**放权给 AI**：带 tolerateUnsupported=true 读进来，拿到
    # `openInputs`（哪些连线型输入空着、缺什么类型）与 `unresolvedInputs`，
    # 由模型用 comfy_submit(connections=…) 把线补上再跑 —— 补的是"哪根线接哪"，
    # 猜的那部分仍然由 ComfyUI 自己的校验兜底。
    Say ''
    Say '  [5/5] 前端节点转不出来：模型按缺口清单补线再提交（用户建议 ②）' 'Cyan'

    $gapPath = Join-Path $WfDir 'e2e_ui_gap_workflow.json'
    $gapWorkflow = @'
{
  "last_node_id": 9,
  "last_link_id": 10,
  "nodes": [
    { "id": 1, "type": "CheckpointLoaderSimple", "mode": 0,
      "widgets_values": ["e2e_model.safetensors"], "inputs": [] },
    { "id": 2, "type": "Anything Everywhere", "mode": 0,
      "inputs": [{ "name": "anything", "type": "VAE", "link": 9 }], "outputs": [] },
    { "id": 6, "type": "CLIPTextEncode", "mode": 0,
      "widgets_values": ["a cyberpunk cat"], "inputs": [{ "name": "clip", "type": "CLIP", "link": 1 }] },
    { "id": 7, "type": "CLIPTextEncode", "mode": 0,
      "widgets_values": ["lowres"], "inputs": [{ "name": "clip", "type": "CLIP", "link": 2 }] },
    { "id": 5, "type": "EmptyLatentImage", "mode": 0, "widgets_values": [640, 960, 1], "inputs": [] },
    { "id": 3, "type": "KSampler", "mode": 0,
      "widgets_values": [777002, "fixed", 20, 7.0, "euler", "normal", 1.0],
      "inputs": [
        { "name": "model", "type": "MODEL", "link": 3 },
        { "name": "positive", "type": "CONDITIONING", "link": 4 },
        { "name": "negative", "type": "CONDITIONING", "link": 5 },
        { "name": "latent_image", "type": "LATENT", "link": 6 }
      ] },
    { "id": 8, "type": "VAEDecode", "mode": 0,
      "inputs": [
        { "name": "samples", "type": "LATENT", "link": 8 },
        { "name": "vae", "type": "VAE", "link": null }
      ] },
    { "id": 9, "type": "SaveImage", "mode": 0,
      "widgets_values": ["e2e/ui_gap"], "inputs": [{ "name": "images", "type": "IMAGE", "link": 10 }] }
  ],
  "links": [
    [1, 1, 1, 6, 0, "CLIP"],
    [2, 1, 1, 7, 0, "CLIP"],
    [3, 1, 0, 3, 0, "MODEL"],
    [4, 6, 0, 3, 1, "CONDITIONING"],
    [5, 7, 0, 3, 2, "CONDITIONING"],
    [6, 5, 0, 3, 3, "LATENT"],
    [8, 3, 0, 8, 0, "LATENT"],
    [9, 1, 2, 2, 0, "VAE"],
    [10, 8, 0, 9, 0, "IMAGE"]
  ]
}
'@
    Set-Content -Path $gapPath -Value $gapWorkflow -Encoding utf8

    $submittedBefore5 = (Invoke-RestMethod "http://127.0.0.1:$ComfyPort/__submitted").runs.Count
    $conv3 = Invoke-Api 'POST' '/api/ai/conversations' @{ providerId = $ProviderId; modelId = 'fake-title-model' }
    # VAEDecode 的 vae 该接节点 1 的 2 号输出（CheckpointLoaderSimple 的 VAE）——
    # 这条线正是被摘掉的 `Anything Everywhere` 原来广播进去的
    $gapRun = Invoke-Api 'POST' "/api/ai/conversations/$($conv3.id)/runs" @{
        text = "补线 $gapPath 8.vae=1,2"
        providerId = $ProviderId; modelId = 'fake-title-model'
    }
    $gapMsg = $null
    foreach ($i in 1..200) {
        Start-Sleep -Milliseconds 400
        # 第二轮那个 comfy_submit 是要批准的（本脚本此刻把权限档钉在 ask）
        $cid = Get-PendingCallId $gapRun.runId
        if ($cid) { Invoke-Api 'POST' "/api/ai/tool-calls/$cid/approve" @{} | Out-Null }
        $msgs = Invoke-Api 'GET' "/api/ai/conversations/$($conv3.id)/messages" $null
        $gapMsg = $msgs | Where-Object { $_.id -eq $gapRun.assistantMessageId } | Select-Object -First 1
        if ($gapMsg -and ($gapMsg.status -eq 'complete' -or $gapMsg.status -eq 'failed')) { break }
    }
    $gapParts = @()
    if ($gapMsg) { $gapParts = @($gapMsg.parts | Where-Object { $_.type -eq 'tool_result' }) }
    $gapLoad = $gapParts | Where-Object { $_.jsonPayload.name -eq 'comfy_load_workflow' } | Select-Object -First 1
    Check '转不出来的那份也能读进来（ok=true）' ($null -ne $gapLoad -and $gapLoad.jsonPayload.ok -eq $true) `
        $(if ($gapLoad) { $gapLoad.text } else { '没有 comfy_load_workflow 的结果' })
    $gapLoaded = $null
    if ($gapLoad -and $gapLoad.jsonPayload.ok -eq $true) { $gapLoaded = $gapLoad.text | ConvertFrom-Json }
    Check '如实标成 runnable=false（缺口没补之前不能跑）' `
        ($null -ne $gapLoaded -and $gapLoaded.runnable -eq $false) $(if ($gapLoaded) { $gapLoaded.runnable })
    Check '点名摘掉了哪个前端节点（Anything Everywhere）' `
        ($null -ne $gapLoaded -and @($gapLoaded.unsupportedNodes) -match 'Anything Everywhere') `
        $(if ($gapLoaded) { @($gapLoaded.unsupportedNodes) -join ';' })
    Check '列出空着的连线型输入（8.vae 该补什么一目了然）' `
        ($null -ne $gapLoaded -and @($gapLoaded.openInputs) -contains '8.vae (VAE)') `
        $(if ($gapLoaded) { @($gapLoaded.openInputs) -join ';' })

    $gapSubmit = $gapParts | Where-Object { $_.jsonPayload.name -eq 'comfy_submit' } | Select-Object -First 1
    Check '补完线之后提交成功（ok=true）' ($null -ne $gapSubmit -and $gapSubmit.jsonPayload.ok -eq $true) `
        $(if ($gapSubmit) { $gapSubmit.text } else { '没有 comfy_submit 的结果' })
    if ($gapSubmit -and $gapSubmit.jsonPayload.ok -eq $true) {
        $gapOut = $gapSubmit.text | ConvertFrom-Json
        $script:GapPromptId = $gapOut.promptId
        $createdMediaIds += @($gapSubmit.jsonPayload.mediaIds)
        $appliedGap = @($gapOut.appliedOverrides) | Where-Object { $_ -match '8\.vae' }
        Check '补的那根线如实记进了 appliedOverrides' ($appliedGap.Count -gt 0) `
            (@($gapOut.appliedOverrides) -join ';')
    }
    $submittedAfter5 = (Invoke-RestMethod "http://127.0.0.1:$ComfyPort/__submitted").runs
    Check '补线的图真的提交给了 ComfyUI' ($submittedAfter5.Count -eq $submittedBefore5 + 1) `
        "$($submittedAfter5.Count) 次"
    if ($submittedAfter5.Count -gt $submittedBefore5) {
        $g5 = $submittedAfter5[-1].graph
        Check '提交的图里补上了 vae 连线（8.vae=[1,2]）' `
            ("$($g5.'8'.inputs.vae -join ',')" -eq '1,2') "$($g5.'8'.inputs.vae -join ',')"
        Check '被摘掉的前端节点没有混进提交的图' `
            ($null -eq $g5.PSObject.Properties['2']) (@($g5.PSObject.Properties.Name) -join ',')
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
        # 第 4 / 5 幕从文件加载出来的提示词（source=ComfyUI-File）也要清掉，
        # 否则库里会攒下一堆 e2e 工作流，下一次跑"载入"还会命中同一个 run_key（幂等 → 不算新数据）
        $filePromptIds = @($script:LoadedPromptId, $script:GapPromptId) | Where-Object { $_ }
        foreach ($fid in $filePromptIds) {
            try { Invoke-Api 'DELETE' "/api/prompts/$fid" $null | Out-Null } catch { }
        }
        # capture_runs 用**文件内容的 sha256** 精确定位（run_key = `file:<sha256>`）：
        # 只按 prompt_id 删会漏掉"提示词已经被删、prompt_id 置空"的那种残留记录。
        $fileRunKeys = @()
        foreach ($wf in @($uiWorkflowPath, $gapPath)) {
            if ($wf -and (Test-Path $wf)) {
                $sha = (Get-FileHash -Algorithm SHA256 -Path $wf).Hash.ToLower()
                $fileRunKeys += "'file:$sha'"
            }
        }
        if ($script:Mysql -and $fileRunKeys.Count -gt 0) {
            & $script:Mysql --host=127.0.0.1 --port=3307 --user=comfyhub --password=comfyhub `
                --database=comfy_hub -e "DELETE FROM capture_runs WHERE run_key IN ($($fileRunKeys -join ','))" 2>&1 | Out-Null
        }
        if ($script:Mysql) {
            $keys = @()
            if ($createdRunKey) { $keys += "'$createdRunKey'" }
            if ($keys.Count -gt 0) {
                & $script:Mysql --host=127.0.0.1 --port=3307 --user=comfyhub --password=comfyhub `
                    --database=comfy_hub -e "DELETE FROM capture_runs WHERE run_key IN ($($keys -join ','))" 2>&1 | Out-Null
            }
        }
        Say ("  已删除 {0} 个产物 / {1} 条提示词 / 运行记录" -f `
                ($createdMediaIds | Select-Object -Unique).Count, (1 + $filePromptIds.Count)) 'DarkGray'
        # 假 ComfyUI 的 prompt_id 造的提示词与运行记录（被自动捕获轮询收进来的那些）
        $stale = Clear-SubmitTestCaptures
        if ($stale -gt 0) { Say "  已清掉 $stale 条捕获进来的测试提示词与运行记录" 'DarkGray' }
        Remove-Item $WorkDir -Recurse -Force -ErrorAction SilentlyContinue
        Remove-Item $WfDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Say ''
if ($script:Failed -gt 0) {
    Say ("  ✗ 有 $script:Failed 项没通过，请看上面的 [FAIL]") 'Red'
    exit 1
}
Say '  ✓ 全部通过：批准闸门 / 真的提交 / 参数覆盖 / 产物入库 / 工作流文件直接提交 / 前端节点缺口补线' 'Green'
Say ("  （{0} 项检查）" -f $script:Checks) 'DarkGray'
Say ''
exit 0

