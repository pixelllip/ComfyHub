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
      9. 重试      —— `retryOfRunId` 复用**同一条**提问：空壳失败气泡就地复用（消息数不变、
          不留空壳），**有内容的**失败气泡原样留着、重试另起一条（用户 2026-09-19 报的
          "同一条语句出现两次" + "429 之后看不到已经回复完的内容"）
     10. 接着写    —— 输出被长度上限截断时**再要一轮**把话补完，而不是把半句当结尾（用户建议 ②）
     11. 首尾干净  —— 起标题的指令只发第一轮（回声标记不漏进正文、也不改会话名）+
         工具轮数用尽那一轮照给工具清单（模型就不会把工具语法当正文吐出来，用户 bug⑤）
     12. 限流保住  —— 被 HTTP 429 打断时正文与 parts（含工具卡）一个字都不丢；
         重试另起一条助手气泡，原来那条原样保留。另一半：**流到一半被掐断**时，
         已经流出去的正文照样要落成 part（parts 非空时界面只渲染 parts）

    还会核对落库的消息 parts 顺序（tool_call / tool_result / text），保证重开会话能渲染工具卡。

    脚本会**临时**改三处配置，跑完在 finally 里原样还原：
      · 权限档切成 `ask`（不然用户在界面上选了「自动允许」时，第 5 幕的审批断言必红）；
      · 长期记忆写一条测试记忆（结束时放回原文）；
      · 工具轮数上限钉成 1（第 12 幕要真的跑到"轮数用尽"那一轮）。

    前置条件：后端已经在跑，且是**带 M4/M5 的新构建**：
        pwsh -File scripts\backend.ps1 start     # 缺/旧会自动 cargo build 再起

.EXAMPLE
    pwsh -File scripts\e2e-ai-tools-test.ps1
    pwsh -File scripts\e2e-ai-tools-test.ps1 -KeepData
#>
[CmdletBinding()]
param(
    # 后端地址。默认按**后端记在案的端口**算（端口从 2026-09-22 起是自动挑的）。
    # 显式传 `-ApiBase …` 仍然照用。
    [string]$ApiBase = '',
    [int]$GatewayPort = 8799,
    [int]$TimeoutSec = 60,
    [switch]$KeepData
)

$ErrorActionPreference = 'Continue'
$ProjectRoot = Split-Path -Parent $PSScriptRoot
# 数据目录可能被移动到其他磁盘（`<根>\data-dir.json`，与 lib\core\backend_paths.dart 同一约定）
$DataDir = $ProjectRoot
$DataDirFile = Join-Path $ProjectRoot 'data-dir.json'
if (Test-Path $DataDirFile) {
    try { $customDataDir = (Get-Content $DataDirFile -Raw -Encoding UTF8 | ConvertFrom-Json).dataDir } catch { $customDataDir = '' }
    if ($customDataDir) { $DataDir = $customDataDir }
}
if (-not $ApiBase) {
    . (Join-Path $PSScriptRoot 'api-port.ps1')
    $portState = Read-ApiPortState -ProjectRoot $ProjectRoot
    $port = if ($portState -and $portState.Port -gt 0) { $portState.Port } else { 8080 }
    $ApiBase = "http://127.0.0.1:$port"
}
$E2eDir      = Join-Path $PSScriptRoot 'e2e'
$GatewayBase = "http://127.0.0.1:$GatewayPort"

$ProviderId  = 'e2e-tools'
$ModelId     = 'fake-tools-model'
$SkillName   = 'e2e-tool-demo'
$ConversationTitle = 'e2e-ai-tools'

# 第 10 幕（重试不重复提问）用：一个指向死端口的 Provider（造一次必然失败的 Run）
$DeadProviderId = 'e2e-tools-dead'
$DeadPort       = 59901
$RetryConversationTitle = 'e2e-ai-tools-retry'

# 第 11 幕（接着写）：假网关见到这个关键词就只吐半段 + finish_reason=length
$TruncateAsk = '续写测试'
$HeadText = '第一段正文：这段话写到一半就被上游的输出长度上限切断了，'
$TailText = '第二段正文：这里是接着写下去的部分。'

# 第 13 幕（被不可抗力打断时内容不许消失）：
#   · 假网关第一轮吐正文 + 工具调用，工具结果回填那一轮返回 HTTP 429（真机实据就是这么回来的）；
#   · 另一半是"流到一半被掐断"：第一轮照旧跑一次工具（让 parts 非空），第二轮吐一半正文就断连接。
$FailKeepAsk  = '限流截断：先看状态再动手'
$FailKeepText = '先把本机状态查出来，再动手：'
$BreakAsk     = '中途断流：先看状态再动手'
$BreakFirst   = '先查一下当前状态：'
$BreakTail    = '状态查完了，接下来这一步其实是：'

