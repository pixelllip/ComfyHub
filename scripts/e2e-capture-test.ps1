<#
.SYNOPSIS
    ComfyHub「ComfyUI 自动捕获」端到端自测（不需要真的跑一次生成）。

.DESCRIPTION
    用 scripts\e2e\fake_comfy.py 假装成一个 ComfyUI，把整条链路走一遍：

      1. 轮询捕获   —— 后端读 /history，解析参数、存工作流、把产物入库并与提示词关联
      2. 幂等       —— 再轮询一次不应重复入库
      3. 目录导入   —— 已经生成好的 PNG，靠内嵌元数据自动建提示词；重复文件自动跳过
      4. 推送捕获   —— POST /api/ingest/comfyui（ComfyUI 自定义节点走的就是这条）
      5. 清理       —— 默认把测试产生的提示词 / 产物 / 运行记录删掉（用 -KeepData 保留）

    前置条件：MySQL + 后端已经在跑（scripts\comfyhub.ps1 up），本机有 python。

.EXAMPLE
    pwsh -File scripts\e2e-capture-test.ps1
    pwsh -File scripts\e2e-capture-test.ps1 -KeepData
#>
[CmdletBinding()]
param(
    [string]$ApiBase = 'http://127.0.0.1:8080',
    # **故意不用 8188**：那是真实 ComfyUI 的默认端口，开发机上它多半正开着。
    # 撞上之后假 ComfyUI 起不来（Address already in use），而就绪探测会**连上真实的那个**、
    # 于是脚本一路跑下去读到陌生人的 /history —— 表现为"轮询啥也没收到"，极难查。
    [int]$ComfyPort = 18188,
    [switch]$KeepData
)

$ErrorActionPreference = 'Continue'
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$E2eDir      = Join-Path $PSScriptRoot 'e2e'
$WorkDir     = Join-Path $ProjectRoot '.run\e2e'
$OutDir      = Join-Path $WorkDir 'output'
# 三张测试图（内容由 make_png.py 确定性生成：同名同色 → 同字节）
$capturePng  = Join-Path $OutDir 'e2e_capture_00001_.png'
$importPng   = Join-Path $OutDir 'e2e_import_00002_.png'
$pushPng     = Join-Path $OutDir 'e2e_push_00003_.png'

