<#
.SYNOPSIS
    ComfyHub「AI 工具循环（M4）+ Skills（M5）」端到端自测。

.DESCRIPTION
    用 scripts\e2e\fake_openai.py 假装成一个 OpenAI 兼容的流式网关（**不需要真实 API Key、不出网**），
    把整条工具链路走一遍：

      1. 注册    —— register_skill 落盘 + GET /api/ai/skills + SKILL.md 真的写出来了
      2. 按需加载 —— load_skill 把正文喂给模型，**工具结果确实被回了上游**（第二轮带 role=tool）
      3. 越界写被拒 —— write_file 写 storage/ 被 PATH_DENIED 拦下，文件不存在
      4. 目录内写成功 —— write_file 写 comfyui/ 成功，内容正确
      5. 审批    —— comfy_sync_history 发 tool.requested(approval=pending)，批准后才 tool.started
      6. 只读工具 —— comfy_get_status 直接执行（不需要审批）
      7. 长期记忆 —— remember 落盘 + 下一次 Run 注入系统提示
      8. 附件（M3）—— 上传 → 缩略图 → 内联图片进请求体 → 纯文本模型零上游请求
         （含 **WebP**：按签名收下 + 尺寸探测 + 缩略图真的是 JPEG）

    还会核对落库的消息 parts 顺序（tool_call / tool_result / text），保证重开会话能渲染工具卡。

    脚本会**临时**改两处配置，跑完在 finally 里原样还原：
      · 权限档切成 `ask`（不然用户在界面上选了「自动允许」时，第 5 幕的审批断言必红）；
      · 长期记忆写一条测试记忆（结束时放回原文）。

    前置条件：MySQL + 后端已经在跑，且后端是**带 M4/M5 的新构建**：
        pwsh -File scripts\comfyhub.ps1 up
        pwsh -File scripts\server.ps1 stop
        pwsh -File scripts\server.ps1 start

.EXAMPLE
    pwsh -File scripts\e2e-ai-tools-test.ps1
    pwsh -File scripts\e2e-ai-tools-test.ps1 -KeepData
#>
[CmdletBinding()]
param(
    [string]$ApiBase = 'http://127.0.0.1:8080',
    [int]$GatewayPort = 8799,
    [int]$TimeoutSec = 60,
    [switch]$KeepData
)

$ErrorActionPreference = 'Continue'
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$E2eDir      = Join-Path $PSScriptRoot 'e2e'
$GatewayBase = "http://127.0.0.1:$GatewayPort"

$ProviderId  = 'e2e-tools'
$ModelId     = 'fake-tools-model'
$SkillName   = 'e2e-tool-demo'
$ConversationTitle = 'e2e-ai-tools'

$SkillFile   = Join-Path $ProjectRoot "storage\ai\skills\$SkillName\SKILL.md"
$HackFile    = Join-Path $ProjectRoot 'storage\e2e-hack.txt'
$OkFile      = Join-Path $ProjectRoot 'comfyui\e2e-ok.txt'
$ExpectedWriteRoot = Join-Path $ProjectRoot 'comfyui'

$script:Failed = 0
$script:Total  = 0
$script:Http   = $null
$script:Streams = @{}
$script:ConversationId = $null

# 长期记忆的原内容：本脚本会往里写一条测试记忆，结束时原样放回
$script:MemoryBefore = $null

# 权限档的原值：审批那一幕要求 `comfy_sync_history` 是 pending，而用户在界面上把档位切到
# 「自动允许（无需批准）」时它会是 `not_required`（第 5 幕会红）。所以脚本先临时钉成 ask、结束时原样放回。
$script:PermissionModeBefore = $null

# 附件（M3）：测试用的一张 1×1 PNG / 一张 64×64 WebP 与上传后的附件 id
$script:AttachPng = Join-Path ([System.IO.Path]::GetTempPath()) 'comfyhub-e2e-attach.png'
$script:AttachWebp = Join-Path ([System.IO.Path]::GetTempPath()) 'comfyhub-e2e-attach.webp'
$script:AttachmentId = $null
$script:WebpAttachmentId = $null

function Say([string]$msg, [string]$color = 'Gray') { Write-Host $msg -ForegroundColor $color }

function Check([string]$name, [bool]$ok, [string]$detail = '') {
    $script:Total++
    if ($ok) {
        Say ("  [OK]   " + $name) 'Green'
    } else {
        Say ("  [FAIL] " + $name + $(if ($detail) { "  -> $detail" } else { '' })) 'Red'
        $script:Failed++
    }
}

# PS7 的 Invoke-RestMethod **不会枚举** JSON 数组（整块作为一个对象返回），
# 直接 `@(...)` 只会得到「一个里面装着数组的元素」——这里显式摊平，
# 保证调用方 `@(Invoke-Api ...)` 拿到的是逐项的元素。
function Expand-Result($value) {
    if ($null -eq $value) { return }
    if ($value -is [System.Array]) {
        foreach ($item in $value) { Write-Output $item }
        return
    }
    Write-Output $value
}