$SkillFile   = Join-Path $DataDir "storage\ai\skills\$SkillName\SKILL.md"
$HackFile    = Join-Path $DataDir 'storage\e2e-hack.txt'
$OkFile      = Join-Path $ProjectRoot 'comfyui\e2e-ok.txt'
$ExpectedWriteRoot = Join-Path $ProjectRoot 'comfyui'

$script:Failed = 0
$script:Total  = 0
$script:Http   = $null
$script:Streams = @{}
$script:ConversationId = $null
$script:RetryConversationId = $null
$script:TitleConversationId = $null
$script:FailKeepConversationId = $null
$script:BreakConversationId = $null

# 长期记忆的原内容：本脚本会往里写一条测试记忆，结束时原样放回
$script:MemoryBefore = $null

# 工具轮数上限的原值：第 12 幕要临时钉成 1（跑到"轮数用尽"那一轮），结束时原样放回
$script:MaxStepsBefore = $null

# 权限档的原值：审批那一幕要求 `comfy_sync_history` 是 pending，而在界面上把档位切换到
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
    # 不写死路径：交给 comfy-path.ps1 里那套"现探 ComfyUI 装在哪"的判据
    # （以前这里列的是 D:\Comfy-Desktop\... —— 只有开发机能命中，2026-11-10 复查改掉）
    if (-not (Get-Command Find-ComfyPython -ErrorAction SilentlyContinue)) {
        . (Join-Path $PSScriptRoot 'comfy-path.ps1')
    }
    return Find-ComfyPython -ProjectRoot $ProjectRoot
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
    Say '    pwsh -File scripts\backend.ps1 start' 'Yellow'
    exit 1
}
Say ("  后端: {0}  v{1}  db={2}" -f $ApiBase, $health.version, $health.database) 'DarkGray'

$gatewayStarted = $false

# SSE 用流式读；整体超时给足（Run 本身可能跑几十秒）
$script:Http = [System.Net.Http.HttpClient]::new()
$script:Http.Timeout = [TimeSpan]::FromSeconds(300)

