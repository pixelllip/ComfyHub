<#
.SYNOPSIS
    ComfyHub「工作流配方（学 → 列 → 绑）」端到端自测（**Rust 后端**；不花钱、不出网）。

.DESCRIPTION
    用户 2026-09-26 的问题："如果以后出了新的模型，如何让 AI 自主学习新模型泛用/特定工作流的搭建？"
    落地的东西是三条工具 + 四条端点（设计见 docs/workflow-recipe-design.md）。这个脚本把那条闭环
    用**真 HTTP + 真库副本 + 真工作流文件**走一遍：

      1. 学一份**API 格式**工作流（`-ApiWorkflow` 指定） —— 零转换，必须 complete=true
      2. 幂等                                     —— 同一份文件学两次只有一行、同一个 id
      3. 学库里的提示词（promptId）                —— 从 capture_runs.raw 里那份 API 图学
      4. 用户真实的**界面格式**工作流（可选场景）  —— ComfyUI 在场时验"有缺口就拒学"
      5. 列表（task / q / skeleton 过滤）          —— 同一族 = 同一个骨架指纹
      6. 详情                                     —— 带 render 正文（能直接照抄 overrides）
      7. **绑定**（只报不改）                      —— 四档结论；**不是 exact 就一个键都不许写**
      8. 错误分支                                 —— 稳定码而不是 500 / ToolFailure
      9. 删除

    三条纪律（脚本里逐条断言）：
      · **配方不是可提交对象**：绑定只产出 overrides，提交仍走 comfy_submit（R10）；
      · **不许学一条缺了链的骨架**：界面格式有节点转不出来时默认 400 UNSUPPORTED_NODES；
      · **不许编模型名**：非 exact 时 overrides 必须为空。

    **不碰真库**：库是 `.db\comfyhub.db` 的临时副本，产物目录也是临时目录，跑完删掉。

.EXAMPLE
    pwsh -File scripts\e2e-recipe-test.ps1
    pwsh -File scripts\e2e-recipe-test.ps1 -KeepData          # 留着临时库看现场
    pwsh -File scripts\e2e-recipe-test.ps1 -UiWorkflow '<某个目录>\krea2ImageEditingAnd_v10.json'
#>
[CmdletBinding()]
param(
    # 监听端口；0 = 自动挑一个空闲的
    [int]$Port = 0,
    # 实际使用的**界面格式**工作流（默认去 ComfyUI 的 workflows 目录找 krea2 编辑那两份）；
    # 找不到 / ComfyUI 不在跑就跳过第 4 幕（不判失败）
    [string]$UiWorkflow = '',
    # 第 1 幕要学的那份 **API 格式**工作流。仓库不再随包带模板（2026-10-28 删除），
    # 必须显式给一份（可以是你自己在 ComfyUI 里存的、或官方模板导出成 API 的那份）
    [string]$ApiWorkflow = '',
    [switch]$KeepData
)

$ErrorActionPreference = 'Continue'
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$WorkDir = Join-Path $ProjectRoot '.run\e2e-recipe'
# 「用哪个后端」只有一个真源：`scripts\backend.ps1` 的 Resolve-Backend，
# App 那份在 `lib\core\backend_paths.dart`（AGENTS §7，改顺序两边一起改）
. (Join-Path $PSScriptRoot 'backend.ps1')

$script:Pass = 0
$script:Fail = 0

function Assert-That {
    param([string]$Name, [bool]$Ok, [string]$Detail = '')
    if ($Ok) {
        $script:Pass++
        Write-Host ("  [ok]   " + $Name) -ForegroundColor DarkGreen
    } else {
        $script:Fail++
        Write-Host ("  [FAIL] " + $Name) -ForegroundColor Red
        if ($Detail) { Write-Host ("         " + $Detail.Substring(0, [Math]::Min(500, $Detail.Length))) -ForegroundColor DarkGray }
    }
}

function Convert-JsonOrNull {
    # ⚠ PowerShell 里 `try/catch` 是**语句**不是表达式：不能写在哈希表字面量里
    # （写成 `@{ json = (try { … } catch { … }) }` 会把 `try` 当作命令名，整段直接失败）
    param([string]$Text)
    if ([string]::IsNullOrWhiteSpace($Text)) { return $null }
    try { return $Text | ConvertFrom-Json } catch { return $null }
}