function Invoke-Api {
    param([string]$Method, [string]$Path, $Body)
    $uri = "$ApiBase$Path"
    if ($null -eq $Body) {
        Expand-Result (Invoke-RestMethod -Method $Method -Uri $uri -TimeoutSec 120)
        return
    }
    $json = $Body | ConvertTo-Json -Depth 12 -Compress
    Expand-Result (Invoke-RestMethod -Method $Method -Uri $uri -TimeoutSec 180 `
        -ContentType 'application/json; charset=utf-8' -Body $json)
}

function Invoke-Gateway {
    param([string]$Path)
    Expand-Result (Invoke-RestMethod -Method 'GET' -Uri "$GatewayBase$Path" -TimeoutSec 30)
}

function Find-Python {
    $cands = @($env:COMFYHUB_PYTHON)
    $cmd = Get-Command python -ErrorAction SilentlyContinue
    if ($cmd) { $cands += $cmd.Source }
    $cands += @(
        'D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\standalone-env\python.exe',
        'D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI\.venv\Scripts\python.exe'
    )
    foreach ($c in $cands) {
        if ($c -and (Test-Path $c)) { return $c }
    }
    return $null
}

# --- 假网关进程（静默启动 / 按命令行精确回收） ------------------------------

function Get-GatewayPids {
    # 只认 python 进程：命令行里出现 fake_openai.py 的 pwsh（也就是我们自己）绝不能被杀
    @(Get-CimInstance Win32_Process -Filter "Name LIKE 'python%'" -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -and $_.CommandLine -like '*fake_openai.py*' } |
        Select-Object -ExpandProperty ProcessId)
}

function Stop-Gateway {
    foreach ($procId in (Get-GatewayPids)) {
        Stop-Process -Id $procId -Force -ErrorAction SilentlyContinue
    }
}

# --- SSE（后端统一事件流，不是供应商流） ------------------------------------

function Open-RunStream {
    param([string]$RunId)
    $uri = "$ApiBase/api/ai/runs/$RunId/events"
    $req = [System.Net.Http.HttpRequestMessage]::new([System.Net.Http.HttpMethod]::Get, $uri)
    $req.Headers.Accept.ParseAdd('text/event-stream')
    $resp = $script:Http.Send($req, [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead)
    if (-not $resp.IsSuccessStatusCode) {
        $code = [int]$resp.StatusCode
        $resp.Dispose(); $req.Dispose()
        throw "SSE 订阅失败：HTTP $code"
    }
    $reader = [System.IO.StreamReader]::new($resp.Content.ReadAsStream(), [System.Text.Encoding]::UTF8)
    $script:Streams[$RunId] = @{ resp = $resp; reader = $reader }
    $req.Dispose()
}

function Close-RunStream {
    param([string]$RunId)
    $s = $script:Streams[$RunId]
    if (-not $s) { return }
    try { $s.reader.Dispose() } catch { }
    try { $s.resp.Dispose() } catch { }
    $script:Streams.Remove($RunId)
}

function Read-RunEvent {
    <# 读一个完整 SSE 事件；返回 $null 表示流结束或超时。
       后端每 15s 会发一次 heartbeat，所以阻塞读不会真的卡死。 #>
    param([string]$RunId, [datetime]$Deadline)
    $reader = $script:Streams[$RunId].reader
    $ev = $null
    $data = New-Object System.Collections.Generic.List[string]
    while ((Get-Date) -lt $Deadline) {
        $line = $reader.ReadLine()
        if ($null -eq $line) { return $null }
        if ($line.Length -eq 0) {
            if ($null -ne $ev -or $data.Count -gt 0) {
                return @{ event = $ev; data = ($data -join "`n") }
            }
            continue
        }
        if ($line.StartsWith('event:')) { $ev = $line.Substring(6).Trim() }
        elseif ($line.StartsWith('data:')) { $data.Add($line.Substring(5).TrimStart()) }
    }
    return $null
}

function Receive-RunEvents {
    param([string]$RunId, [int]$TimeoutSec = 60, [scriptblock]$OnEvent)
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    $list = New-Object System.Collections.Generic.List[hashtable]
    while ($true) {
        $e = Read-RunEvent -RunId $RunId -Deadline $deadline
        if ($null -eq $e) { break }
        $list.Add($e)
        if ($OnEvent) { & $OnEvent $e ($list.Count - 1) | Out-Null }
        if ($e.event -in @('run.completed', 'run.failed', 'run.cancelled')) { break }
    }
    Expand-Result $list.ToArray()
}

function Get-Events {
    param($Events, [string]$Type)
    Expand-Result @($Events | Where-Object { $_.event -eq $Type })
}

function Convert-Data($Event) { return ($Event.data | ConvertFrom-Json) }

function Start-AiRun {
    param([string]$ConversationId, [string]$Text, [string[]]$AttachmentIds)
    # 故意**不带** reasoningEffort：DTO 与文档都写着"缺省 = off"，而前端在"模型不支持推理"时
    # 本来就不传这个字段。早先后端对 null 直接抛错（"未知的思考强度：null"），
    # 于是所有不支持推理的模型都发不出消息 —— 这条路径就是那个 bug 的回归防线。
    $body = @{ text = $Text; providerId = $ProviderId; modelId = $ModelId }
    if ($AttachmentIds -and $AttachmentIds.Count -gt 0) { $body.attachmentIds = @($AttachmentIds) }
    return Invoke-Api 'POST' "/api/ai/conversations/$ConversationId/runs" $body
}

function Receive-Scenario {
    param([string]$ConversationId, [string]$Text, [string[]]$AttachmentIds, [scriptblock]$OnEvent)
    $run = Start-AiRun -ConversationId $ConversationId -Text $Text -AttachmentIds $AttachmentIds
    Open-RunStream -RunId $run.runId
    $events = @(Receive-RunEvents -RunId $run.runId -TimeoutSec $TimeoutSec -OnEvent $OnEvent)
    Close-RunStream -RunId $run.runId
    return @{ run = $run; events = $events }
}

# ---------------------------------------------------------------------------

Say ''
Say '  ComfyHub AI 工具循环 + Skills · 端到端自测（假网关，不出网）' 'White'
Say '  ────────────────────────────────────────────────────────────'

# --- 0. 环境 ---------------------------------------------------------------
$python = Find-Python
if (-not $python) {
    Say '  找不到 python（可用环境变量 COMFYHUB_PYTHON 指定）。' 'Red'
    exit 1
}
Say "  python: $python" 'DarkGray'

