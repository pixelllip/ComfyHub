<#
.SYNOPSIS
    ComfyHub「配对闸门 + 密钥下行」端到端自测（**Rust 后端**；不需要真的手机）。

.DESCRIPTION
    配对这件事有两半：**后端认不认令牌**、**人怎么把令牌交到手机手里**。这个脚本把两半
    都用真 HTTP 走一遍 —— 手机那一半用一台"假装是手机"的 HTTP 客户端（不需要真机、不需要相机），
    所以它能进 CI，也能在没人拿手机的时候当回归防线（`docs/pairing-design.md` §6.1 的验收）。

    为什么打的是 **Rust 后端**：`/api/auth*` 与 `/api/ai/credentials` 是 M0/M1 的新契约，
    **Kotlin 侧一条都没有**（配对的三步都做在 Rust 上，见 §5.4）。

    断言（每一条都是这条链路上真实会断的地方）：

      1. 闸门**默认关着**          —— 非回环不带令牌照样通（三条既有 e2e 与桌面 App 的代言人）
      2. 打开开关                  —— 顺手生成长期令牌（`hasToken=true`）
      3. 非回环**没带**令牌 → 403  —— 不是 401（语义不能混：一个该去配对，一个该换钥匙）
      4. 非回环**带错**令牌 → 401
      5. 手机举手                  —— `pair/request` 的响应里**没有 ticket**（给了它，"必须看到屏幕"就失效）
      6. 桌面看得见 6 位码         —— `pair/pending` 是本机端点（非回环 403）
      7. 码错 → 400「配对失败」    —— 且**不回显** ticket / code
      8. 码对 → 拿到 43 字符令牌   —— 且立刻能用（拿它打受保护端点 200）
      9. 同一会话不能兑第二次      —— 410（"一次性"的全部含义）
     10. 设备列表**不含令牌**      —— 连前缀都不回
     11. 密钥下行**永不对匿名开放** —— 即使闸门关着（`enabled=false` 也 403）
     12. 勾了「允许密钥」才给       —— 418 与手机拿到的值一致
     13. 吊销立刻 401              —— 且**不误伤**别的设备
     14. 轮换 = 全部重配           —— 旧令牌 401、新令牌 200

    前置条件：本机有 cargo（首次会编 Rust 后端，之后是增量）。
    **不碰真库**：库是 `.db\comfyhub.db` 的临时副本，产物目录也是临时目录，跑完删掉。

.EXAMPLE
    pwsh -File scripts\e2e-pair-test.ps1
    pwsh -File scripts\e2e-pair-test.ps1 -KeepData      # 留着临时库看现场
#>
[CmdletBinding()]
param(
    # 已编好的 Rust 后端；不给就现编（`cargo build --bin comfyhub-server-rs`）。
    [string]$ServerExe = '',
    # 监听端口；0 = 自动挑一个空闲的。
    [int]$Port = 0,
    [switch]$KeepData
)

$ErrorActionPreference = 'Continue'
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$WorkDir     = Join-Path $ProjectRoot '.run\e2e-pair'
$ServerRs    = Join-Path $ProjectRoot 'server-rs'

$script:Pass = 0
$script:Fail = 0

function Assert-That {
    param([string]$Name, [bool]$Ok, [string]$Detail = '')
    if ($Ok) {
        $script:Pass++
        Write-Host ("  [ok]   " + $Name) -ForegroundColor DarkGreen
    } else {
        $script:Fail++
        Write-Host ("  [FAIL] " + $Name + $(if ($Detail) { "  -> $Detail" } else { '' })) -ForegroundColor Red
    }
}