function Invoke-Api {
    param([string]$Method, [string]$Uri, $Body = $null)
    $args = @{ Method = $Method; Uri = $Uri; TimeoutSec = 60; UseBasicParsing = $true }
    if ($null -ne $Body) {
        $args.ContentType = 'application/json; charset=utf-8'
        $args.Body = [Text.Encoding]::UTF8.GetBytes(($Body | ConvertTo-Json -Depth 10 -Compress))
    }
    try {
        $r = Invoke-WebRequest @args
        return @{ status = [int]$r.StatusCode; raw = $r.Content; json = (Convert-JsonOrNull $r.Content) }
    } catch {
        $resp = $_.Exception.Response
        $status = if ($resp) { [int]$resp.StatusCode } else { 0 }
        $body = if ($_.ErrorDetails) { $_.ErrorDetails.Message } else { $_.Exception.Message }
        return @{ status = $status; raw = $body; json = (Convert-JsonOrNull $body) }
    }
}

# --- 用哪个后端：**这个脚本只认 Rust** -----------------------------------
# 它验的是 Rust 工具层/端点（`workflow_recipe.rs`）。Kotlin 已退役（C-6），
# 所以找不到 Rust 二进制时**明确报错退出**，绝不悄悄回退（回退了就是拿另一套实现
# 证明了另一件事，而那正是 e2e 最不能有的假绿）。
$backend = Resolve-Backend -ProjectRoot $ProjectRoot
if ($backend.Kind -ne 'rust' -or -not $backend.Exe) {
    Write-Host "找不到 Rust 后端二进制（$($backend.Note)）" -ForegroundColor Red
    Write-Host '先编：cargo build --release --manifest-path server-rs\Cargo.toml' -ForegroundColor Yellow
    exit 2
}
Write-Host "后端：$($backend.Exe)（$($backend.Note)）" -ForegroundColor DarkGray

# --- **构建新鲜度**：跑的必须是刚编出来的产物 -----------------------------
# 不查这一条会得到假红的"e2e 抓到了 bug"：实测踩过 —— 改了源码没重编，
# e2e 报的一堆 FAIL 全是**旧二进制**的行为（AGENTS §1 那条"改了但界面没变"的后端版）。
# 判据是 `scripts\backend.ps1` 的 `Get-RustBuildFreshness`（同一份函数）。
$fresh = Get-RustBuildFreshness -Exe $backend.Exe -ProjectRoot $ProjectRoot
if ($fresh -and $fresh.Stale) {
    Write-Host "后端产物比源码旧（exe $($fresh.ExeTime) < $($fresh.NewestSource.Name) $($fresh.NewestSource.LastWriteTime)）" -ForegroundColor Red
    Write-Host '先重编：cargo build --release --manifest-path server-rs\Cargo.toml' -ForegroundColor Yellow
    exit 2
}

# --- 端口：0 = 自动挑一个空闲的 -----------------------------------------
if ($Port -eq 0) {
    $l = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $l.Start(); $Port = $l.LocalEndpoint.Port; $l.Stop()
}
$base = "http://127.0.0.1:$Port"

# --- 临时现场：真库副本 + 空 storage ------------------------------------
# ⚠ 先清掉**上一次跑剩下的**后端（跑挂了就会有残留，它会占着库副本的 WAL/shm，
#    下一次 `Copy-Item` 会报文件被占用的错误——已复现）。
#    只认**命令行里带这个 WorkDir 的**那些，不终止正在运行的后端。
function Stop-Tree {
    param([int]$ProcessId)
    if ($ProcessId -le 0) { return }
    & taskkill /PID $ProcessId /T /F 2>&1 | Out-Null
}
Get-CimInstance Win32_Process -Filter "Name='comfyhub-server-rs.exe'" -ErrorAction SilentlyContinue |
    Where-Object { $_.CommandLine -and $_.CommandLine -like "*$WorkDir*" } |
    ForEach-Object { Stop-Tree $_.ProcessId }