$script:Failed = 0
$script:CreatedPromptIds = New-Object System.Collections.Generic.List[long]
$script:CreatedMediaIds = New-Object System.Collections.Generic.List[long]

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
    if ($null -eq $Body) {
        return Invoke-RestMethod -Method $Method -Uri $uri -TimeoutSec 120
    }
    $json = $Body | ConvertTo-Json -Depth 12 -Compress
    return Invoke-RestMethod -Method $Method -Uri $uri -TimeoutSec 180 `
        -ContentType 'application/json; charset=utf-8' -Body $json
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

# ---------------------------------------------------------------------------

Say ''
Say '  ComfyHub 自动捕获 · 端到端自测' 'White'
Say '  ────────────────────────────────────────────────────────────'

# --- 0. 环境 ---------------------------------------------------------------
$python = Find-Python
if (-not $python) {
    Say '  找不到 python（可用环境变量 COMFYHUB_PYTHON 指定）。' 'Red'
    exit 1
}
Say "  python: $python" 'DarkGray'

try {
    $healthBefore = Invoke-Api 'GET' '/api/health' $null
} catch {
    $healthBefore = $null
}
if (-not $healthBefore -or $healthBefore.database -ne 'ok') {
    Say '  后端没在跑，先执行 scripts\comfyhub.ps1 up …' 'Yellow'
    & pwsh -NoProfile -File (Join-Path $PSScriptRoot 'comfyhub.ps1') up -SkipBuild | Out-Null
    try { $healthBefore = Invoke-Api 'GET' '/api/health' $null } catch { $healthBefore = $null }
}
Check '后端可用（db=ok）' ($null -ne $healthBefore -and $healthBefore.database -eq 'ok')

if ($script:Failed -gt 0) { exit 1 }

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
Get-ChildItem $OutDir -File -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue

# --- 0.5 先把**上一轮遗留的测试数据**清掉 -----------------------------------
#
# 为什么必须在开头清（而不只是在 finally 里清）：这个脚本用的 run_key 是固定的
# （假 ComfyUI 的 prompt_id = `e2e-capture-0001` / 推送用的 `e2e-push-0001`），
# 而 `CaptureRepo.beginRun` 对 `success` 的记录**永远不再抢占**（那是幂等的正确行为）。
# 于是只要上一轮被 Ctrl+C 打断过（清理在 finally 里，没跑到），这一轮的第 1 幕就会
# 「/history 明明有一条、轮询却 checked=0」—— 表现为"捕获全挂"，但根因只是一个残留行。
# 这个坑在 `docs/pitfalls.md` 里记着，脚本自己先兜住它。
$script:Mysql = Find-MysqlExe
if ($script:Mysql) {
    $ownKeys = "'e2e-capture-0001'", "'e2e-push-0001'"
    # 三张测试图先按同样的方式生成一遍，算出 hash —— 判重是按 **sha256** 走的
    # （`media_assets.sha256`），上一轮没清干净的产物会让这一轮的"目录导入"全都判成重复：
    # 表现是 `imported=0 duplicates=2`，而目录里其实只有一份是该重复的。
    foreach ($spec in @(@($capturePng, '30,90,180'), @($importPng, '200,120,30'), @($pushPng, '20,200,120'))) {
        & $python (Join-Path $E2eDir 'make_png.py') $spec[0] --color $spec[1] 2>&1 | Out-Null
    }
    $staleHashes = @()
    foreach ($f in @($capturePng, $importPng, $pushPng)) {
        if (Test-Path $f) {
            $sha = (Get-FileHash -Algorithm SHA256 -Path $f).Hash.ToLower()
            $staleHashes += "'$sha'"
            # 目录导入那条路还会留一条 `import:<sha 前 40 位>` 的运行记录
            $ownKeys += "'import:$($sha.Substring(0, 40))'"
        }
    }
    $staleMedia = 0
    if ($staleHashes.Count -gt 0) {
        $ids = & $script:Mysql --host=127.0.0.1 --port=3307 --user=comfyhub --password=comfyhub `
            --database=comfy_hub -N -B -e "SELECT GROUP_CONCAT(id) FROM media_assets WHERE sha256 IN ($($staleHashes -join ','));" 2>$null
        if ($ids -and "$ids" -ne 'NULL') {
            foreach ($id in ("$ids" -split ',')) {
                if (-not $id) { continue }
                try { Invoke-Api 'DELETE' "/api/media/$id" $null | Out-Null; $staleMedia++ } catch { }
            }
        }
    }
    $keyList = $ownKeys -join ','
    $stalePrompts = & $script:Mysql --host=127.0.0.1 --port=3307 --user=comfyhub --password=comfyhub `
        --database=comfy_hub -N -B -e "SELECT GROUP_CONCAT(id) FROM prompts WHERE source_ref IN ($keyList);" 2>$null
    $staleCount = 0
    if ($stalePrompts -and "$stalePrompts" -ne 'NULL') {
        foreach ($id in ("$stalePrompts" -split ',')) {
            if (-not $id) { continue }
            try { Invoke-Api 'DELETE' "/api/prompts/$id" $null | Out-Null; $staleCount++ } catch { }
        }
    }
    & $script:Mysql --host=127.0.0.1 --port=3307 --user=comfyhub --password=comfyhub `
        --database=comfy_hub -e "DELETE FROM capture_runs WHERE run_key IN ($keyList);" 2>&1 | Out-Null
    if ($staleCount -gt 0 -or $staleMedia -gt 0) {
        Say "  已清掉上一轮遗留的 $staleCount 条测试提示词 / $staleMedia 个产物与运行记录" 'DarkGray'
    }
    # 探针用的三张图删掉：它们是**按需**生成的（act 3 / act 4 各自造自己那张），
    # 留下来会让 act 3 的"扫描到 2 个文件"变成 3 个。
    foreach ($f in @($capturePng, $importPng, $pushPng)) {
        Remove-Item $f -Force -ErrorAction SilentlyContinue
    }
}

$capturePng = Join-Path $OutDir 'e2e_capture_00001_.png'
$importPng  = Join-Path $OutDir 'e2e_import_00002_.png'
$pushPng    = Join-Path $OutDir 'e2e_push_00003_.png'

$oldConfig = $null
$fakeComfy = $null
$pushPromptId = $null