# 发一个请求，回 `@{ status; body }`。**不抛异常** —— 这里断言的就是状态码本身。
function Invoke-Api {
    param(
        [string]$Method,
        [string]$Uri,
        [object]$Body = $null,
        [string]$Token = ''
    )
    $headers = @{}
    if ($Token) { $headers['X-ComfyHub-Token'] = $Token }
    $args = @{
        Method      = $Method
        Uri         = $Uri
        Headers     = $headers
        SkipHttpErrorCheck = $true
        TimeoutSec  = 20
    }
    if ($null -ne $Body) {
        $args['Body']        = ($Body | ConvertTo-Json -Compress -Depth 8)
        $args['ContentType'] = 'application/json'
    }
    $response = Invoke-WebRequest @args
    $json = $null
    try { $json = $response.Content | ConvertFrom-Json } catch { $json = $null }
    return @{ status = [int]$response.StatusCode; body = $json; raw = $response.Content }
}

function Get-FreePort {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $listener.Start()
    $port = $listener.LocalEndpoint.Port
    $listener.Stop()
    return $port
}

# 本机在局域网上的地址 —— **非回环**断言必须有一个真实网卡地址才能验
# （`inbound-auth-design.md` §D8 的那条造法：绑 0.0.0.0，然后打本机网卡 IP）。
function Get-LanAddress {
    $candidates = Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue |
        Where-Object {
            $_.IPAddress -notlike '127.*' -and
            $_.IPAddress -notlike '169.254.*' -and
            $_.AddressState -eq 'Preferred'
        } | Sort-Object -Property SkipAsSource, InterfaceMetric
    if ($candidates) { return $candidates[0].IPAddress }
    return $null
}