if (Test-Path $WorkDir) { Remove-Item $WorkDir -Recurse -Force -ErrorAction SilentlyContinue }
New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null
# 数据目录可能被移动到其他磁盘（`<根>\data-dir.json`，与 lib\core\backend_paths.dart 同一约定）
$DataDir = $ProjectRoot
$DataDirFile = Join-Path $ProjectRoot 'data-dir.json'
if (Test-Path $DataDirFile) {
    try { $customDataDir = (Get-Content $DataDirFile -Raw -Encoding UTF8 | ConvertFrom-Json).dataDir } catch { $customDataDir = '' }
    if ($customDataDir) { $DataDir = $customDataDir }
}
$realDb = Join-Path $DataDir '.db\comfyhub.db'
$hasRealDb = Test-Path $realDb
$dbCopy = Join-Path $WorkDir 'comfyhub.db'
if ($hasRealDb) {
    Copy-Item $realDb $dbCopy -Force
    foreach ($suffix in @('-wal', '-shm')) {
        if (Test-Path "$realDb$suffix") { Copy-Item "$realDb$suffix" "$dbCopy$suffix" -Force }
    }
    Write-Host "库副本：$dbCopy（真库只读未动）" -ForegroundColor DarkGray
} else {
    Write-Host "没有真库（$realDb）——用空库跑：第 3 幕（学库里的提示词）会跳过" -ForegroundColor DarkGray
}