try {
    $health = Invoke-Api 'GET' '/api/health' $null
} catch {
    $health = $null
}
if (-not $health -or $health.database -ne 'ok') {
    Say '' 
    Say '  后端没在跑（GET /api/health 不可用）。请先：' 'Red'
    Say '    pwsh -File scripts\comfyhub.ps1 up' 'Yellow'
    Say '    pwsh -File scripts\server.ps1 stop' 'Yellow'
    Say '    pwsh -File scripts\server.ps1 start' 'Yellow'
    exit 1
}
Say ("  后端: {0}  v{1}  db={2}" -f $ApiBase, $health.version, $health.database) 'DarkGray'

$gatewayStarted = $false

# SSE 用流式读；整体超时给足（Run 本身可能跑几十秒）
$script:Http = [System.Net.Http.HttpClient]::new()
$script:Http.Timeout = [TimeSpan]::FromSeconds(300)

try {
    # --- 1. 启动假网关（静默，绝不弹窗；finally 里必杀） -------------------
    Stop-Gateway   # 清掉上一次残留
    . "$PSScriptRoot\silent-process.ps1"
    $cmdLine = "`"$python`" `"$(Join-Path $E2eDir 'fake_openai.py')`" --port $GatewayPort"
    $way = Start-SilentProcess -CommandLine $cmdLine -WorkingDirectory $ProjectRoot -Tag 'fake-openai'
    $gatewayStarted = $true
    Say "  假网关启动方式: $way" 'DarkGray'

    $ready = $false
    foreach ($i in 1..40) {
        try {
            $models = Invoke-Gateway '/v1/models'
            if ($models.data[0].id -eq $ModelId) { $ready = $true; break }
        } catch { Start-Sleep -Milliseconds 300 }
    }
    Check "假网关已就绪（$GatewayBase/v1）" $ready

    # --- 2. Provider + 模型目录 -------------------------------------------
    Say ''
    Say '  [0/8] 配置 e2e Provider / 模型' 'Cyan'

    try { Invoke-Api 'DELETE' "/api/ai/providers/$ProviderId" $null | Out-Null } catch { }
    try { Invoke-Api 'DELETE' "/api/ai/skills/$SkillName" $null | Out-Null } catch { }
    # 上一次 -KeepData / 失败留下的会话也一并清掉，保证脚本可重复跑
    try {
        foreach ($old in @(Invoke-Api 'GET' '/api/ai/conversations?includeArchived=true' $null)) {
            if ($old.title -eq $ConversationTitle) {
                Invoke-Api 'DELETE' "/api/ai/conversations/$($old.id)" $null | Out-Null
            }
        }
    } catch { }
    Remove-Item $SkillFile -Force -ErrorAction SilentlyContinue
    Remove-Item $HackFile -Force -ErrorAction SilentlyContinue
    Remove-Item $OkFile -Force -ErrorAction SilentlyContinue
    # 长期记忆先存一份：后面会往里面写测试数据，结束时原样放回（不能污染用户真实的记忆）
    $script:MemoryBefore = (Invoke-Api 'GET' '/api/ai/memory' $null).content
    # 权限档也存一份并临时钉成 ask：审批那一幕只有在"需要批准"的档位下才有意义
    $script:PermissionModeBefore = (Invoke-Api 'GET' '/api/ai/tools/policy' $null).permissionMode
    if ($script:PermissionModeBefore -ne 'ask') {
        Invoke-Api 'PUT' '/api/ai/tools/policy' @{ permissionMode = 'ask' } | Out-Null
        Say "  权限档临时从 '$($script:PermissionModeBefore)' 切成 'ask'（结束时还原）" 'DarkGray'
    }

    $provider = Invoke-Api 'POST' '/api/ai/providers' @{
        id = $ProviderId
        displayName = 'E2E Tools (fake)'
        api = 'openai-completions'
        baseURL = "$GatewayBase/v1"
        credentialRef = $null
        endpointTrust = 'loopback'
        enabled = $true
    }
    Check "Provider 已创建（$ProviderId, api=$($provider.api), trust=$($provider.endpointTrust)）" `
        ($provider.id -eq $ProviderId -and $provider.api -eq 'openai-completions') ($provider | ConvertTo-Json -Compress)

    $models = Invoke-Api 'PUT' "/api/ai/providers/$ProviderId/models" @{
        models = @(@{
            providerId = $ProviderId
            id = $ModelId
            displayName = 'Fake Tools Model'
            inputModalities = @('text')
            attachmentTransports = @{}
            mimeAllowlist = @()
            tools = $true
            parallelTools = $false
            reasoning = $false
            thinkingEfforts = @{}
            contextWindow = 32768
            maxOutputTokens = 4096
            capabilitySource = 'manual'
            enabled = $true
        })
    }
    $m = @($models) | Where-Object { $_.id -eq $ModelId } | Select-Object -First 1
    Check '模型已登记且 tools=true' ($null -ne $m -and $m.tools -eq $true) ($models | ConvertTo-Json -Compress)

    $conv = Invoke-Api 'POST' '/api/ai/conversations' @{ title = $ConversationTitle; providerId = $ProviderId; modelId = $ModelId }
    $script:ConversationId = $conv.id
    Check '会话已创建' (-not [string]::IsNullOrWhiteSpace($conv.id)) ($conv | ConvertTo-Json -Compress)

    # --- 3. 场景 1：注册 skill --------------------------------------------
    Say ''
    Say '  [1/9] 注册：register_skill 落盘' 'Cyan'
    $beforeLog = @(Invoke-Gateway '/__log').Count
    $s1 = Receive-Scenario -ConversationId $conv.id -Text '帮我注册一个新的 skill'
    $e1 = $s1.events
    $started = @(Get-Events $e1 'run.started')
    Check '收到 run.started' ($started.Count -ge 1)
    if ($started.Count -ge 1) {
        $p = Convert-Data $started[0]
        $toolKeys = @($p.tools.PSObject.Properties.Name)
        Check "run.started 带上了非空 tools（$($toolKeys.Count) 个工具）" ($toolKeys.Count -gt 0) ($p | ConvertTo-Json -Compress)
    }
    $req1 = @(Get-Events $e1 'tool.requested')
    $r1 = if ($req1.Count -ge 1) { Convert-Data $req1[0] } else { $null }
    Check 'tool.requested.name == register_skill' ($null -ne $r1 -and $r1.name -eq 'register_skill') ($req1 | ConvertTo-Json -Compress)
    Check 'register_skill 的 approval == not_required' ($null -ne $r1 -and $r1.approval -eq 'not_required') ($r1 | ConvertTo-Json -Compress)
    Check '收到 tool.started' ((@(Get-Events $e1 'tool.started')).Count -ge 1) ($e1 | ConvertTo-Json -Depth 6 -Compress)
    $mc1 = @(Get-Events $e1 'message.completed')
    Check '收到 message.completed' ($mc1.Count -ge 1) ($e1 | ConvertTo-Json -Depth 6 -Compress)
    if ($mc1.Count -ge 1) {
        $p = Convert-Data $mc1[0]
        $parts = @($p.parts)
        $types = @($parts | ForEach-Object { $_.type })
        Check 'message.completed.parts 非空' ($parts.Count -gt 0) ($p | ConvertTo-Json -Depth 6 -Compress)
    }
    Check '收到 run.completed' ((@(Get-Events $e1 'run.completed')).Count -ge 1) ($e1[-1].event)

    $skills = @(Invoke-Api 'GET' '/api/ai/skills' $null)
    $skill = $skills | Where-Object { $_.name -eq $SkillName } | Select-Object -First 1
    Check "GET /api/ai/skills 里有 $SkillName" ($null -ne $skill) (($skills | ForEach-Object { $_.name }) -join ',')
    if ($skill) {
        Check 'skill 来源是 user 且无校验错误' ($skill.source -eq 'user' -and -not $skill.validationError) ($skill | ConvertTo-Json -Compress)
    }
    Check "SKILL.md 已落盘（$SkillFile）" (Test-Path $SkillFile)
    if (Test-Path $SkillFile) {
        $head = (Get-Content $SkillFile -TotalCount 1 -Encoding UTF8)
        Check 'SKILL.md 以 --- 开头（带 frontmatter）' ($head -eq '---') $head
    }

    # --- 4. 场景 2：按需加载 ----------------------------------------------
    Say ''
    Say '  [2/9] 按需加载：load_skill + 工具结果回灌上游' 'Cyan'
    $beforeLog = @(Invoke-Gateway '/__log').Count
    $s2 = Receive-Scenario -ConversationId $conv.id -Text '加载那个 skill'
    $e2 = $s2.events
    $comp2 = @(Get-Events $e2 'tool.completed')
    $c2 = if ($comp2.Count -ge 1) { Convert-Data $comp2[0] } else { $null }
    Check 'load_skill 执行成功（tool.completed）' ($null -ne $c2 -and $c2.name -eq 'load_skill') ($comp2 | ConvertTo-Json -Compress)
    Check '结果里带 <skill_content' ($null -ne $c2 -and $c2.preview -like '*<skill_content*') ($c2 | ConvertTo-Json -Compress)

    $newLog = @(Invoke-Gateway '/__log')
    $runLog = @($newLog | Select-Object -Skip $beforeLog)
    Say ("    本次 Run 的上游请求 {0} 次：{1}" -f $runLog.Count, (($runLog | ForEach-Object { "[$($_.roles -join '/')] tools=$($_.toolCount)" }) -join ' | ')) 'DarkGray'
    Check '该 Run 至少发了 2 次上游请求（工具结果被喂回去了）' ($runLog.Count -ge 2) ($runLog | ConvertTo-Json -Depth 6 -Compress)
    $second = if ($runLog.Count -ge 2) { $runLog[1] } else { $null }
    Check '第二次请求最后一条是 role=tool' ($null -ne $second -and $second.lastRole -eq 'tool') ($second | ConvertTo-Json -Depth 6 -Compress)
    Check '第二次请求带上了 assistant 的 tool_calls 与 tool 结果' `
        ($null -ne $second -and $second.assistantHasToolCall -eq $true -and $second.toolResultCount -ge 1) ($second | ConvertTo-Json -Depth 6 -Compress)
    $mcp2 = @(Get-Events $e2 'message.completed')
    if ($mcp2.Count -ge 1) {
        $p = Convert-Data $mcp2[0]
        $tr = @($p.parts | Where-Object { $_.type -eq 'tool_result' }) | Select-Object -First 1
        Check '落库的 tool_result 正文里带 <skill_content' ($null -ne $tr -and $tr.text -like '*<skill_content*') ($tr.text)
    }

    # --- 5. 场景 3：越界写被拒 --------------------------------------------
    Say ''
    Say '  [3/9] 越界写被拒：write_file 打到 storage/' 'Cyan'
    $s3 = Receive-Scenario -ConversationId $conv.id -Text '越界写个文件'
    $e3 = $s3.events
    $fail3 = @(Get-Events $e3 'tool.failed')
    $f3 = if ($fail3.Count -ge 1) { Convert-Data $fail3[0] } else { $null }
    Check 'write_file 越界 -> tool.failed' ($null -ne $f3 -and $f3.name -eq 'write_file') ($fail3 | ConvertTo-Json -Compress)
    Check '错误码 == PATH_DENIED' ($null -ne $f3 -and $f3.code -eq 'PATH_DENIED') ($f3 | ConvertTo-Json -Compress)
    Check "storage\e2e-hack.txt 不存在" (-not (Test-Path $HackFile))

    $policy = Invoke-Api 'GET' '/api/ai/tools/policy' $null
    $roots = @($policy.writeRoots)
    Check "writeRoots 恰好是 $ExpectedWriteRoot" `
        ($roots.Count -eq 1 -and $roots[0].TrimEnd('\') -ieq $ExpectedWriteRoot.TrimEnd('\')) `
        ($roots -join ' | ')

    # --- 6. 场景 4：目录内写成功 ------------------------------------------
    Say ''
    Say '  [4/9] 目录内写成功：write_file 打到 comfyui/' 'Cyan'
    $s4 = Receive-Scenario -ConversationId $conv.id -Text '在目录内写个文件'
    $e4 = $s4.events
    $comp4 = @(Get-Events $e4 'tool.completed')
    $c4 = if ($comp4.Count -ge 1) { Convert-Data $comp4[0] } else { $null }
    Check 'write_file 在 comfyui/ 内成功' ($null -ne $c4 -and $c4.name -eq 'write_file') ($comp4 | ConvertTo-Json -Compress)
    Check "comfyui\e2e-ok.txt 已创建" (Test-Path $OkFile)
    if (Test-Path $OkFile) {
        $body = (Get-Content $OkFile -Raw -Encoding UTF8).Trim()
        Check '文件内容 == hello from tool' ($body -eq 'hello from tool') $body
    }

    # --- 7. 场景 5：审批闸门 ----------------------------------------------
    Say ''
    Say '  [5/9] 审批：comfy_sync_history 必须等批准才执行' 'Cyan'
    $state = @{ callId = $null; pending = $false; accepted = $false; startedIndex = -1; requestedIndex = -1 }
    $s5 = Receive-Scenario -ConversationId $conv.id -Text '同步一下历史' -OnEvent {
        param($e, $i)
        if ($e.event -eq 'tool.requested') {
            $p = $e.data | ConvertFrom-Json
            if ($p.name -eq 'comfy_sync_history') {
                $state.requestedIndex = $i
                $state.callId = $p.callId
                if ($p.approval -eq 'pending') { $state.pending = $true }
                # 立刻批准；open() 与 emit() 之间可能有极短竞争，accepted=false 就重试
                foreach ($try in 1..15) {
                    $res = Invoke-Api 'POST' "/api/ai/tool-calls/$($p.callId)/approve" $null
                    if ($res.accepted) { $state.accepted = $true; break }
                    Start-Sleep -Milliseconds 150
                }
            }
        }
        if ($e.event -eq 'tool.started' -and $state.startedIndex -lt 0) { $state.startedIndex = $i }
    }
    $e5 = $s5.events
    $req5 = @(Get-Events $e5 'tool.requested')
    $r5 = if ($req5.Count -ge 1) { Convert-Data $req5[0] } else { $null }
    Check 'comfy_sync_history 被请求且 approval == pending' ($null -ne $r5 -and $r5.name -eq 'comfy_sync_history' -and $r5.approval -eq 'pending') ($req5 | ConvertTo-Json -Compress)
    Check '批准被后端接受（accepted=true）' ($state.accepted -eq $true) ($state | ConvertTo-Json -Compress)
    Check '批准之前没有 tool.started' ($state.startedIndex -lt 0 -or $state.startedIndex -gt $state.requestedIndex) ("requested=$($state.requestedIndex) started=$($state.startedIndex)")
    Check '批准之后收到了 tool.started' ($state.startedIndex -gt $state.requestedIndex) ("requested=$($state.requestedIndex) started=$($state.startedIndex)")
    $done5 = @(Get-Events $e5 'tool.completed') + @(Get-Events $e5 'tool.failed')
    Check '审批后工具有了终态（completed/failed）' ($done5.Count -ge 1) ($e5 | ConvertTo-Json -Depth 6 -Compress)

    # --- 8. 场景 6：只读工具 ----------------------------------------------
    Say ''
    Say '  [6/9] 只读工具：comfy_get_status 不需要审批' 'Cyan'
    $s6 = Receive-Scenario -ConversationId $conv.id -Text '看看 ComfyUI 状态'
    $e6 = $s6.events
    $req6 = @(Get-Events $e6 'tool.requested')
    $r6 = if ($req6.Count -ge 1) { Convert-Data $req6[0] } else { $null }
    Check '工具是 comfy_get_status' ($null -ne $r6 -and $r6.name -eq 'comfy_get_status') ($req6 | ConvertTo-Json -Compress)
    Check '只读工具 approval != pending' ($null -ne $r6 -and $r6.approval -ne 'pending') ($r6 | ConvertTo-Json -Compress)
    $done6 = @(Get-Events $e6 'tool.completed') + @(Get-Events $e6 'tool.failed')
    $d6 = if ($done6.Count -ge 1) { Convert-Data $done6[0] } else { $null }
    Check 'comfy_get_status 有终态（completed/failed）' ($null -ne $d6 -and $d6.name -eq 'comfy_get_status') ($d6 | ConvertTo-Json -Compress)

    # --- 9. 落库的消息 parts（重开会话能渲染工具卡） ----------------------
    Say ''
    Say '  [7/9] 落库校验：GET /api/ai/runs/{id} + /conversations/{id}/messages' 'Cyan'
    $runDto = Invoke-Api 'GET' "/api/ai/runs/$($s1.run.runId)" $null
    Check 'run 落库状态 == completed' ($runDto.status -eq 'completed') ($runDto | ConvertTo-Json -Compress)

    $msgs = @(Invoke-Api 'GET' "/api/ai/conversations/$($conv.id)/messages" $null)
    $assistant = @($msgs | Where-Object { $_.id -eq $s1.run.assistantMessageId }) | Select-Object -First 1
    Check '助手消息已落库' ($null -ne $assistant)
    if ($assistant) {
        $types = @($assistant.parts | ForEach-Object { $_.type })
        Check '助手消息 parts 有序且含 tool_call/tool_result/text' `
            (($types -contains 'tool_call') -and ($types -contains 'tool_result') -and ($types -contains 'text')) `
            ($types -join ',')
        $iCall = [array]::IndexOf($types, 'tool_call')
        $iRes  = [array]::IndexOf($types, 'tool_result')
        $iText = [array]::IndexOf($types, 'text')
        Check "parts 顺序 tool_call($iCall) < tool_result($iRes) < text($iText)" `
            ($iCall -ge 0 -and $iRes -gt $iCall -and $iText -gt $iRes) ($types -join ',')
        $tc = $assistant.parts | Where-Object { $_.type -eq 'tool_call' } | Select-Object -First 1
        Check 'tool_call part 带 name 与 arguments' `
            ($null -ne $tc -and $tc.jsonPayload.name -eq 'register_skill' -and $tc.jsonPayload.arguments -like '*e2e-tool-demo*') `
            ($tc | ConvertTo-Json -Depth 6 -Compress)
        $tw = $assistant.parts | Where-Object { $_.type -eq 'tool_result' } | Select-Object -First 1
        Check 'tool_result part 带 ok=true' ($null -ne $tw -and $tw.jsonPayload.ok -eq $true) ($tw | ConvertTo-Json -Depth 6 -Compress)
    }

    # --- 10. 场景 8：长期记忆（M6） ---------------------------------------
    Say ''
    Say '  [8/9] 长期记忆：remember 落盘 + 下一次 Run 注入系统提示' 'Cyan'
    $s8 = Receive-Scenario -ConversationId $conv.id -Text '记住：E2E 记一条，用户偏好 4:3 画幅'
    $e8 = $s8.events
    $req8 = @(Get-Events $e8 'tool.requested')
    $r8 = if ($req8.Count -ge 1) { Convert-Data $req8[0] } else { $null }
    Check 'tool.requested.name == remember' ($null -ne $r8 -and $r8.name -eq 'remember') ($req8 | ConvertTo-Json -Compress)
    Check 'remember 不需要审批（写的是应用自己的记忆文件）' ($null -ne $r8 -and $r8.approval -ne 'pending') ($r8 | ConvertTo-Json -Compress)
    $comp8 = @(Get-Events $e8 'tool.completed')
    Check 'remember 执行成功' ($comp8.Count -ge 1) ($e8 | ConvertTo-Json -Depth 6 -Compress)

    $mem = Invoke-Api 'GET' '/api/ai/memory' $null
    Check "memory.md 里真的多了一条（entryCount=$($mem.entryCount)）" ($mem.entryCount -ge 1) ($mem | ConvertTo-Json -Compress)
    Check '记忆正文里带刚写的内容' ($mem.content -like '*E2E 记一条*') ($mem.content)
    Check '记忆文件落在 storage\ai\memory.md' ($mem.path -like '*storage\ai\memory.md') ($mem.path)

    # 再发一轮普通对话：系统提示里必须带上长期记忆（"以后每次对话都会带上它"）
    $beforeLog2 = @(Invoke-Gateway '/__log').Count
    $null = Receive-Scenario -ConversationId $conv.id -Text '你好'
    $logAfter = @(Invoke-Gateway '/__log')
    $runLog2 = @($logAfter | Select-Object -Skip $beforeLog2)
    Check '后续 Run 的系统提示里带上了那条记忆' `
        ((@($runLog2 | Where-Object { $_.systemMemoryHasProbe -eq $true })).Count -ge 1) `
        ($runLog2 | ConvertTo-Json -Depth 6 -Compress)
    Check '系统提示里有「长期记忆」段落' `
        ((@($runLog2 | Where-Object { $_.systemHasMemorySection -eq $true })).Count -ge 1) `
        ($runLog2 | ConvertTo-Json -Depth 6 -Compress)
    Check '下发给模型的工具里有 remember' `
        ((@($runLog2 | Where-Object { $_.hasRememberTool -eq $true })).Count -ge 1) `
        (($runLog2 | ForEach-Object { $_.toolNames -join ',' }) -join ' | ')

    # --- 11. 场景 9：附件（M3 / AIH-027 ~ AIH-031） -----------------------
    Say ''
    Say '  [9/9] 附件：上传 → 缩略图 → 内联进请求体 → 不支持的模型零请求' 'Cyan'

    function Set-E2eModel {
        param([string[]]$Modalities)
        Invoke-Api 'PUT' "/api/ai/providers/$ProviderId/models" @{
            models = @(@{
                providerId = $ProviderId
                id = $ModelId
                displayName = 'Fake Tools Model'
                inputModalities = $Modalities
                # 刻意**不声明** attachmentTransports：内置目录里那 69 个模型就是这样，
                # 传输方式是协议的属性，后端应当回落到适配器实现的那种（否则图片永远发不出去）
                attachmentTransports = @{}
                mimeAllowlist = @()
                tools = $true
                parallelTools = $false
                reasoning = $false
                thinkingEfforts = @{}
                contextWindow = 32768
                maxOutputTokens = 4096
                capabilitySource = 'manual'
                enabled = $true
            })
        } | Out-Null
    }

    Set-E2eModel -Modalities @('text', 'image')

    # 一张真的 PNG（2×2，脚本内联生成，不依赖 python 的图片库）：
    # 签名对得上，后端才会按 image 收下；而且它能被 ImageIO 解码，缩略图那条路才跑得通。
    $pngBase64 = 'iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAFklEQVR42mNwaDiQ0HCAwaFhQULDAgApDgYB4vLIfgAAAABJRU5ErkJggg=='
    [IO.File]::WriteAllBytes($script:AttachPng, [Convert]::FromBase64String($pngBase64))
    $upload = Invoke-RestMethod -Method 'Post' -Uri "$ApiBase/api/ai/attachments" `
        -Form @{ files = Get-Item $script:AttachPng } -TimeoutSec 60
    $item = @($upload.items) | Select-Object -First 1
    $script:AttachmentId = if ($item) { $item.id } else { $null }
    Check '附件上传成功且按签名判定为 image' ($null -ne $item -and $item.kind -eq 'image') ($upload | ConvertTo-Json -Compress)
    Check "附件 id / 大小 / 尺寸都在（size=$($item.sizeBytes)）" `
        (-not [string]::IsNullOrWhiteSpace("$($item.id)") -and $item.sizeBytes -gt 0 -and $item.width -eq 2)

    # 缩略图：图片走 JPEG 缩略图这条路（视频走同一张接口的预览帧）
    $thumb = Invoke-WebRequest -Uri "$ApiBase/api/ai/attachments/$($script:AttachmentId)/thumb" -TimeoutSec 60
    Check '缩略图接口 200 + image/jpeg + 非空' `
        ($thumb.StatusCode -eq 200 -and "$($thumb.Headers['Content-Type'])" -like 'image/jpeg*' -and
         $thumb.RawContentLength -gt 0) `
        ("$($thumb.StatusCode) $($thumb.Headers['Content-Type']) $($thumb.RawContentLength) bytes")

    # WebP（2026-09 用户要求"附件要支持 webp"）：这是**真的能解码**的回归防线 ——
    # 后端靠 ImageIO 插件读 webp，插件一旦被删 / 版本回退 / 打 fatJar 时服务文件合并丢了，
    # 编译与上传检查**全都照样通过**，只有"缩略图不是 JPEG"这一条会红。
    # 这张 64×64 有损（VP8）webp 是 Pillow 生成的，字节写死在下面。
    $webpBase64 = 'UklGRuoAAABXRUJQVlA4IN4AAACQCACdASpAAEAAPm0wkkayIyGhLAgCQA2JYjONegSAAFLTZ+qf5n7AAJJ/waDCo4kd4G3mQ4ftx///U6nCgIpl//99xQExTbx5N92fMAD+/6DU6mCVjkfOhi0I8uNUKj2DdJnq/rAPrkVF243S7MPMrGu8Ul80qyiVfB8x8Hnunp8OP5rWxLluq4jfQAz49c78P9t9P94sotsk6aZc9g6mR4CWpx6trTZcpJYfCfztO7j2rgviRmudIeJaevtpnAKp4RL+rOYWD1eaTGcxWXBtVBitBN4RFD4bIgiYwAA='
    [IO.File]::WriteAllBytes($script:AttachWebp, [Convert]::FromBase64String($webpBase64))
    $uploadWebp = Invoke-RestMethod -Method 'Post' -Uri "$ApiBase/api/ai/attachments" `
        -Form @{ files = Get-Item $script:AttachWebp } -TimeoutSec 60
    $witem = @($uploadWebp.items) | Select-Object -First 1
    $script:WebpAttachmentId = if ($witem) { $witem.id } else { $null }
    Check 'WebP 附件按签名收下（kind=image / mime=image/webp）' `
        ($null -ne $witem -and $witem.kind -eq 'image' -and $witem.mimeType -eq 'image/webp') `
        ($uploadWebp | ConvertTo-Json -Compress)
    Check 'WebP 尺寸探测正确（64×64，有损 VP8 头）' `
        ($null -ne $witem -and $witem.width -eq 64 -and $witem.height -eq 64) `
        ("$($witem.width)x$($witem.height)")

    $wthumb = Invoke-WebRequest -Uri "$ApiBase/api/ai/attachments/$($script:WebpAttachmentId)/thumb" -TimeoutSec 60
    Check 'WebP 缩略图 200 + image/jpeg（ImageIO 真的解出了 webp，不是回退原件）' `
        ($wthumb.StatusCode -eq 200 -and "$($wthumb.Headers['Content-Type'])" -like 'image/jpeg*' -and
         $wthumb.RawContentLength -gt 0) `
        ("$($wthumb.StatusCode) $($wthumb.Headers['Content-Type']) $($wthumb.RawContentLength) bytes")

    # 谎报类型骗不过准入：把一个文本文件改名成 .png 传上来必须被拒（AIH-027）
    $liar = Join-Path ([System.IO.Path]::GetTempPath()) 'comfyhub-e2e-liar.png'
    Set-Content -Path $liar -Value 'this is definitely not a png' -Encoding utf8
    $liarRejected = $false
    try {
        Invoke-RestMethod -Method 'Post' -Uri "$ApiBase/api/ai/attachments" -Form @{ files = Get-Item $liar } -TimeoutSec 60 | Out-Null
    } catch {
        $liarRejected = "$($_.ErrorDetails.Message)" -like '*UNSUPPORTED_CONTENT*'
    }
    Remove-Item $liar -Force -ErrorAction SilentlyContinue
    Check '签名不匹配的文件被拒绝入库（不是乐观放行）' $liarRejected

    $pre = Invoke-Api 'POST' '/api/ai/preflight' @{
        providerId = $ProviderId; modelId = $ModelId; attachmentIds = @($script:AttachmentId)
    }
    Check '预检放行（模型声明了 image + 协议实现了内联）' ($pre.allowed -eq $true) ($pre | ConvertTo-Json -Compress)

    $beforeLog3 = @(Invoke-Gateway '/__log').Count
    $s9 = Receive-Scenario -ConversationId $conv.id -Text '这张图里画的是什么？' -AttachmentIds @($script:AttachmentId)
    $e9 = $s9.events
    Check '带附件的 Run 正常完成' ((@(Get-Events $e9 'run.completed')).Count -ge 1) ($e9[-1].event)

    $log3 = @(@(Invoke-Gateway '/__log') | Select-Object -Skip $beforeLog3)
    $withImage = @($log3 | Where-Object { $_.imageCount -ge 1 })
    Check '上游请求里真的带了内联图片' ($withImage.Count -ge 1) ($log3 | ConvertTo-Json -Depth 6 -Compress)
    if ($withImage.Count -ge 1) {
        Check "图片是 data URL 且带 base64 载荷（prefix=$($withImage[0].imageUrlPrefix)）" `
            ($withImage[0].imageUrlPrefix -like 'data:image/png;base64,*' -and $withImage[0].imagePayloadChars -gt 60)
        Check '带图那一轮的 content 是"文本 + 图片"的有序块数组' ($withImage[0].userContentIsBlocks -eq $true)
    }

    # 用户消息落库时要带 attachment 有序块：重开 App 才画得出缩略图
    # （注意只挑 role=user：假网关会把提问原样回显在助手正文里，按文本找会挑错人）
    $msgs9 = @(Invoke-Api 'GET' "/api/ai/conversations/$($conv.id)/messages" $null)
    $user9 = @($msgs9 | Where-Object { $_.role -eq 'user' -and $_.text -like '*这张图里画的是什么*' }) |
        Select-Object -Last 1
    $att9 = if ($user9) { @($user9.parts | Where-Object { $_.type -eq 'attachment' }) | Select-Object -First 1 } else { $null }
    Check '用户消息落库带 attachment 有序块（含 attachmentId）' `
        ($null -ne $att9 -and $att9.attachmentId -eq $script:AttachmentId) `
        ($user9.parts | ConvertTo-Json -Depth 6 -Compress)

    # 零请求证明（AIH-030）：换成纯文本模型，同一个附件必须被拒，且**一个上游请求都不发**
    Set-E2eModel -Modalities @('text')
    $beforeLog4 = @(Invoke-Gateway '/__log').Count
    $blockedBody = $null
    try {
        Invoke-Api 'POST' "/api/ai/conversations/$($conv.id)/runs" @{
            text = '再发一次这张图'; providerId = $ProviderId; modelId = $ModelId
            attachmentIds = @($script:AttachmentId)
        } | Out-Null
    } catch {
        $blockedBody = "$($_.ErrorDetails.Message)"
    }
    $afterLog4 = @(Invoke-Gateway '/__log').Count
    Check '纯文本模型 + 图片 → 创建 Run 被拒且带 UNSUPPORTED_CONTENT' `
        ($blockedBody -like '*UNSUPPORTED_CONTENT*') $blockedBody
    Check '这一路**零上游请求**（before=$beforeLog4 after=$afterLog4）' ($afterLog4 -eq $beforeLog4)
} finally {
    # --- 清理 -------------------------------------------------------------
    Say ''
    Say '  清理' 'Cyan'
    Stop-Gateway
    if ($gatewayStarted) { Say '  已停止假网关' 'DarkGray' }

    if ($KeepData) {
        Say '  -KeepData：保留了测试产生的数据（会话 / Provider / skill / 临时文件）' 'Yellow'
    } else {
        if ($script:ConversationId) {
            try { Invoke-Api 'DELETE' "/api/ai/conversations/$($script:ConversationId)" $null | Out-Null } catch { }
        }
        try { Invoke-Api 'DELETE' "/api/ai/providers/$ProviderId" $null | Out-Null } catch { }
        try { Invoke-Api 'DELETE' "/api/ai/skills/$SkillName" $null | Out-Null } catch { }
        Remove-Item $SkillFile -Force -ErrorAction SilentlyContinue
        Remove-Item (Split-Path -Parent $SkillFile) -Recurse -Force -ErrorAction SilentlyContinue
        Remove-Item $HackFile -Force -ErrorAction SilentlyContinue
        Remove-Item $OkFile -Force -ErrorAction SilentlyContinue
        # 附件（M3）：先删库里的附件行（连原件与缩略图一起删），再删本地那两张测试图
        if ($script:AttachmentId) {
            try { Invoke-Api 'DELETE' "/api/ai/attachments/$($script:AttachmentId)" $null | Out-Null } catch { }
        }
        if ($script:WebpAttachmentId) {
            try { Invoke-Api 'DELETE' "/api/ai/attachments/$($script:WebpAttachmentId)" $null | Out-Null } catch { }
        }
        Remove-Item $script:AttachPng -Force -ErrorAction SilentlyContinue
        Remove-Item $script:AttachWebp -Force -ErrorAction SilentlyContinue
        if ($null -ne $script:MemoryBefore) {
            try {
                Invoke-Api 'PUT' '/api/ai/memory' @{ content = $script:MemoryBefore } | Out-Null
                Say '  已把长期记忆恢复成本次运行前的内容' 'DarkGray'
            } catch { }
        }
        if ($null -ne $script:PermissionModeBefore) {
            try {
                Invoke-Api 'PUT' '/api/ai/tools/policy' @{ permissionMode = $script:PermissionModeBefore } | Out-Null
                Say "  已把权限档还原成 '$($script:PermissionModeBefore)'" 'DarkGray'
            } catch { }
        }
        Say '  已删除会话 / Provider / skill / 临时文件' 'DarkGray'
    }
    if ($script:Http) { try { $script:Http.Dispose() } catch { } }
}

Say ''
Say ("  总计 {0} 项，失败 {1} 项" -f $script:Total, $script:Failed) $(if ($script:Failed -gt 0) { 'Red' } else { 'Green' })
Say ''
exit $script:Failed