try {
    # --- 1. 启动假网关（静默，绝不弹窗；finally 里必杀） -------------------
    # 2026-09-30：silent-process.ps1 已删（WMI 那套随 App 直启退场）。这里不需要
    # 脱离进程树 —— 脚本全程活着、finally 必杀，用 Start-Process -WindowStyle Hidden
    # 就够了（= .NET CreateProcess + SW_HIDE，不弹窗；console 继承本终端的那个）。
    Stop-Gateway   # 清掉上一次残留
    $gatewayProc = Start-Process -FilePath $python `
        -ArgumentList "`"$(Join-Path $E2eDir 'fake_openai.py')`"", '--port', $GatewayPort `
        -WorkingDirectory $ProjectRoot -WindowStyle Hidden -PassThru
    $gatewayStarted = $true
    Say "  假网关已起（pid $($gatewayProc.Id)，hidden）" 'DarkGray'

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
    Say '  [0/13] 配置 e2e Provider / 模型' 'Cyan'

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
    # 长期记忆先存一份：后面会往里面写测试数据，结束时原样放回（不能改动实际记忆内容）
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
    Say '  [1/13] 注册：register_skill 落盘' 'Cyan'
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
    Say '  [2/13] 按需加载：load_skill + 工具结果回灌上游' 'Cyan'
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
    Say '  [3/13] 越界写被拒：write_file 打到 storage/' 'Cyan'
    $s3 = Receive-Scenario -ConversationId $conv.id -Text '越界写个文件'
    $e3 = $s3.events
    $fail3 = @(Get-Events $e3 'tool.failed')
    $f3 = if ($fail3.Count -ge 1) { Convert-Data $fail3[0] } else { $null }
    Check 'write_file 越界 -> tool.failed' ($null -ne $f3 -and $f3.name -eq 'write_file') ($fail3 | ConvertTo-Json -Compress)
    Check '错误码 == PATH_DENIED' ($null -ne $f3 -and $f3.code -eq 'PATH_DENIED') ($f3 | ConvertTo-Json -Compress)
    Check "storage\e2e-hack.txt 不存在" (-not (Test-Path $HackFile))

    $policy = Invoke-Api 'GET' '/api/ai/tools/policy' $null
    $roots = @($policy.writeRoots)
    # ⚠ 该项检查"**代码**不得自行向写白名单添加目录"（本机 ComfyUI
    # 目录只放宽**读**）。而白名单属于**配置**：为向 ComfyUI 的 input 目录写文件，
    # 可能在设置页添加过 `D:\Comfy-Desktop`——
    # 此时"恰好只有 comfyui"不成立，要求严格相等会使该 e2e 持续失败，
    # 无法区分新增回归与配置差异。因此：
    #   · 只有一项时 → 仍然**严格相等**（干净环境下作为硬性检查）；
    #   · 多于一项时 → 只要求"项目 comfyui 在列表中"，并**输出**多出的目录。
    # "代码不自行添加目录"由 `api_test.rs` 的默认值断言保证（该用例不读取配置）。
    if ($roots.Count -eq 1) {
        Check "writeRoots 恰好是 $ExpectedWriteRoot" `
            ($roots[0].TrimEnd('\') -ieq $ExpectedWriteRoot.TrimEnd('\')) `
            ($roots -join ' | ')
    } else {
        $has = @($roots | Where-Object { $_.TrimEnd('\') -ieq $ExpectedWriteRoot.TrimEnd('\') }).Count -ge 1
        Check "writeRoots 含 $ExpectedWriteRoot（另有用户自己加的目录）" $has ($roots -join ' | ')
        Say "    · 写白名单里还有用户自己加的目录：$(($roots | Where-Object { $_.TrimEnd('\') -ine $ExpectedWriteRoot.TrimEnd('\') }) -join '、')" 'DarkGray'
    }

    # --- 6. 场景 4：目录内写成功 ------------------------------------------
    Say ''
    Say '  [4/13] 目录内写成功：write_file 打到 comfyui/' 'Cyan'
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
    Say '  [5/13] 审批：comfy_sync_history 必须等批准才执行' 'Cyan'
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
    Say '  [6/13] 只读工具：comfy_get_status 不需要审批' 'Cyan'
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
    Say '  [7/13] 落库校验：GET /api/ai/runs/{id} + /conversations/{id}/messages' 'Cyan'
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
    Say '  [8/13] 长期记忆：remember 落盘 + 下一次 Run 注入系统提示' 'Cyan'
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
    Say '  [9/13] 附件：上传 → 缩略图 → 内联进请求体 → 不支持的模型零请求' 'Cyan'

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

    # WebP：这是**实际解码**的回归防线 ——
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

    # 请求消息落库时需带 attachment 有序块：重开 App 后才能显示缩略图
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

    # --- 12. 场景 10：重试**不重复提问**（AIH-024："同一条语句出现两次"）
    #
    # 现场：会话里点一次「重试」，就多出一条一模一样的提问（库里同时留着新提问 + 旧的失败气泡）。
    # 原因是 `POST /conversations/{id}/runs` 不读取 `retryOfRunId`，照旧 append 了一条新消息。
    # 这一幕检查三件事：重试复用**同一条**请求消息、复用**同一条**助手占位、消息总数不变。
    Say ''
    Say '  [10/13] 重试：复用原来那条提问与失败气泡，绝不新增' 'Cyan'

    # 先造一次"必然失败"的 Run：Provider 指向一个没人监听的端口（连接被拒 → PROVIDER_UNREACHABLE）
    try { Invoke-Api 'DELETE' "/api/ai/providers/$DeadProviderId" $null | Out-Null } catch { }
    $null = Invoke-Api 'POST' '/api/ai/providers' @{
        id = $DeadProviderId
        displayName = 'E2E Dead (fake)'
        api = 'openai-completions'
        baseURL = "http://127.0.0.1:$DeadPort/v1"
        credentialRef = $null
        endpointTrust = 'loopback'
        enabled = $true
    }
    $null = Invoke-Api 'PUT' "/api/ai/providers/$DeadProviderId/models" @{
        models = @(@{
            providerId = $DeadProviderId
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

    $retryConv = Invoke-Api 'POST' '/api/ai/conversations' @{ title = $RetryConversationTitle; providerId = $DeadProviderId; modelId = $ModelId }
    $script:RetryConversationId = $retryConv.id
    $ask = '重试用例：这句话在会话里只能出现一次'
    $deadRun = Invoke-Api 'POST' "/api/ai/conversations/$($retryConv.id)/runs" @{
        text = $ask; providerId = $DeadProviderId; modelId = $ModelId
    }

    $failedRun = $null
    foreach ($i in 1..60) {
        Start-Sleep -Milliseconds 300
        $failedRun = Invoke-Api 'GET' "/api/ai/runs/$($deadRun.runId)" $null
        if ($failedRun.status -in @('failed', 'completed', 'cancelled')) { break }
    }
    Check '第一次 Run 如实失败（PROVIDER_UNREACHABLE）' `
        ($null -ne $failedRun -and $failedRun.status -eq 'failed') ($failedRun | ConvertTo-Json -Compress)

    $msgsBefore = @(Invoke-Api 'GET' "/api/ai/conversations/$($retryConv.id)/messages" $null)
    Check '失败之后会话里是 1 问 1 答（2 条）' ($msgsBefore.Count -eq 2) `
        (($msgsBefore | ForEach-Object { "$($_.role):$($_.status)" }) -join ' | ')

    # 重试：同一句话 + retryOfRunId（Provider 换成能通的那个）
    $retryRun = Invoke-Api 'POST' "/api/ai/conversations/$($retryConv.id)/runs" @{
        text = $ask; providerId = $ProviderId; modelId = $ModelId; retryOfRunId = $deadRun.runId
    }
    Check '重试复用了**原来那条**用户消息（userMessageId 不变）' `
        ($retryRun.userMessageId -eq $deadRun.userMessageId) `
        ("before=$($deadRun.userMessageId) after=$($retryRun.userMessageId)")
    Check '重试复用了**那条失败气泡**（assistantMessageId 不变）' `
        ($retryRun.assistantMessageId -eq $deadRun.assistantMessageId) `
        ("before=$($deadRun.assistantMessageId) after=$($retryRun.assistantMessageId)")

    Open-RunStream -RunId $retryRun.runId
    $e10 = @(Receive-RunEvents -RunId $retryRun.runId -TimeoutSec $TimeoutSec)
    Close-RunStream -RunId $retryRun.runId
    Check '重试的 Run 正常完成' ((@(Get-Events $e10 'run.completed')).Count -ge 1) ($e10[-1].event)

    $msgsAfter = @(Invoke-Api 'GET' "/api/ai/conversations/$($retryConv.id)/messages" $null)
    Check '重试之后**仍然**只有 2 条消息（没有多出一条重复提问、也没留空壳气泡）' `
        ($msgsAfter.Count -eq 2) `
        (($msgsAfter | ForEach-Object { "$($_.role):$($_.status):$($_.text.Length)" }) -join ' | ')
    $askRows = @($msgsAfter | Where-Object { $_.role -eq 'user' -and $_.text -eq $ask })
    Check '那句提问在会话里只有 1 条' ($askRows.Count -eq 1) ("找到 $($askRows.Count) 条")
    $assistantRows = @($msgsAfter | Where-Object { $_.role -eq 'assistant' })
    Check '助手气泡只有 1 条' ($assistantRows.Count -eq 1) ("找到 $($assistantRows.Count) 条")

    # --- 13. 场景 11：输出被长度上限截断时**接着写**，不强行截断 ------
    #
    # 现场：长回复被上游的 max_tokens 切断（这里用假网关的 finish_reason=length 复现），
    # 后端必须**再要一轮**把话说完，而不是把半句话当成结尾。
    Say ''
    Say '  [11/13] 接着写：输出被长度上限截断时把回复补完（不强行截断）' 'Cyan'

    $beforeLog5 = @(Invoke-Gateway '/__log').Count
    $s11 = Receive-Scenario -ConversationId $conv.id -Text "$TruncateAsk 请写两段话"
    $e11 = $s11.events
    Check '截断那一轮正常收尾（run.completed）' ((@(Get-Events $e11 'run.completed')).Count -ge 1) ($e11[-1].event)

    $log5 = @(@(Invoke-Gateway '/__log') | Select-Object -Skip $beforeLog5)
    Check '后端为"接着写"真的多要了一轮上游请求（共 2 次）' ($log5.Count -eq 2) `
        (($log5 | ForEach-Object { $_.decision }) -join ' | ')
    if ($log5.Count -ge 2) {
        Check '第一轮是被截断的那一轮（finish_reason=length）' ($log5[0].decision -eq 'truncated') `
            ($log5[0] | ConvertTo-Json -Compress)
        Check '第二轮是接着写（最后一条 user 就是续写指令）' ($log5[1].decision -eq 'continue') `
            ($log5[1] | ConvertTo-Json -Compress)
        Check '续写请求里真的带上了"已经说过的那半段"（最后一条 assistant 正文）' `
            ($log5[1].lastAssistantText -like "*$HeadText*") `
            ($log5[1].lastAssistantText)
    }

    $msgs11 = @(Invoke-Api 'GET' "/api/ai/conversations/$($conv.id)/messages" $null)
    $answer11 = @($msgs11 | Where-Object { $_.id -eq $s11.run.assistantMessageId }) | Select-Object -First 1
    Check '截断后的回复被补完了（两段都在同一条消息里）' `
        ($null -ne $answer11 -and $answer11.text -like "*$HeadText*" -and $answer11.text -like "*$TailText*") `
        ($answer11.text)
    Check '两段按顺序拼在一起、没有把结尾重说一遍' `
        ($null -ne $answer11 -and
         $answer11.text.IndexOf($HeadText) -ge 0 -and
         $answer11.text.IndexOf($TailText) -gt $answer11.text.IndexOf($HeadText) -and
         $answer11.text.Split($HeadText).Count -eq 2) `
        ($answer11.text)

    # --- 14. 场景 12：回答首尾不再"被截断"--------------------------
    #
    # 现场（会话 ab487605，2026-09-19）：
    #   ① 10:08 那条回复里漏出一行 `[标题] Krea2 初音画风迁移出图[/标题]`（"首"）；
    #   ② 10:12 那条回复的**尾巴**是一段原始的调用语法（"尾"），看起来像被截断在一半。
    # 根因两条：
    #   ① 起标题的指令原先常驻在每一轮的上下文里，后面的工具轮 / 收尾轮还在提醒模型
    #      "在最前面输出一行 [标题]"，模型就又吐一遍，而那时摘取状态机已经收工 → 标记漏进正文；
    #   ② 工具轮数用尽的那一轮原先**不给工具清单**，DeepSeek 系模型就把工具语法当正文吐了出来。
    Say ''
    Say '  [12/13] 首尾不再"被截断"：起标题指令只发首轮 + 收尾轮照给工具清单' 'Cyan'

    $script:MaxStepsBefore = (Invoke-Api 'GET' '/api/ai/tools/policy' $null).maxToolSteps

    # 自动起标题只在"还没有标题"的会话上做 —— 所以这里专门建一条不带标题的
    $titleConv = Invoke-Api 'POST' '/api/ai/conversations' @{ providerId = $ProviderId; modelId = $ModelId }
    $script:TitleConversationId = $titleConv.id
    Check '不带标题的会话已建好（标题回落成「新对话」，才会触发自动起标题）' `
        ($titleConv.title -eq '新对话') ($titleConv | ConvertTo-Json -Compress)

    $beforeLog6 = @(Invoke-Gateway '/__log').Count
    $s12 = Receive-Scenario -ConversationId $titleConv.id -Text '回声标题：随便做点什么'
    Check '起标题那一幕正常收尾（run.completed）' `
        ((@(Get-Events $s12.events 'run.completed')).Count -ge 1) ($s12.events[-1].event)

    $log6 = @(@(Invoke-Gateway '/__log') | Select-Object -Skip $beforeLog6)
    Check '这一轮发了两次上游请求（首轮 + 工具结果回填）' ($log6.Count -eq 2) `
        (($log6 | ForEach-Object { $_.decision }) -join ' | ')
    if ($log6.Count -ge 2) {
        Check '起标题的指令**只出现在第一轮**请求里' `
            ($log6[0].systemHasTitleAsk -eq $true -and $log6[1].systemHasTitleAsk -eq $false) `
            (($log6 | ForEach-Object { "titleAsk=$($_.systemHasTitleAsk)" }) -join ' | ')
    }

    $msgs12 = @(Invoke-Api 'GET' "/api/ai/conversations/$($titleConv.id)/messages" $null)
    $answer12 = @($msgs12 | Where-Object { $_.id -eq $s12.run.assistantMessageId }) | Select-Object -First 1
    Check '回声（第二遍的标记）没有漏进正文' `
        ($null -ne $answer12 -and $answer12.text -notmatch '\[标题\]' -and $answer12.text -like '*回执来了*') `
        ($answer12.text)
    $conv12 = @(Invoke-Api 'GET' '/api/ai/conversations?includeArchived=true' $null) |
        Where-Object { $_.id -eq $titleConv.id } | Select-Object -First 1
    Check '会话名保持最早摘到的那个（回声不许改名）' `
        ($null -ne $conv12 -and $conv12.title -eq 'E2E 回声标题') ($conv12.title)

    # 收尾轮：把工具轮数上限临时钉成 1，第二轮的 step 就等于上限
    Invoke-Api 'PUT' '/api/ai/tools/policy' @{ maxToolSteps = 1 } | Out-Null
    $beforeLog7 = @(Invoke-Gateway '/__log').Count
    $s13 = Receive-Scenario -ConversationId $titleConv.id -Text '轮数收尾：再查一次状态'
    Check '收尾那一幕正常收尾（run.completed）' `
        ((@(Get-Events $s13.events 'run.completed')).Count -ge 1) ($s13.events[-1].event)

    $log7 = @(@(Invoke-Gateway '/__log') | Select-Object -Skip $beforeLog7)
    Check '这一轮同样只发两次上游请求（没执行的工具不会引发无限循环）' ($log7.Count -eq 2) `
        (($log7 | ForEach-Object { $_.decision }) -join ' | ')
    if ($log7.Count -ge 2) {
        Check '轮数用尽那一轮**照样**带上了工具清单' ($log7[1].toolsPresent -eq $true) `
            ($log7[1] | ConvertTo-Json -Depth 6 -Compress)
    }
    $msgs13 = @(Invoke-Api 'GET' "/api/ai/conversations/$($titleConv.id)/messages" $null)
    $answer13 = @($msgs13 | Where-Object { $_.id -eq $s13.run.assistantMessageId }) | Select-Object -First 1
    Check '正文里没有原始的调用语法（模型没机会把它当正文吐出来）' `
        ($null -ne $answer13 -and $answer13.text -notmatch 'invoke name=') ($answer13.text)
    Check '如实说明"工具轮数到上限、这次没执行"' `
        ($null -ne $answer13 -and $answer13.text -match '工具轮数上限') ($answer13.text)

    # --- 15. 场景 13：被 429 打断时，已回复的内容不得丢失 -------
    #
    # 现场（会话 9b6d01af 的 seq12，**查库实据**）：那次 Run 跑了 4 分钟、成功调了 3 次
    # `comfy_submit`，最后一轮返回 HTTP 429（`error_code=RATE_LIMIT`）。随后点「重试」，
    # 结果**整条回复连同工具卡一起消失** —— 库里那条消息 `text=''`、一个 part 都没有：
    # `resetAssistantForRetry` 把"有内容的失败回复"就地清零了（现在只在**空壳**上才这么做）。
    # 这一幕钉五件事：
    #   ① 失败时正文与 parts（正文 + 工具调用 + 工具结果）都还在；
    #   ② 重试**另起一条**助手气泡（原来那条原样保留，含工具卡）；
    #   ③ 提问仍然只有 1 条（重试复用那条请求消息，不重复提问）；
    #   ④ 重试能正常收尾，新气泡是完整回复；
    #   ⑤ 假网关确实按"先内容、再 429、重试成功"三步走的（防止断言失真）。
    Say ''
    Say '  [13/13] 被 429 打断：已经回复完的内容不许消失，重试另起一条' 'Cyan'

    # 上一幕把工具轮数上限钉成了 1（那一幕要跑到"轮数用尽"）；这一幕要真的执行工具，先还原
    if ($null -ne $script:MaxStepsBefore) {
        Invoke-Api 'PUT' '/api/ai/tools/policy' @{ maxToolSteps = $script:MaxStepsBefore } | Out-Null
    }

    # 会话**先给个标题**再发：这一幕不打算把"自动起标题"那条链路卷进断言
    # （没有标题时后端会按轮带上"起标题"的 system 指令；它固定在系统提示之后，
    #  但一劳永逸地把这条链路摘出去，断言才只盯"工具轮之后撞 429"这一件事）。
    $failConv = Invoke-Api 'POST' '/api/ai/conversations' @{
        title = 'e2e-ai-tools-failkeep'; providerId = $ProviderId; modelId = $ModelId
    }
    $script:FailKeepConversationId = $failConv.id

    $beforeLog8 = @(Invoke-Gateway '/__log').Count
    $s14 = Receive-Scenario -ConversationId $failConv.id -Text $FailKeepAsk

    # 先钉假网关的走法：这一轮**真的**是"先吐内容、再被 429 打断"（断言不空转）
    $log8 = @(@(Invoke-Gateway '/__log') | Select-Object -Skip $beforeLog8)
    $decisions8 = @($log8 | ForEach-Object { $_.decision })
    Check '假网关确实走了"先内容 + 工具 → 429"两步' `
        (($decisions8 -join ' | ') -eq 'failkeep_tool | failkeep_429') ($decisions8 -join ' | ')

    $failEv = @(Get-Events $s14.events 'run.failed') | Select-Object -First 1
    Check '被限流的那次 Run 如实失败（RATE_LIMIT，不假装成功）' `
        ($null -ne $failEv -and (Convert-Data $failEv).code -eq 'RATE_LIMIT') `
        ($s14.events[-1].event)

    $msgs14 = @(Invoke-Api 'GET' "/api/ai/conversations/$($failConv.id)/messages" $null)
    $failedMsg = @($msgs14 | Where-Object { $_.id -eq $s14.run.assistantMessageId }) | Select-Object -First 1
    Check '失败之后会话里是 1 问 1 答（2 条）' ($msgs14.Count -eq 2) `
        (($msgs14 | ForEach-Object { "$($_.role):$($_.status)" }) -join ' | ')
    Check '失败的那条回复**正文还在**（429 不许把已经说出去的话抹掉）' `
        ($null -ne $failedMsg -and $failedMsg.status -eq 'failed' -and
         $failedMsg.text -like "*$FailKeepText*") ($failedMsg.text)
    $failedTypes = @($failedMsg.parts | ForEach-Object { $_.type })
    Check '失败的那条回复**parts 也在**（正文 + 工具卡：少了块，界面上就看不见这些内容）' `
        (($failedTypes -contains 'text') -and ($failedTypes -contains 'tool_call') -and
         ($failedTypes -contains 'tool_result')) ($failedTypes -join ' | ')

    # 重试：原来那条**有内容**，后端必须另起一条助手气泡（不是就地清零）
    $retry14 = $null
    $retryErr = ''
    try {
        $retry14 = Invoke-Api 'POST' "/api/ai/conversations/$($failConv.id)/runs" @{
            text = $FailKeepAsk; providerId = $ProviderId; modelId = $ModelId
            retryOfRunId = $s14.run.runId
        }
    } catch {
        $retryErr = "$($_.ErrorDetails.Message)"
    }
    Check '重试请求被接受（有内容的失败回复允许重试）' ($null -ne $retry14) $retryErr
    if ($null -ne $retry14) {
        Check '重试**另起**了一条助手气泡（有内容的失败回复不许被就地重置）' `
            ($retry14.assistantMessageId -ne $s14.run.assistantMessageId) `
            ("before=$($s14.run.assistantMessageId) after=$($retry14.assistantMessageId)")
        Check '重试仍然复用原来那条提问（userMessageId 不变）' `
            ($retry14.userMessageId -eq $s14.run.userMessageId) `
            ("before=$($s14.run.userMessageId) after=$($retry14.userMessageId)")

        Open-RunStream -RunId $retry14.runId
        $e14 = @(Receive-RunEvents -RunId $retry14.runId -TimeoutSec $TimeoutSec)
        Close-RunStream -RunId $retry14.runId
        Check '重试的 Run 正常完成' ((@(Get-Events $e14 'run.completed')).Count -ge 1) ($e14[-1].event)

        $msgs14b = @(Invoke-Api 'GET' "/api/ai/conversations/$($failConv.id)/messages" $null)
        Check '重试之后是 3 条：1 条提问 + 两条助手气泡（旧的留在原地，新的另起）' `
            ($msgs14b.Count -eq 3) `
            (($msgs14b | ForEach-Object { "$($_.role):$($_.status):$($_.text.Length)" }) -join ' | ')
        $old14 = @($msgs14b | Where-Object { $_.id -eq $s14.run.assistantMessageId }) | Select-Object -First 1
        $old14Types = @($old14.parts | ForEach-Object { $_.type })
        Check '重试没有擦掉原来那条：正文与 parts（含工具卡）一个不少' `
            ($null -ne $old14 -and $old14.status -eq 'failed' -and
             $old14.text -like "*$FailKeepText*" -and
             ($old14Types -contains 'text') -and ($old14Types -contains 'tool_call') -and
             ($old14Types -contains 'tool_result')) `
            ("text=$($old14.text) parts=$($old14Types -join ' | ')")
        $new14 = @($msgs14b | Where-Object { $_.id -eq $retry14.assistantMessageId }) | Select-Object -First 1
        Check '新的那条是完整回复（重试真的接上了没说完的部分）' `
            ($null -ne $new14 -and $new14.status -eq 'complete' -and
             $new14.text -like '*重试之后把话说完*') ($new14.text)
        Check '那句提问在会话里仍然只有 1 条（重试不重复提问）' `
            (@($msgs14b | Where-Object { $_.role -eq 'user' }).Count -eq 1) `
            (@($msgs14b | Where-Object { $_.role -eq 'user' }).Count)
    }

    $decisions8b = @(@(Invoke-Gateway '/__log') | Select-Object -Skip $beforeLog8 |
        ForEach-Object { $_.decision })
    Check '假网关最后一步是"重试正常收尾"（三步都真的走了）' `
        (($decisions8b -join ' | ') -eq 'failkeep_tool | failkeep_429 | failkeep_final') `
        ($decisions8b -join ' | ')

    # 另一半：**流到一半被掐断**（这一轮已经流出去的正文还没落成 part，见 HarnessRunner 的
    # `rescueAlreadySaid` / `PartSink.flushInflight`）。第一轮先跑完一次工具 → parts 非空，
    # 于是"这一轮的半段正文"如果没补成 part，界面上就只渲染 parts、那半段就看不见了
    # （退回去实测：正文里有、parts 里没有）。
    Say ''
    Say '  [13/13·续] 流到一半被掐断：已经流出去的正文照样要落成 part' 'Cyan'

    $breakConv = Invoke-Api 'POST' '/api/ai/conversations' @{
        title = 'e2e-ai-tools-break'; providerId = $ProviderId; modelId = $ModelId
    }
    $script:BreakConversationId = $breakConv.id

    $s15 = Receive-Scenario -ConversationId $breakConv.id -Text $BreakAsk
    $breakEv = @(Get-Events $s15.events 'run.failed') | Select-Object -First 1
    Check '中途断流的那次 Run 如实失败（不假装答完）' ($null -ne $breakEv) ($s15.events[-1].event)

    $msgs15 = @(Invoke-Api 'GET' "/api/ai/conversations/$($breakConv.id)/messages" $null)
    $breakMsg = @($msgs15 | Where-Object { $_.id -eq $s15.run.assistantMessageId }) | Select-Object -First 1
    $breakTexts = @($breakMsg.parts | Where-Object { $_.type -eq 'text' } | ForEach-Object { $_.text })
    Check '被掐断前流出去的正文在正文里（text）' `
        ($null -ne $breakMsg -and $breakMsg.text -like "*$BreakTail*") ($breakMsg.text)
    Check '同样也落成了 part（parts 非空时界面只渲染 parts —— 少了它用户就看不见这半段）' `
        (($breakTexts -join '') -like "*$BreakTail*") ($breakTexts -join ' || ')
    Check '工具轮那一轮的内容也还在（工具卡没被这次失败吃掉）' `
        (@($breakMsg.parts | Where-Object { $_.type -eq 'tool_call' }).Count -ge 1 -and
         (@($breakMsg.parts | ForEach-Object { $_.type }) -contains 'tool_result')) `
        (@($breakMsg.parts | ForEach-Object { $_.type }) -join ' | ')
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
        # 第 10 幕的会话与那个"死端口 Provider"也要清理干净（否则会在工作台中显示）
        if ($script:RetryConversationId) {
            try { Invoke-Api 'DELETE' "/api/ai/conversations/$($script:RetryConversationId)" $null | Out-Null } catch { }
        }
        # 第 12 幕那条"不带标题"的会话（自动起标题的现场）
        if ($script:TitleConversationId) {
            try { Invoke-Api 'DELETE' "/api/ai/conversations/$($script:TitleConversationId)" $null | Out-Null } catch { }
        }
        # 第 13 幕那条"被 429 打断"的会话
        if ($script:FailKeepConversationId) {
            try { Invoke-Api 'DELETE' "/api/ai/conversations/$($script:FailKeepConversationId)" $null | Out-Null } catch { }
        }
        # 第 13 幕那条"流到一半被掐断"的会话
        if ($script:BreakConversationId) {
            try { Invoke-Api 'DELETE' "/api/ai/conversations/$($script:BreakConversationId)" $null | Out-Null } catch { }
        }
        try { Invoke-Api 'DELETE' "/api/ai/providers/$DeadProviderId" $null | Out-Null } catch { }
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
    # 工具轮数上限是"改了配置"而不是"造了数据"，**-KeepData 时也必须还原**
    if ($null -ne $script:MaxStepsBefore) {
        try {
            Invoke-Api 'PUT' '/api/ai/tools/policy' @{ maxToolSteps = $script:MaxStepsBefore } | Out-Null
            Say "  已把工具轮数上限还原成 $($script:MaxStepsBefore)" 'DarkGray'
        } catch { }
    }
    if ($script:Http) { try { $script:Http.Dispose() } catch { } }
}

# ★ **一条都没跑 = 没通过**（2026-09-30 补）：整段断言可能被静默跳过，而那时 Failed=0
#   —— 只看 Failed 就会 exit 0，把"什么都没验"报成通过（总纲 §26 记的是同一族假绿）。
if ($script:Total -le 0) {
    Say '  ✗ 一项检查都没跑（后端没起？前置条件没过？）—— 这不叫通过。' 'Red'
    exit 1
}
Say ''
Say ("  总计 {0} 项，失败 {1} 项" -f $script:Total, $script:Failed) $(if ($script:Failed -gt 0) { 'Red' } else { 'Green' })
Say ''
exit $script:Failed