try {
    # --- 1. 备份配置，并用「手动轮询」模式跑，避免后台轮询把结果抢走 -------
    $oldConfig = Invoke-Api 'GET' '/api/capture/config' $null
    Say ("  原配置: enabled={0} comfyUrl={1} outputDir={2}" -f $oldConfig.enabled, $oldConfig.comfyUrl, $oldConfig.outputDir) 'DarkGray'

    $testConfig = @{
        enabled          = $false
        comfyUrl         = "http://127.0.0.1:$ComfyPort"
        outputDir        = $OutDir
        pollSeconds      = $oldConfig.pollSeconds
        autoTag          = 'ComfyUI'
        maxPerPoll       = $oldConfig.maxPerPoll
        downloadFallback = $true
    }
    Invoke-Api 'PUT' '/api/capture/config' $testConfig | Out-Null

    # --- 2. 起假 ComfyUI + 造一张带元数据的 PNG ---------------------------
    & $python (Join-Path $E2eDir 'make_png.py') $capturePng --color '30,90,180' | Out-Null
    Check '生成测试 PNG（带 prompt/workflow 元数据）' (Test-Path $capturePng)

    $fakeComfy = Start-Process -FilePath $python -PassThru -WindowStyle Hidden `
        -ArgumentList @((Join-Path $E2eDir 'fake_comfy.py'), '--port', "$ComfyPort", '--output', $OutDir)

    $ready = $false
    foreach ($i in 1..40) {
        try {
            $h = Invoke-RestMethod "http://127.0.0.1:$ComfyPort/history" -TimeoutSec 2
            if ($h) { $ready = $true; break }
        } catch { Start-Sleep -Milliseconds 300 }
    }
    Check "假 ComfyUI 已就绪（127.0.0.1:$ComfyPort）" $ready

    # 「连得上」不等于「连的是我们起的那个」：端口被真实 ComfyUI 占着时，上面那句就绪检查
    # 会**对着它**返回成功，然后整场测试都在读陌生人的 /history —— 现象是"轮询啥也没收到",
    # 根因却在一个跟代码无关的地方。所以这里认一下身份，不认就当场停下。
    $isOurs = $false
    try {
        $root = Invoke-RestMethod "http://127.0.0.1:$ComfyPort/" -TimeoutSec 3
        $isOurs = ("$($root.app)" -eq 'fake-comfyui')
    } catch { }
    Check "端口 $ComfyPort 上是我们起的假 ComfyUI（不是真实 ComfyUI）" $isOurs `
        '多半是真实 ComfyUI 占着这个端口，用 -ComfyPort <空闲端口> 重跑'
    if (-not $isOurs) { throw "端口 $ComfyPort 被别的服务占用（不是 fake-comfyui）" }

    $status = Invoke-Api 'GET' '/api/capture/status' $null
    Check '状态接口能连上假 ComfyUI' ($status.comfyReachable -eq $true)

    # --- 3. 轮询捕获 ------------------------------------------------------
    Say ''
    Say '  [1/4] 轮询捕获' 'Cyan'
    $poll1 = Invoke-Api 'POST' '/api/capture/poll' $null
    Check '发现并捕获 1 次运行' ($poll1.ok -and $poll1.newRuns -eq 1) ($poll1 | ConvertTo-Json -Compress)
    Check '入库 1 个产物' ($poll1.newMedia -eq 1) ($poll1 | ConvertTo-Json -Compress)

    $found = Invoke-Api 'GET' '/api/prompts?q=e2e_capture&size=5' $null
    $prompt = $found.items | Select-Object -First 1
    Check '自动建了提示词' ($null -ne $prompt)
    if ($prompt) {
        $script:CreatedPromptIds.Add([long]$prompt.id)
        Check '标题带上了文件名前缀与 seed' ($prompt.title -like 'e2e_capture*') $prompt.title
        Check '正向提示词解析正确' ($prompt.positivePrompt -like '*neon alley at night*') $prompt.positivePrompt
        Check '负向提示词解析正确' ($prompt.negativePrompt -like '*watermark*') $prompt.negativePrompt
        Check 'checkpoint 解析正确' ($prompt.checkpoint -eq 'e2e_sdxl_base_1.0.safetensors') $prompt.checkpoint
        Check '采样器 / 调度器解析正确' ($prompt.sampler -eq 'dpmpp_2m' -and $prompt.scheduler -eq 'karras') "$($prompt.sampler)/$($prompt.scheduler)"
        Check 'steps=24 cfg=6.5 seed=20260101' ($prompt.steps -eq 24 -and $prompt.cfgScale -eq 6.5 -and $prompt.seed -eq 20260101) "$($prompt.steps)/$($prompt.cfgScale)/$($prompt.seed)"
        Check '宽高 / 批量解析正确' ($prompt.width -eq 1024 -and $prompt.height -eq 1024 -and $prompt.batchSize -eq 2) "$($prompt.width)x$($prompt.height)x$($prompt.batchSize)"
        Check 'LoRA 解析正确（名字 + 权重）' ($prompt.loras.Count -eq 1 -and $prompt.loras[0].name -eq 'e2e_detail_tweaker.safetensors' -and [double]$prompt.loras[0].weight -eq 0.8) ($prompt.loras | ConvertTo-Json -Compress)
        Check '自动打了 ComfyUI 标签' (($prompt.tags | ForEach-Object { $_.name }) -contains 'ComfyUI')
        Check '来源标记为 ComfyUI' ($prompt.source -eq 'ComfyUI') $prompt.source
        Check '标记了「有工作流」' ($prompt.hasWorkflow -eq $true)

        $wf = Invoke-WebRequest "$ApiBase/api/prompts/$($prompt.id)/workflow" -TimeoutSec 30
        Check '能取到完整工作流 JSON' ($wf.StatusCode -eq 200 -and $wf.Content -like '*e2e-node*')
        # 存的是紧凑 JSON（展示时由前端的查看器格式化），所以这里用正则容忍空白
        Check '工作流原文里的节点类型完好' ($wf.Content -match '"type"\s*:\s*"KSampler"')
        Check '工作流原文里的中文/非 ASCII 没有乱码' ($wf.Content -notmatch '\\u[0-9a-fA-F]{4}')

        $media = Invoke-Api 'GET' "/api/prompts/$($prompt.id)/media" $null
        Check '产物已关联到该提示词' ($media.Count -eq 1)
        if ($media.Count -ge 1) {
            $m = $media[0]
            $script:CreatedMediaIds.Add([long]$m.id)
            Check '产物类型 = IMAGE' ($m.kind -eq 'IMAGE') $m.kind
            Check '产物尺寸探测正确（1024x1024）' ($m.width -eq 1024 -and $m.height -eq 1024) "$($m.width)x$($m.height)"
            Check '产物来源标记正确' ($m.source -eq 'ComfyUI')
        }
    }

    # --- 4. 幂等 ----------------------------------------------------------
    Say ''
    Say '  [2/4] 重复轮询应当幂等' 'Cyan'
    $poll2 = Invoke-Api 'POST' '/api/capture/poll' $null
    Check '第二次轮询没有重复入库' ($poll2.ok -and $poll2.newRuns -eq 0 -and $poll2.newMedia -eq 0) ($poll2 | ConvertTo-Json -Compress)

    # --- 5. 目录导入 ------------------------------------------------------
    Say ''
    Say '  [3/4] 导入已经生成好的 PNG' 'Cyan'
    & $python (Join-Path $E2eDir 'make_png.py') $importPng --color '200,120,30' | Out-Null
    $import = Invoke-Api 'POST' '/api/capture/import' @{
        dir = $OutDir; recursive = $true; limit = 50; linkWorkflow = $true; tags = @('导入测试')
    }
    Check '扫描到 2 个文件' ($import.scanned -ge 2) ($import | ConvertTo-Json -Compress)
    Check '新导入 1 个（另一个按 SHA-256 判重跳过）' ($import.imported -eq 1 -and $import.duplicates -ge 1) ($import | ConvertTo-Json -Compress)
    Check '从 PNG 元数据自动建了 1 条提示词' ($import.promptsCreated -eq 1) ($import | ConvertTo-Json -Compress)
    foreach ($id in $import.promptIds) { $script:CreatedPromptIds.Add([long]$id) }

    if ($import.promptIds.Count -ge 1) {
        $p2 = Invoke-Api 'GET' "/api/prompts/$($import.promptIds[0])" $null
        Check '导入的提示词也带上了完整参数' ($p2.seed -eq 20260101 -and $p2.steps -eq 24 -and $p2.hasWorkflow -eq $true)
        Check '导入来源标记正确' ($p2.source -eq 'ComfyUI-Import') $p2.source
        $m2 = Invoke-Api 'GET' "/api/prompts/$($p2.id)/media" $null
        foreach ($x in $m2) { $script:CreatedMediaIds.Add([long]$x.id) }
    }

    # --- 6. 推送捕获（自定义节点走的路） ----------------------------------
    Say ''
    Say '  [4/4] 推送捕获 POST /api/ingest/comfyui' 'Cyan'
    & $python (Join-Path $E2eDir 'make_png.py') $pushPng --color '20,200,120' | Out-Null
    $pushBody = @{
        runKey    = 'e2e-push-0001'
        source    = 'ComfyUI'
        status    = 'success'
        comfyUrl  = "http://127.0.0.1:$ComfyPort"
        outputDir = $OutDir
        prompt    = $null
        workflow  = $null
        outputs   = @(@{ filename = 'e2e_push_00003_.png'; subfolder = ''; type = 'output'; kind = 'IMAGE'; nodeType = 'SaveImage' })
        extra     = @{ title = 'e2e 推送捕获'; tags = @('推送测试') }
    }
    $push = Invoke-Api 'POST' '/api/ingest/comfyui' $pushBody
    Check '推送捕获成功' ($push.created -eq $true -and $push.imported -eq 1) ($push | ConvertTo-Json -Compress)
    if ($push.promptId) {
        $pushPromptId = [long]$push.promptId
        $script:CreatedPromptIds.Add($pushPromptId)
    }
    foreach ($id in $push.mediaIds) { $script:CreatedMediaIds.Add([long]$id) }

    $push2 = Invoke-Api 'POST' '/api/ingest/comfyui' $pushBody
    Check '同一 runKey 再次推送不会重复' ($push2.alreadyCaptured -eq $true) ($push2 | ConvertTo-Json -Compress)

    $st = Invoke-Api 'GET' '/api/capture/status' $null
    Check '状态里能看到最近的捕获记录' ($st.recent.Count -ge 2) ($st.recent.Count)
} finally {
    # --- 清理 -------------------------------------------------------------
    Say ''
    Say '  清理测试数据' 'Cyan'
    if ($fakeComfy) {
        Stop-Process -Id $fakeComfy.Id -Force -ErrorAction SilentlyContinue
        Say '  已停止假 ComfyUI' 'DarkGray'
    }
    if ($oldConfig) {
        try {
            Invoke-Api 'PUT' '/api/capture/config' @{
                enabled          = $oldConfig.enabled
                comfyUrl         = $oldConfig.comfyUrl
                outputDir        = $oldConfig.outputDir
                pollSeconds      = $oldConfig.pollSeconds
                autoTag          = $oldConfig.autoTag
                maxPerPoll       = $oldConfig.maxPerPoll
                downloadFallback = $oldConfig.downloadFallback
            } | Out-Null
            Say '  已还原自动捕获配置' 'DarkGray'
        } catch { Say "  还原配置失败: $($_.Exception.Message)" 'Yellow' }
    }

    if ($KeepData) {
        Say '  -KeepData：保留了测试产生的数据' 'Yellow'
    } else {
        foreach ($id in ($script:CreatedMediaIds | Select-Object -Unique)) {
            try { Invoke-Api 'DELETE' "/api/media/$id" $null | Out-Null } catch { }
        }
        foreach ($id in ($script:CreatedPromptIds | Select-Object -Unique)) {
            try { Invoke-Api 'DELETE' "/api/prompts/$id" $null | Out-Null } catch { }
        }
        Say ("  已删除 {0} 个产物 / {1} 条提示词" -f ($script:CreatedMediaIds | Select-Object -Unique).Count, ($script:CreatedPromptIds | Select-Object -Unique).Count) 'DarkGray'

        # 运行记录只能从库里删（顺便清掉导入产生的 import:<sha> 记录）
        $mysql = Find-MysqlExe
        if ($mysql) {
            $keys = @("'e2e-capture-0001'", "'e2e-push-0001'")
            foreach ($f in @($capturePng, $importPng, $pushPng)) {
                if (Test-Path $f) {
                    $sha = (Get-FileHash $f -Algorithm SHA256).Hash.ToLower()
                    $keys += "'import:$($sha.Substring(0, 40))'"
                }
            }
            $sql = "DELETE FROM capture_runs WHERE run_key IN ($($keys -join ','))"
            & $mysql --protocol=TCP -h 127.0.0.1 -P 3307 -u comfyhub --password=comfyhub comfy_hub -e $sql 2>&1 | Out-Null
            Say '  已清理测试产生的运行记录' 'DarkGray'
        }
        Remove-Item $OutDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Say ''
if ($script:Failed -gt 0) {
    Say ("  ✗ 有 $script:Failed 项没通过，请看上面的 [FAIL]") 'Red'
    exit 1
}
Say '  ✓ 全部通过：轮询捕获 / 幂等 / 目录导入 / 推送捕获' 'Green'
Say ''
exit 0