$server = $null
try {
    # ---------------------------------------------------------------- 准备
    if (-not $ServerExe) {
        $ServerExe = Join-Path $ServerRs 'target\debug\comfyhub-server-rs.exe'
        Write-Host '编译 Rust 后端（增量）…' -ForegroundColor DarkGray
        Push-Location $ServerRs
        try {
            & cargo build --bin comfyhub-server-rs 2>&1 | Select-Object -Last 3 | ForEach-Object { Write-Host "    $_" -ForegroundColor DarkGray }
            if ($LASTEXITCODE -ne 0) { throw "cargo build 失败（exit=$LASTEXITCODE）" }
        } finally { Pop-Location }
    }
    if (-not (Test-Path $ServerExe)) { throw "找不到后端可执行文件：$ServerExe" }

    if (Test-Path $WorkDir) { Remove-Item $WorkDir -Recurse -Force }
    New-Item -ItemType Directory -Path $WorkDir -Force | Out-Null
    $storage = Join-Path $WorkDir 'storage'
    New-Item -ItemType Directory -Path (Join-Path $storage 'media') -Force | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $storage 'ai') -Force | Out-Null

    # 真库的**临时副本**（连同 WAL：只拷主文件会丢掉最后一批事务）
    $realDb = Join-Path $ProjectRoot '.db\comfyhub.db'
    $dbCopy = Join-Path $WorkDir 'comfyhub.db'
    if (Test-Path $realDb) {
        Copy-Item $realDb $dbCopy -Force
        foreach ($suffix in @('-wal', '-shm')) {
            $side = "$realDb$suffix"
            if (Test-Path $side) { Copy-Item $side "$dbCopy$suffix" -Force }
        }
        Write-Host "库副本：$dbCopy（真库只读未动）" -ForegroundColor DarkGray

        # ① 把闸门拨回**出厂态**：第 1 幕验的就是"默认关着、没有令牌"。
        #    操作员的真库里很可能**已经配过手机**（`auth.pairing` enabled=true ——
        #    那是正常使用状态，不是脏数据），照原样拷过来会让第 1 幕必红 ——
        #    2026-09-25 实测（本机真机验收之后就一直红在这里，而代码一行没错）。
        #    只动**副本**，真库一个字节都不碰。
        #    ⚠ 这里曾经写成双引号 + `\"` 转义（PowerShell 不认那种转义）→ python 直接
        #    SyntaxError，而输出被 `Out-Null` 吞掉，于是"复位过了"却照样红。
        $resetPy = 'import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); c.execute("DELETE FROM app_settings WHERE k = ''auth.pairing''"); c.commit(); c.close()'
        & python -c $resetPy $dbCopy
        if ($LASTEXITCODE -ne 0) { throw "复位副本的配对状态失败（python 退出码 $LASTEXITCODE）" }
        Write-Host "副本上的配对状态已复位（第 1 幕验的是出厂态）" -ForegroundColor DarkGray
    } else {
        throw "没有 $realDb：Rust 后端不建表，先让 Kotlin 后端起一次库"
    }

    if ($Port -le 0) { $Port = Get-FreePort }
    $baseLoopback = "http://127.0.0.1:$Port"
    $lan = Get-LanAddress
    $baseLan = if ($lan) { "http://${lan}:$Port" } else { $null }

    # 密钥下行那一幕用**环境变量**这一档（托管文件那份是 DPAPI 密文，由 auth.rs 的单测覆盖）
    $env:COMFYHUB_CRED_E2E_PAIR_FIXTURE = 'sk-e2e-pair-fixture'

    Write-Host "启动 Rust 后端（0.0.0.0:$Port）…" -ForegroundColor DarkGray
    $server = Start-Process -FilePath $ServerExe `
        -ArgumentList @('--db', $dbCopy, '--storage', $storage, '--host', '0.0.0.0', '--port', "$Port") `
        -PassThru -WindowStyle Hidden `
        -RedirectStandardOutput (Join-Path $WorkDir 'server.out.log') `
        -RedirectStandardError (Join-Path $WorkDir 'server.err.log')

    $ready = $false
    for ($i = 0; $i -lt 60; $i++) {
        Start-Sleep -Milliseconds 250
        try {
            $health = Invoke-Api -Method GET -Uri "$baseLoopback/api/health"
            if ($health.status -eq 200) { $ready = $true; break }
        } catch { }
    }
    if (-not $ready) { throw '后端没起来（看 .run\e2e-pair\server.err.log）' }
    Write-Host "后端就绪：$baseLoopback  局域网地址：$baseLan" -ForegroundColor DarkGray

    # ---------------------------------------------------------------- 断言
    Write-Host "`n[1] 闸门默认关着（不许把默认行为改掉）" -ForegroundColor Cyan
    $auth = Invoke-Api -Method GET -Uri "$baseLoopback/api/auth"
    Assert-That 'GET /api/auth（回环）200' ($auth.status -eq 200) "$($auth.status)"
    Assert-That '默认 enabled=false' ($auth.body.enabled -eq $false)
    Assert-That '默认 hasToken=false' ($auth.body.hasToken -eq $false)
    Assert-That '状态形状就是那五个键' (
        (($auth.body.PSObject.Properties.Name | Sort-Object) -join ',') -eq 'devices,enabled,hasToken,pending,schema'
    ) (($auth.body.PSObject.Properties.Name | Sort-Object) -join ',')

    if (-not $baseLan) {
        Write-Host '  [skip] 本机没有局域网地址：非回环那几条断言跳过了（联网后再跑一次）' -ForegroundColor Yellow
    } else {
        $open = Invoke-Api -Method GET -Uri "$baseLan/api/prompts"
        Assert-That '闸门关着：非回环不带令牌也通（既有 e2e / 桌面 App 靠这条）' ($open.status -eq 200) "$($open.status)"

        Write-Host "`n[2] 打开开关" -ForegroundColor Cyan
        $on = Invoke-Api -Method PUT -Uri "$baseLoopback/api/auth" -Body @{ enabled = $true }
        Assert-That 'PUT /api/auth 200' ($on.status -eq 200) "$($on.status)"
        Assert-That '打开时顺手发了长期令牌' ($on.body.hasToken -eq $true)
        $bad = Invoke-Api -Method PUT -Uri "$baseLoopback/api/auth" -Body @{ nope = 1 }
        Assert-That '参数不对 → 400（不静默当成 false）' ($bad.status -eq 400) "$($bad.status)"
        $notLocal = Invoke-Api -Method GET -Uri "$baseLan/api/auth"
        Assert-That '非回环打管理端点 → 403' ($notLocal.status -eq 403) "$($notLocal.status)"

        Write-Host "`n[3] 403 与 401 的语义不能混" -ForegroundColor Cyan
        $noToken = Invoke-Api -Method GET -Uri "$baseLan/api/prompts"
        Assert-That '非回环没带令牌 → 403' ($noToken.status -eq 403) "$($noToken.status)"
        Assert-That '那句话是给人看的（需要配对令牌）' ($noToken.body.error -like '需要配对令牌*') "$($noToken.body.error)"
        $wrongToken = Invoke-Api -Method GET -Uri "$baseLan/api/prompts" -Token 'not-a-real-token'
        Assert-That '带错令牌 → 401' ($wrongToken.status -eq 401) "$($wrongToken.status)"
        Assert-That '401 的话是"无效或已被吊销"' ($wrongToken.body.error -eq '令牌无效或已被吊销') "$($wrongToken.body.error)"

        Write-Host "`n[4] 手机举手（响应里不许有 ticket）" -ForegroundColor Cyan
        $deviceId = 'phone-e2e-' + [guid]::NewGuid().ToString('N').Substring(0, 8)
        $req = Invoke-Api -Method POST -Uri "$baseLan/api/auth/pair/request" `
            -Body @{ deviceId = $deviceId; deviceName = 'E2E 假手机' }
        Assert-That 'pair/request 200（匿名可用）' ($req.status -eq 200) "$($req.status)"
        Assert-That '响应里没有 ticket（否则手机能自己算码）' (
            -not ($req.body.PSObject.Properties.Name -contains 'ticket')
        ) "$($req.raw)"
        Assert-That '键集合就是那四个' (
            (($req.body.PSObject.Properties.Name | Sort-Object) -join ',') -eq 'codeExpiresAt,expiresAt,requestId,serverTime'
        ) (($req.body.PSObject.Properties.Name | Sort-Object) -join ',')
        $requestId = $req.body.requestId

        Write-Host "`n[5] 桌面屏幕上显示 6 位码" -ForegroundColor Cyan
        $lanPending = Invoke-Api -Method GET -Uri "$baseLan/api/auth/pair/pending"
        Assert-That '非回环读待配对列表 → 403（那是"谁有钥匙"的情报）' ($lanPending.status -eq 403) "$($lanPending.status)"
        $pending = Invoke-Api -Method GET -Uri "$baseLoopback/api/auth/pair/pending"
        $mine = $pending.body.pending | Where-Object { $_.requestId -eq $requestId }
        Assert-That '本机看得到这次请求' ($null -ne $mine) $pending.raw
        $code = $mine.code
        Assert-That '6 位十进制码' ($code -match '^\d{6}$') "$code"
        Assert-That '带着来源 IP 与设备名' ($mine.ip -and $mine.deviceName -eq 'E2E 假手机')

        Write-Host "`n[6] 兑码" -ForegroundColor Cyan
        $wrongCode = if ($code -eq '000000') { '111111' } else { '000000' }
        $badCode = Invoke-Api -Method POST -Uri "$baseLan/api/auth/pair/claim" `
            -Body @{ requestId = $requestId; code = $wrongCode; deviceId = $deviceId; deviceName = 'E2E 假手机' }
        Assert-That '码错 → 400' ($badCode.status -eq 400) "$($badCode.status)"
        Assert-That '失败一律只说"配对失败"（不区分是哪一个错）' ($badCode.body.error -eq '配对失败') "$($badCode.body.error)"
        Assert-That '失败响应里不回显 6 位码' (-not ($badCode.raw -like "*$code*")) $badCode.raw

        $claim = Invoke-Api -Method POST -Uri "$baseLan/api/auth/pair/claim" `
            -Body @{ requestId = $requestId; code = $code; deviceId = $deviceId; deviceName = 'E2E 假手机' }
        Assert-That '码对 → 200' ($claim.status -eq 200) "$($claim.raw)"
        $token = [string]$claim.body.token
        Assert-That '令牌是 43 字符 base64url（D11）' ($token.Length -eq 43) "len=$($token.Length)"
        Assert-That '回了 deviceId' ($claim.body.deviceId -eq $deviceId)

        $replay = Invoke-Api -Method POST -Uri "$baseLan/api/auth/pair/claim" `
            -Body @{ requestId = $requestId; code = $code; deviceId = $deviceId; deviceName = 'E2E 假手机' }
        Assert-That '同一会话兑第二次 → 410' ($replay.status -eq 410) "$($replay.status)"

        $withToken = Invoke-Api -Method GET -Uri "$baseLan/api/prompts" -Token $token
        Assert-That '拿这台设备的令牌 → 200' ($withToken.status -eq 200) "$($withToken.status)"

        Write-Host "`n[7] 设备列表不回令牌 + 密钥下行要显式授权" -ForegroundColor Cyan
        $devices = Invoke-Api -Method GET -Uri "$baseLoopback/api/auth/devices"
        $row = $devices.body.devices | Where-Object { $_.id -eq $deviceId }
        Assert-That '新设备在列表里' ($null -ne $row)
        Assert-That '设备行里不许出现长期令牌' (-not ($devices.raw -like "*$token*"))
        Assert-That '默认不允许下行密钥' ($row.allowCredentials -eq $false)

        $credUri = "$baseLan/api/ai/credentials?refs=E2E_PAIR_FIXTURE"
        $credAnon = Invoke-Api -Method GET -Uri $credUri
        Assert-That '匿名读密钥 → 403（与闸门开关无关）' ($credAnon.status -eq 403) "$($credAnon.status)"
        $credDevice = Invoke-Api -Method GET -Uri $credUri -Token $token
        Assert-That '没勾"允许密钥"的设备 → 403' ($credDevice.status -eq 403) "$($credDevice.status)"

        $allow = Invoke-Api -Method POST -Uri "$baseLoopback/api/auth/devices/$deviceId/credentials" -Body @{ allow = $true }
        Assert-That '勾上 → 200' ($allow.status -eq 200) "$($allow.status)"
        Assert-That '回的就是这台设备的新状态' ($allow.body.allowCredentials -eq $true) $allow.raw
        $credDevice = Invoke-Api -Method GET -Uri $credUri -Token $token
        Assert-That '勾上之后拿得到值' ($credDevice.status -eq 200) "$($credDevice.status)"
        Assert-That '值就是桌面上的那一把' (
            $credDevice.body.credentials.E2E_PAIR_FIXTURE -eq 'sk-e2e-pair-fixture'
        ) $credDevice.raw

        Write-Host "`n[8] 吊销与轮换" -ForegroundColor Cyan
        # 先再配一台"别的设备"，验"吊销一台不误伤另一台"
        $otherReq = Invoke-Api -Method POST -Uri "$baseLan/api/auth/pair/request" -Body @{ deviceId = "$deviceId-b"; deviceName = 'E2E 假手机 B' }
        $otherPending = Invoke-Api -Method GET -Uri "$baseLoopback/api/auth/pair/pending"
        $otherCode = ($otherPending.body.pending | Where-Object { $_.requestId -eq $otherReq.body.requestId }).code
        $otherClaim = Invoke-Api -Method POST -Uri "$baseLan/api/auth/pair/claim" `
            -Body @{ requestId = $otherReq.body.requestId; code = $otherCode; deviceId = "$deviceId-b"; deviceName = 'E2E 假手机 B' }
        $otherToken = [string]$otherClaim.body.token
        Assert-That '第二台设备也配上了' ($otherClaim.status -eq 200) "$($otherClaim.status)"

        $revoke = Invoke-Api -Method POST -Uri "$baseLoopback/api/auth/devices/$deviceId/revoke" -Body @{}
        Assert-That '吊销 → 200' ($revoke.status -eq 200) "$($revoke.status)"
        $afterRevoke = Invoke-Api -Method GET -Uri "$baseLan/api/prompts" -Token $token
        Assert-That '被吊销的设备**立刻** 401' ($afterRevoke.status -eq 401) "$($afterRevoke.status)"
        $otherStill = Invoke-Api -Method GET -Uri "$baseLan/api/prompts" -Token $otherToken
        Assert-That '别的设备不受影响' ($otherStill.status -eq 200) "$($otherStill.status)"

        $rotate = Invoke-Api -Method POST -Uri "$baseLoopback/api/auth/rotate" -Body @{}
        Assert-That '轮换 → 200' ($rotate.status -eq 200) "$($rotate.status)"
        Assert-That '轮换说明了吊销几台' ([int]$rotate.body.revokedDevices -ge 1) $rotate.raw
        $afterRotate = Invoke-Api -Method GET -Uri "$baseLan/api/prompts" -Token $otherToken
        Assert-That '轮换后所有设备立刻 401（无宽限期）' ($afterRotate.status -eq 401) "$($afterRotate.status)"
        $newGlobal = (Invoke-Api -Method GET -Uri "$baseLoopback/api/auth").body
        Assert-That '轮换后仍有令牌' ($newGlobal.hasToken -eq $true)
        Assert-That '轮换后设备清空' (@($newGlobal.devices).Count -eq 0)

        Write-Host "`n[9] 兑码限速（按来源地址）" -ForegroundColor Cyan
        # 只靠"错 5 次作废"挡不住"再举一次手换 5 次机会" —— 所以要按来源地址压一道。
        # 前面那一串真实兑码也占同一个桶，所以这里一路打到出现 429 为止（上限给足）。
        $rateLimited = $false
        $echoed = ''
        for ($i = 1; $i -le 60; $i++) {
            $flood = Invoke-Api -Method POST -Uri "$baseLan/api/auth/pair/claim" `
                -Body @{ requestId = 'deadbeef'; code = '000000'; deviceId = 'phone-flood' }
            if ($flood.status -eq 429) {
                $rateLimited = $true
                $echoed = $flood.raw
                break
            }
        }
        Assert-That '反复兑码最终被限速（429）' $rateLimited "第 $i 次仍是 $($flood.status)"
        Assert-That '429 的响应里不回显码 / 会话 id' (
            $echoed -notlike '*000000*' -and $echoed -notlike '*deadbeef*'
        ) $echoed

        Write-Host "`n[10] 关回去（这台机器上的真实验收别被脚本开着闸门）" -ForegroundColor Cyan
        $off = Invoke-Api -Method PUT -Uri "$baseLoopback/api/auth" -Body @{ enabled = $false }
        Assert-That '关掉闸门 200' ($off.status -eq 200) "$($off.status)"
        $back = Invoke-Api -Method GET -Uri "$baseLan/api/prompts"
        Assert-That '关掉之后非回环又通了（默认行为）' ($back.status -eq 200) "$($back.status)"
    }
}
finally {
    if ($server -and -not $server.HasExited) {
        Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue
    }
    Remove-Item Env:\COMFYHUB_CRED_E2E_PAIR_FIXTURE -ErrorAction SilentlyContinue
    if (-not $KeepData -and (Test-Path $WorkDir)) {
        Start-Sleep -Milliseconds 300
        Remove-Item $WorkDir -Recurse -Force -ErrorAction SilentlyContinue
    } elseif ($KeepData) {
        Write-Host "临时现场保留在 $WorkDir" -ForegroundColor DarkGray
    }
}

Write-Host ''
if ($script:Fail -eq 0) {
    Write-Host "配对闸门 e2e：$($script:Pass) 项全部通过" -ForegroundColor Green
    exit 0
} else {
    Write-Host "配对闸门 e2e：$($script:Pass) 项通过，$($script:Fail) 项失败" -ForegroundColor Red
    exit 1
}