$server = $null
try {
    # --- 起后端（Rust；命令行指向库副本，环境变量指向项目根） -------------
    # Rust 用**命令行参数**（`--db` / `--storage` / `--port`），不再是 Kotlin 那套环境变量。
    $env:COMFYHUB_PROJECT_ROOT = $ProjectRoot
    # ⚠ 绝不让它写 `<根>\.run\api.json`：该文件属于正在运行的后端，
    #   被这个临时后端覆盖掉的话，App 下次会去连一个已经关掉的端口（pitfalls 记过现场）。
    $env:COMFYHUB_NO_PORT_FILE = '1'
    $storageDir = Join-Path $WorkDir 'storage'
    New-Item -ItemType Directory -Force -Path $storageDir | Out-Null

    $server = Start-Process -FilePath $backend.Exe -PassThru -WindowStyle Hidden `
        -ArgumentList @('--db', $dbCopy, '--storage', $storageDir, '--port', "$Port") `
        -RedirectStandardOutput (Join-Path $WorkDir 'server.out.log') `
        -RedirectStandardError (Join-Path $WorkDir 'server.err.log')

    $ready = $false
    for ($i = 0; $i -lt 120; $i++) {
        Start-Sleep -Milliseconds 250
        if ($server.HasExited) { break }
        $h = Invoke-Api -Method GET -Uri "$base/api/health"
        if ($h.status -eq 200) { $ready = $true; break }
    }
    if (-not $ready) {
        Write-Host "后端没起来（端口 $Port）——看 $WorkDir\server.err.log" -ForegroundColor Red
        if (Test-Path (Join-Path $WorkDir 'server.err.log')) { Get-Content (Join-Path $WorkDir 'server.err.log') -Tail 20 }
        throw '后端没起来'
    }
    Write-Host "后端就绪：$base（库副本 $dbCopy）" -ForegroundColor DarkGray

    # --- 1. 学一份 API 格式工作流 --------------------------------------
    Write-Host "`n[1] 学一份 API 格式工作流（零转换，必须 complete=true）" -ForegroundColor Cyan
    if (-not $ApiWorkflow) {
        Write-Host "  仓库不再随包带模板工作流（2026-10-28 删除）。请用 -ApiWorkflow <path> 指定一份 API 格式工作流。" -ForegroundColor Red
        throw '缺 -ApiWorkflow'
    }
    $apiFile = $ApiWorkflow
    Assert-That '指定的 API 工作流在' (Test-Path $apiFile) $apiFile
    $learn1 = Invoke-Api -Method POST -Uri "$base/api/ai/workflow-recipes" -Body @{ path = $apiFile }
    Assert-That '学习返回 201' ($learn1.status -eq 201) "$($learn1.status) $($learn1.raw)"
    $recipe1 = $learn1.json
    Assert-That 'task=inpaint（遮罩链判出来的）' ($recipe1.task -eq 'inpaint') "$($recipe1.task)"
    Assert-That 'complete=true（API 格式零转换）' ($recipe1.complete -eq $true) "$($recipe1.complete)"
    Assert-That '认出了 3 个模型槽位' ($recipe1.slotCount -eq 3) "$($recipe1.slotCount)"
    Assert-That '骨架指纹是 16 位' ($recipe1.skeletonHash.Length -eq 16) "$($recipe1.skeletonHash)"
    Assert-That 'render 里有"改哪个键"' ($recipe1.render -like '*1.ckpt_name =*') $recipe1.render
    Assert-That 'format=api' ($recipe1.format -eq 'api') "$($recipe1.format)"

    # --- 2. 幂等 --------------------------------------------------------
    Write-Host "`n[2] 幂等：同一份文件学两次只有一行" -ForegroundColor Cyan
    $learn1b = Invoke-Api -Method POST -Uri "$base/api/ai/workflow-recipes" -Body @{ path = $apiFile }
    Assert-That '第二次还是同一个 recipeId' ($learn1b.json.recipeId -eq $recipe1.recipeId) "$($learn1b.json.recipeId) vs $($recipe1.recipeId)"
    $list0 = Invoke-Api -Method GET -Uri "$base/api/ai/workflow-recipes?skeleton=$($recipe1.skeletonHash)"
    # ⚠ 按**自己这一族的骨架**过滤，不数全库：库副本里本来就可能躺着别的配方
    #   （2026-10-17 实测：真实库里学过的 krea2 两份会让"全库只有 1 条"变成 3 条而假红）
    Assert-That '这一族只有 1 条' (@($list0.json).Count -eq 1) "$(@($list0.json).Count)"

    # --- 3. 学库里的提示词（promptId） ---------------------------------
    Write-Host "`n[3] 学库里的提示词（从 capture_runs.raw 那份 API 图学）" -ForegroundColor Cyan
    $learnedFromPrompt = $false
    if ($hasRealDb) {
        $prompts = (Invoke-Api -Method GET -Uri "$base/api/prompts?size=40").json
        $ids = @()
        if ($prompts.items) { $ids = @($prompts.items) | ForEach-Object { $_.id } }
        foreach ($id in $ids) {
            $g = Invoke-Api -Method GET -Uri "$base/api/prompts/$id/api-graph"
            if ($g.status -eq 200) {
                $learn2 = Invoke-Api -Method POST -Uri "$base/api/ai/workflow-recipes" -Body @{ promptId = $id }
                Assert-That "promptId=$id 能学成配方（201）" ($learn2.status -eq 201) "$($learn2.status) $($learn2.raw)"
                if ($learn2.status -eq 201) {
                    Assert-That 'sourceKind=prompt' ($learn2.json.sourceKind -eq 'prompt') "$($learn2.json.sourceKind)"
                    Assert-That '带 promptId（将来提交要用它）' ($learn2.json.promptId -eq $id) "$($learn2.json.promptId)"
                    $learnedFromPrompt = $true
                }
                break
            }
        }
    }
    if (-not $learnedFromPrompt) { Write-Host '  [skip] 没有可用的 API 图提示词（空库 / 老数据）' -ForegroundColor DarkGray }

    # --- 4. 界面格式 + 有缺口 → 默认拒学（要 ComfyUI 在场） -------------
    Write-Host "`n[4] 界面格式有缺口时默认拒学（真机上最常见的原因：实例没装那些自定义节点）" -ForegroundColor Cyan
    if (-not $UiWorkflow) {
        # 工作流目录从"**现探到的** ComfyUI 位置"推出来，不写死某个盘：
        # 以前这里是 `D:\Comfy-Desktop\...\user\default\workflows`，换台机器就找不到。
        if (-not (Get-Command Resolve-ComfyHome -ErrorAction SilentlyContinue)) {
            . (Join-Path $PSScriptRoot 'comfy-path.ps1')
        }
        $comfyHomeForRecipe = Resolve-ComfyHome -ProjectRoot $ProjectRoot
        if ($comfyHomeForRecipe) {
            $wfDir = Join-Path $comfyHomeForRecipe 'user\default\workflows'
            foreach ($pattern in @('*ImageEdit*.json', 'krea2*.json')) {
                $cand = Get-ChildItem -LiteralPath $wfDir -Filter $pattern -ErrorAction SilentlyContinue |
                    Select-Object -First 1
                if ($cand) { $UiWorkflow = $cand.FullName; break }
            }
        }
    }
    $comfyUp = $false
    try { $comfyUp = (Invoke-Api -Method GET -Uri 'http://127.0.0.1:8188/system_stats').status -eq 200 } catch { $comfyUp = $false }
    $hasUiFile = $UiWorkflow -and (Test-Path $UiWorkflow)
    if (-not $comfyUp -and $hasUiFile) {
        # ComfyUI 不在跑时，界面格式照样**不许**假装学得会：要如实报"转换需要 /object_info"
        $offline = Invoke-Api -Method POST -Uri "$base/api/ai/workflow-recipes" -Body @{ path = $UiWorkflow }
        Assert-That '界面格式 + ComfyUI 不在跑 → 400 COMFY_UNREACHABLE' (
            $offline.status -eq 400 -and $offline.json.error -eq 'COMFY_UNREACHABLE'
        ) "$($offline.status) $($offline.raw)"
        Assert-That '并告诉用户两条出路（起 ComfyUI / 导出 API 格式）' (
            $offline.json.detail -like '*comfy_get_status*' -or $offline.json.detail -like '*导出*'
        ) $offline.json.detail
        Write-Host '  [skip] 第 4 幕的"有缺口就拒学"要 ComfyUI 在场（本次已改验"连不上时如实报"）' -ForegroundColor DarkGray
    } elseif (-not $comfyUp) {
        Write-Host '  [skip] ComfyUI 不在跑（界面格式转换需要 /object_info）' -ForegroundColor DarkGray
    } elseif (-not $hasUiFile) {
        Write-Host '  [skip] 没找到界面格式的工作流文件（可用 -UiWorkflow 指定）' -ForegroundColor DarkGray
    } else {
        Write-Host "  用：$UiWorkflow" -ForegroundColor DarkGray
        $strict = Invoke-Api -Method POST -Uri "$base/api/ai/workflow-recipes" -Body @{ path = $UiWorkflow }
        if ($strict.status -eq 201) {
            # 这份文件恰好没有缺口（转换全过）——那也是一种正确结果，如实记一句
            Write-Host "  [note] 这份文件没有转换缺口（complete=$($strict.json.complete)），第 4 幕按「零缺口」通过" -ForegroundColor DarkGray
            Assert-That '没有缺口时 complete=true' ($strict.json.complete -eq $true) "$($strict.json.complete)"
        } else {
            Assert-That '有缺口 → 400（默认拒学）' ($strict.status -eq 400) "$($strict.status) $($strict.raw)"
            Assert-That '错误码是 UNSUPPORTED_NODES' ($strict.json.error -eq 'UNSUPPORTED_NODES') "$($strict.json.error)"
            Assert-That '说清了"先确认本机实例有没有这些节点"' ($strict.json.detail -like '*本机这个实例*') $strict.json.detail
            $tol = Invoke-Api -Method POST -Uri "$base/api/ai/workflow-recipes" `
                -Body @{ path = $UiWorkflow; tolerateUnsupported = $true }
            Assert-That '带 tolerateUnsupported=true 才学（201）' ($tol.status -eq 201) "$($tol.status) $($tol.raw)"
            Assert-That '照样学但标 complete=false' ($tol.json.complete -eq $false) "$($tol.json.complete)"
            Assert-That '缺口清单非空' (@($tol.json.gaps).Count -gt 0) "$(@($tol.json.gaps).Count)"
        }
    }

    # --- 5. 列表 --------------------------------------------------------
    Write-Host "`n[5] 列表：task / q / skeleton 过滤" -ForegroundColor Cyan
    $byTask = Invoke-Api -Method GET -Uri "$base/api/ai/workflow-recipes?task=inpaint"
    Assert-That 'task=inpaint 能过滤' (@($byTask.json).Count -ge 1) "$(@($byTask.json).Count)"
    $byQ = Invoke-Api -Method GET -Uri "$base/api/ai/workflow-recipes?q=ckpt_name"
    Assert-That 'q 能搜到槽位里的键' (@($byQ.json).Count -ge 1) "$(@($byQ.json).Count)"
    $bySkeleton = Invoke-Api -Method GET -Uri "$base/api/ai/workflow-recipes?skeleton=$($recipe1.skeletonHash)"
    Assert-That 'skeleton 能过滤到同一族' (@($bySkeleton.json).Count -ge 1) "$(@($bySkeleton.json).Count)"

    # --- 6. 详情 --------------------------------------------------------
    Write-Host "`n[6] 详情（带 render 正文）" -ForegroundColor Cyan
    $detail = Invoke-Api -Method GET -Uri "$base/api/ai/workflow-recipes/$($recipe1.recipeId)"
    Assert-That '详情 200' ($detail.status -eq 200) "$($detail.status)"
    Assert-That '详情带 render' ($detail.json.render -like '*【工作流配方】*') $detail.json.render
    Assert-That '详情能还原槽位' (@($detail.json.recipe.slots).Count -eq 3) "$(@($detail.json.recipe.slots).Count)"

    # --- 7. 绑定（只报不改） --------------------------------------------
    Write-Host "`n[7] 绑定：四档结论，**不是 exact 就一个键都不许写**" -ForegroundColor Cyan
    $ownModel = $detail.json.recipe.slots | Where-Object { $_.overrideKey -eq '1.ckpt_name' } | Select-Object -First 1
    $bind = Invoke-Api -Method POST -Uri "$base/api/ai/workflow-recipes/$($recipe1.recipeId)/bind" `
        -Body @{ model = $ownModel.fileName }
    Assert-That '绑定 200' ($bind.status -eq 200) "$($bind.status) $($bind.raw)"
    $binding = @($bind.json.report.bindings)[0]
    Assert-That '结论是四档之一（不是含糊的一句话）' (
        @('exact', 'onDiskOnly', 'substitute', 'missing', 'unchecked', 'notAModel', 'notInRecipe', 'ambiguous') -contains $binding.status
    ) "$($binding.status)"
    if ($binding.status -eq 'exact') {
        Assert-That 'exact 才写覆盖键' (
            $bind.json.report.overrides.'1.ckpt_name' -eq $ownModel.fileName
        ) "$($bind.json.report.overrides | ConvertTo-Json -Compress)"
    } else {
        Assert-That "非 exact（$($binding.status)）时 overrides 必须为空" (
            @($bind.json.report.overrides.PSObject.Properties).Count -eq 0
        ) "$($bind.json.report.overrides | ConvertTo-Json -Compress)"
    }
    Assert-That '绑定正文说清了结论与下一步' ($bind.json.render -like '*【把配方绑到模型上】*') $bind.json.render
    # 一个明显不存在的新模型名：绝不能悄悄写进去
    $fresh = Invoke-Api -Method POST -Uri "$base/api/ai/workflow-recipes/$($recipe1.recipeId)/bind" `
        -Body @{ model = 'zzz_not_a_real_model_20260926.safetensors' }
    $freshBinding = @($fresh.json.report.bindings)[0]
    Assert-That '不存在的新模型名：不写键' (
        @($fresh.json.report.overrides.PSObject.Properties).Count -eq 0
    ) "$($fresh.json.report.overrides | ConvertTo-Json -Compress)"
    Assert-That '并明说「绝不编文件名」或「先问用户」' (
        $freshBinding.note -like '*不要自己编*' -or $freshBinding.note -like '*先问用户*'
    ) $freshBinding.note

    # --- 8. 错误分支 ----------------------------------------------------
    Write-Host "`n[8] 错误分支：稳定码而不是 500 / ToolFailure" -ForegroundColor Cyan
    $outside = Invoke-Api -Method POST -Uri "$base/api/ai/workflow-recipes" -Body @{ path = 'C:\Windows\win.ini' }
    Assert-That '白名单外的路径 → 400' ($outside.status -eq 400) "$($outside.status) $($outside.raw)"
    Assert-That '不是 500 / ToolFailure' ($outside.json.error -ne 'ToolFailure') "$($outside.json.error)"
    $noGraph = Invoke-Api -Method POST -Uri "$base/api/ai/workflow-recipes" -Body @{ promptId = 999999 }
    Assert-That '不存在的 promptId → NO_API_GRAPH' ($noGraph.json.error -eq 'NO_API_GRAPH') "$($noGraph.raw)"
    $notWf = Invoke-Api -Method POST -Uri "$base/api/ai/workflow-recipes" -Body @{ path = (Join-Path $ProjectRoot 'README.md') }
    Assert-That '不是工作流的文件 → NOT_A_WORKFLOW' ($notWf.json.error -eq 'NOT_A_WORKFLOW') "$($notWf.raw)"
    $empty = Invoke-Api -Method POST -Uri "$base/api/ai/workflow-recipes" -Body @{}
    Assert-That '三个来源都不给 → INVALID_ARGUMENT' ($empty.json.error -eq 'INVALID_ARGUMENT') "$($empty.raw)"
    $noRecipe = Invoke-Api -Method POST -Uri "$base/api/ai/workflow-recipes/999999/bind" -Body @{ model = 'x.safetensors' }
    Assert-That '绑一条不存在的配方 → 404' ($noRecipe.status -eq 404) "$($noRecipe.status)"

    # --- 9. 删除 --------------------------------------------------------
    Write-Host "`n[9] 删除" -ForegroundColor Cyan
    $del = Invoke-Api -Method DELETE -Uri "$base/api/ai/workflow-recipes/$($recipe1.recipeId)"
    Assert-That '删除 200' ($del.status -eq 200) "$($del.status)"
    $after = Invoke-Api -Method GET -Uri "$base/api/ai/workflow-recipes?skeleton=$($recipe1.skeletonHash)"
    Assert-That '删掉之后那一族没了' (@($after.json).Count -eq 0) "$(@($after.json).Count)"
}
catch {
    # ⚠ **脚本自己崩了也必须算失败**：PowerShell 的非终止错误默认只打一行红字、
    #    然后接着往下走 —— 那样最后会印出"全部通过"，而实际上什么都没验
    #    （写这个脚本时就踩过一次：`try` 写进哈希表里，20 项"全绿"其实一项没跑）。
    $script:Fail++
    Write-Host ("  [FAIL] 脚本异常中断：" + $_.Exception.Message) -ForegroundColor Red
    Write-Host ("         " + $_.InvocationInfo.PositionMessage) -ForegroundColor DarkGray
}
finally {
    if ($server -and -not $server.HasExited) {
        # Rust 是单进程（没有 cmd 包 java 那一层），`/T` 留着是为了万一它有子进程（MCP 桥会起）
        Stop-Tree $server.Id
        Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue
    }
    foreach ($v in @('COMFYHUB_PROJECT_ROOT', 'COMFYHUB_NO_PORT_FILE')) {
        Remove-Item "Env:\$v" -ErrorAction SilentlyContinue
    }
    if (-not $KeepData -and (Test-Path $WorkDir)) {
        Start-Sleep -Milliseconds 300
        Remove-Item $WorkDir -Recurse -Force -ErrorAction SilentlyContinue
    } elseif ($KeepData) {
        Write-Host "临时现场保留在 $WorkDir" -ForegroundColor DarkGray
    }
}

Write-Host ''
# ★ **一条都没跑 = 没通过**（2026-09-30 补）：整段断言可能因为"后端没起 / 前置条件没过"
#   被静默跳过，而那时 Fail=0 —— 只看 Fail 就会输出"0 项全部通过"这种假绿
#   （e2e-submit 那边实测踩到过同一种；总纲 §26 记的也是这一族）。
if ($script:Pass -le 0) {
    Write-Host '  ✗ 一项检查都没跑（后端没起？前置条件没过？）—— 这不叫通过。' -ForegroundColor Red
    exit 1
}
if ($script:Fail -eq 0) {
    Write-Host "工作流配方 e2e：$($script:Pass) 项全部通过" -ForegroundColor Green
    exit 0
} else {
    Write-Host "工作流配方 e2e：$($script:Pass) 项通过，$($script:Fail) 项失败" -ForegroundColor Red
    exit 1
}
