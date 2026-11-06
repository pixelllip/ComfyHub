<#
.SYNOPSIS
    ComfyHub「托管凭据双端互操作」端到端自测（`/api/ai/providers/{id}/credentials` + web-search）。

.DESCRIPTION
    凭据的**写侧**落在 `<storage>\ai\credentials.dpapi.json` —— 那是**一份**文件，
    灰度期 Kotlin（8080）与 Rust（8081）两个实现都可能写它。于是有一条**单测永远证明不了**的判据：

        **一端写进去的密文，另一端必须解得开。**

    单测里两边都是我们自己（写→读都过同一段代码），所以它只能证明"我们自己自洽"，
    证明不了"Kotlin 的 `ProtectedData.Protect` 与我们的 `CryptProtectData` 是同一档作用域"。
    DPAPI 的作用域就藏在三个参数里（空熵 / CurrentUser / 禁 UI），错一个的表现是
    **换一个实现就读不出来** —— 而用户看到的是"设置页里明明配好了，AI 却说没有密钥"。

    这个脚本两边都真起（同一份真库副本、同一份凭据文件副本），按四个方向验：

      1. 初始状态两端一致           —— `{configured:false, source:"none", writable:true}`
      2. **Rust 写 → Kotlin 读**    —— 状态翻转 + Kotlin `POST /providers/{id}/test` 解得开
      3. **Kotlin 写 → Rust 读**    —— `GET /api/ai/credentials?refs=` 拿回**逐字相等**的明文
      4. 删除两端同步                —— 不留空壳
    另外四条是形状与纪律（响应永不含明文 / 真库原有那把 key 一个都没被动 / 错误档 / 输入校验）。

    第 2 条用 `POST /providers/{id}/test` 当判别器：夹具 provider 的 base_url 指向
    **必然被拒绝的端口**（`https://127.0.0.1:9`）+ 信任档 `loopback`。于是
      - 凭据解得开 → 过了凭据那关去连网 → `PROVIDER_UNREACHABLE`
      - 凭据解不开 → `MISSING_CREDENTIAL`
    两者互不混淆（这是 2026-09-25 现场调出来的：一开始拿 `/test` 的红/绿当判据，
    结果是"端点信任级别不允许回环"这种**别的**原因，压根没走到解密那一步）。

    前置条件：cargo（会增量编译 Rust 后端）；**Kotlin 那一半可选** ——
    它需要一份已 `installDist` 的 Kotlin 后端（`server\build\install\comfy-hub-server\bin\`）
    与一份 21~23 的 JDK。起不来时脚本会**大声报"跳过 N 项"**，不会假装通过。
    **不碰真库**：库与凭据文件都是临时副本，跑完删掉。

.EXAMPLE
    pwsh -File scripts\e2e-credential-test.ps1
    pwsh -File scripts\e2e-credential-test.ps1 -KeepData       # 留着现场
    pwsh -File scripts\e2e-credential-test.ps1 -JavaHome 'D:\Program Files\Android Studio\jbr'
#>
[CmdletBinding()]
param(
    # 已编好的 Rust 后端；不给就现编（`cargo build --bin comfyhub-server-rs`）。
    [string]$ServerExe = '',
    # 已 installDist 的 Kotlin 启动脚本；不给就按默认位置找，找不到 = 跳过跨端那一半。
    [string]$KotlinBat = '',
    # 跑 Kotlin 用的 JDK（Gradle 8.12 不支持 24/25，必须是 21~23）。
    [string]$JavaHome = '',
    [switch]$KeepData
)

$ErrorActionPreference = 'Continue'
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$WorkDir     = Join-Path $ProjectRoot '.run\e2e-credential'
$ServerRs    = Join-Path $ProjectRoot 'server-rs'

$script:Pass = 0
$script:Fail = 0
$script:Skip = 0

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

function Skip-That {
    param([string]$Name, [string]$Why)
    $script:Skip++
    Write-Host ("  [skip] " + $Name + "  -> $Why") -ForegroundColor Yellow
}

# 发一个请求，回 `@{ status; body; raw }`。**不抛异常** —— 断言的就是状态码本身。
function Invoke-Api {
    param([string]$Method, [string]$Uri, [object]$Body = $null)
    $args = @{ Method = $Method; Uri = $Uri; SkipHttpErrorCheck = $true; TimeoutSec = 30 }
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

# JDK 21~23（Gradle 8.12 不支持 24/25）：显式参数 → COMFYHUB_JDK_HOME → JAVA_HOME（版本要对）
# → 本机常见位置。判版本读 `release` 文件，**不启子进程**（起一次 JVM 要好几秒）。
function Resolve-Jdk21 {
    param([string]$Explicit)
    $candidates = @()
    if ($Explicit) { $candidates += $Explicit }
    if ($env:COMFYHUB_JDK_HOME) { $candidates += $env:COMFYHUB_JDK_HOME }
    if ($env:JAVA_HOME) { $candidates += $env:JAVA_HOME }
    $candidates += (Join-Path $ProjectRoot 'jre')
    $candidates += 'D:\Program Files\Android Studio\jbr'
    # ⚠ 循环变量**不能叫 `$home`**：PowerShell 的自动变量大小写不敏感，
    # `$HOME` 是只读的 → 当场抛 "Cannot overwrite variable HOME"（AGENTS §8 记过这条）。
    foreach ($jdkHome in $candidates) {
        if (-not $jdkHome) { continue }
        $java = Join-Path $jdkHome 'bin\java.exe'
        if (-not (Test-Path $java)) { continue }
        $release = Join-Path $jdkHome 'release'
        if (Test-Path $release) {
            $line = Select-String -Path $release -Pattern '^JAVA_VERSION="?([0-9]+)' -ErrorAction SilentlyContinue
            if ($line) {
                $major = [int]$line.Matches[0].Groups[1].Value
                if ($major -ge 24) { continue }   # 24/25 会让 gradle 8.12 直接失败
            }
        }
        return $jdkHome
    }
    return $null
}

$rustProc = $null
$ktProc   = $null
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
    # 数据目录可能被用户搬到别的盘（`<根>\data-dir.json`，与 backend_paths.dart 同一约定）
    $DataDir = $ProjectRoot
    $DataDirFile = Join-Path $ProjectRoot 'data-dir.json'
    if (Test-Path $DataDirFile) {
        try { $customDataDir = (Get-Content $DataDirFile -Raw -Encoding UTF8 | ConvertFrom-Json).dataDir } catch { $customDataDir = '' }
        if ($customDataDir) { $DataDir = $customDataDir }
    }
    $realDb = Join-Path $DataDir '.db\comfyhub.db'
    $dbCopy = Join-Path $WorkDir 'comfyhub.db'
    if (-not (Test-Path $realDb)) { throw "没有 $realDb：Rust 后端不建表，先让 Kotlin 后端起一次库" }
    Copy-Item $realDb $dbCopy -Force
    foreach ($suffix in @('-wal', '-shm')) {
        $side = "$realDb$suffix"
        if (Test-Path $side) { Copy-Item $side "$dbCopy$suffix" -Force }
    }
    Write-Host "库副本：$dbCopy（真库只读未动）" -ForegroundColor DarkGray

    # 真实的凭据文件副本 —— 它里面有 **Kotlin 写过的**那几把 key：
    # "Rust 解得开 Kotlin 写的"这一半靠它（没有它就只能验我们自己自洽）。
    $realCred = Join-Path $DataDir 'storage\ai\credentials.dpapi.json'
    $credCopy = Join-Path $storage 'ai\credentials.dpapi.json'
    $realRefs = @()
    if (Test-Path $realCred) {
        Copy-Item $realCred $credCopy -Force
        $realRefs = @((Get-Content $credCopy -Raw | ConvertFrom-Json).PSObject.Properties.Name)
        Write-Host "凭据副本：$credCopy（真实 ref：$($realRefs -join ', ')）" -ForegroundColor DarkGray
    } else {
        Write-Host "真机上还没有托管凭据文件：'Kotlin 写过的那几把'那几条会跳过" -ForegroundColor Yellow
    }

    # 夹具 provider：base_url 指向**必然被拒绝**的端口，信任档 `loopback` 让端点校验放行 ——
    # 于是 `/test` 的结果就只在"凭据解得开 / 解不开"之间分化（见文件头）。
    $fixtureRef = 'E2E_CRED_CHECK'
    $seedPy = @'
import sqlite3, sys
db = sys.argv[1]
c = sqlite3.connect(db)
c.execute("DELETE FROM ai_providers WHERE id = 'e2e-cred-check'")
c.execute(
    "INSERT INTO ai_providers (id, display_name, api, base_url, credential_ref, endpoint_trust,"
    " enabled, revision, created_at, updated_at)"
    " VALUES ('e2e-cred-check', '凭据 e2e', 'openai-completions', 'https://127.0.0.1:9',"
    " 'E2E_CRED_CHECK', 'loopback', 1, 1, '2026-09-25T00:00:00.000Z', '2026-09-25T00:00:00.000Z')"
)
c.commit()
c.close()
'@
    $seedFile = Join-Path $WorkDir 'seed.py'
    Set-Content -Path $seedFile -Value $seedPy -Encoding utf8
    & python $seedFile $dbCopy
    if ($LASTEXITCODE -ne 0) { throw "往副本里塞夹具 provider 失败（python 退出码 $LASTEXITCODE）" }

    $rsPort = Get-FreePort
    $rs = "http://127.0.0.1:$rsPort"

    Write-Host "启动 Rust 后端（$rs）…" -ForegroundColor DarkGray
    $rustProc = Start-Process -FilePath $ServerExe `
        -ArgumentList @('--db', $dbCopy, '--storage', $storage, '--host', '127.0.0.1', '--port', "$rsPort") `
        -PassThru -WindowStyle Hidden `
        -RedirectStandardOutput (Join-Path $WorkDir 'rust.out.log') `
        -RedirectStandardError (Join-Path $WorkDir 'rust.err.log')

    $ready = $false
    for ($i = 0; $i -lt 80; $i++) {
        Start-Sleep -Milliseconds 250
        try {
            $health = Invoke-Api -Method GET -Uri "$rs/api/health"
            if ($health.status -eq 200) { $ready = $true; break }
        } catch { }
    }
    if (-not $ready) { throw 'Rust 后端没起来（看 .run\e2e-credential\rust.err.log）' }

    # Kotlin 那一半（可选）：它才是"跨端"这个词的全部意义所在
    $kt = $null
    if (-not $KotlinBat) {
        $KotlinBat = Join-Path $ProjectRoot 'server\build\install\comfy-hub-server\bin\comfy-hub-server.bat'
    }
    $jdk = Resolve-Jdk21 -Explicit $JavaHome
    if (-not (Test-Path $KotlinBat)) {
        Write-Host "找不到 Kotlin 启动脚本（$KotlinBat）：先 cd server 跑 gradle installDist" -ForegroundColor Yellow
    } elseif (-not $jdk) {
        Write-Host '找不到 21~23 的 JDK（Gradle 8.12 不支持 24/25）：用 -JavaHome 指定' -ForegroundColor Yellow
    } else {
        $ktPort = Get-FreePort
        $kt = "http://127.0.0.1:$ktPort"
        Write-Host "启动 Kotlin 后端（$kt，JDK=$jdk）…" -ForegroundColor DarkGray
        $env:COMFYHUB_JDBC_URL = 'jdbc:sqlite:' + ($dbCopy -replace '\\', '/')
        $env:COMFYHUB_STORAGE = $storage
        $env:COMFYHUB_PROJECT_ROOT = $WorkDir
        $env:COMFYHUB_PORT = "$ktPort"
        $env:JAVA_HOME = $jdk
        $ktProc = Start-Process -FilePath $KotlinBat -PassThru -WindowStyle Hidden `
            -WorkingDirectory (Join-Path $ProjectRoot 'server') `
            -RedirectStandardOutput (Join-Path $WorkDir 'kotlin.out.log') `
            -RedirectStandardError (Join-Path $WorkDir 'kotlin.err.log')
        $ready = $false
        for ($i = 0; $i -lt 120; $i++) {
            Start-Sleep -Milliseconds 250
            try {
                $health = Invoke-Api -Method GET -Uri "$kt/api/health"
                if ($health.status -eq 200) { $ready = $true; break }
            } catch { }
        }
        if (-not $ready) {
            Write-Host 'Kotlin 后端没起来（看 kotlin.err.log）：跨端那几条会跳过' -ForegroundColor Yellow
            $kt = $null
        }
    }

    # ---------------------------------------------------------------- 断言
    Write-Host "`n[1] 初始状态（夹具 ref 没配过）" -ForegroundColor Cyan
    $rsList = (Invoke-Api -Method GET -Uri "$rs/api/ai/providers").body
    $rsCred = ($rsList | Where-Object { $_.id -eq 'e2e-cred-check' }).credential
    Assert-That 'Rust：configured=false / none / writable' (
        $rsCred.configured -eq $false -and $rsCred.source -eq 'none' -and $rsCred.writable -eq $true
    ) ($rsCred | ConvertTo-Json -Compress)
    if ($kt) {
        $ktList = (Invoke-Api -Method GET -Uri "$kt/api/ai/providers").body
        $ktCred = ($ktList | Where-Object { $_.id -eq 'e2e-cred-check' }).credential
        Assert-That 'Kotlin：同一个状态（两端对同一份文件的读法一致）' (
            $ktCred.configured -eq $false -and $ktCred.source -eq 'none' -and $ktCred.writable -eq $true
        ) ($ktCred | ConvertTo-Json -Compress)
    }

    Write-Host "`n[2] **Rust 写 → Kotlin 读**" -ForegroundColor Cyan
    $sentinel = 'sk-rust-wrote-this-0123456789'
    $put = Invoke-Api -Method PUT -Uri "$rs/api/ai/providers/e2e-cred-check/credentials" -Body @{ value = $sentinel }
    Assert-That 'PUT 200 且状态翻成 managed' (
        $put.status -eq 200 -and $put.body.configured -eq $true -and $put.body.source -eq 'managed'
    ) "$($put.status) $($put.raw)"
    Assert-That '响应里**搜不到明文**（AIH-012）' (-not $put.raw.Contains($sentinel))
    $fileText = Get-Content $credCopy -Raw
    Assert-That '文件里也搜不到明文（AIH-015）' (-not $fileText.Contains($sentinel))
    $sealed = ($fileText | ConvertFrom-Json).$fixtureRef
    Assert-That '密文是**标准** base64（`+/` 而非 url-safe）' (
        $sealed -and -not $sealed.Contains('-') -and -not $sealed.Contains('_') -and ($sealed.Length % 4 -eq 0)
    ) "长度=$($sealed.Length)"

    if ($kt) {
        $ktCred = ((Invoke-Api -Method GET -Uri "$kt/api/ai/providers").body |
            Where-Object { $_.id -eq 'e2e-cred-check' }).credential
        Assert-That 'Kotlin 看得到 managed（读的是同一份文件）' (
            $ktCred.configured -eq $true -and $ktCred.source -eq 'managed'
        ) ($ktCred | ConvertTo-Json -Compress)
        # ★ 这一条就是整个脚本存在的理由：Kotlin **解得开 Rust 写的密文**
        $test = Invoke-Api -Method POST -Uri "$kt/api/ai/providers/e2e-cred-check/test"
        Assert-That 'Kotlin 解得开 Rust 的密文（解不开会是 MISSING_CREDENTIAL）' (
            $test.body.errorCode -eq 'PROVIDER_UNREACHABLE'
        ) "errorCode=$($test.body.errorCode) message=$($test.body.message)"
    } else {
        Skip-That 'Kotlin 解得开 Rust 的密文' 'Kotlin 后端没起来（跨端互操作这一条没验）'
    }

    Write-Host "`n[3] **Kotlin 写 → Rust 读**" -ForegroundColor Cyan
    if ($kt) {
        $sentinel2 = 'sk-kotlin-wrote-this-9876543210'
        $put2 = Invoke-Api -Method PUT -Uri "$kt/api/ai/providers/e2e-cred-check/credentials" -Body @{ value = $sentinel2 }
        Assert-That 'Kotlin PUT 200 且状态仍是 managed' (
            $put2.status -eq 200 -and $put2.body.configured -eq $true
        ) "$($put2.status) $($put2.raw)"
        $down = Invoke-Api -Method GET -Uri "$rs/api/ai/credentials?refs=$fixtureRef"
        Assert-That 'Rust 解得开 Kotlin 的密文（且逐字相等）' (
            $down.status -eq 200 -and $down.body.credentials.$fixtureRef -eq $sentinel2
        ) "$($down.status) 取回长度=$(if ($down.body.credentials.$fixtureRef) { $down.body.credentials.$fixtureRef.Length } else { 0 })"
    } else {
        Skip-That 'Rust 解得开 Kotlin 的密文' 'Kotlin 后端没起来'
        Skip-That 'Rust 解得开 Kotlin 的密文（逐字相等）' 'Kotlin 后端没起来'
    }

    Write-Host "`n[4] 删除：两端同步（不留空壳）" -ForegroundColor Cyan
    $del = Invoke-Api -Method DELETE -Uri "$rs/api/ai/providers/e2e-cred-check/credentials"
    Assert-That 'Rust DELETE 200 且回到 none' (
        $del.status -eq 200 -and $del.body.configured -eq $false -and $del.body.source -eq 'none'
    ) "$($del.status) $($del.raw)"
    Assert-That '文件里那把 key 真的没了' (-not ((Get-Content $credCopy -Raw | ConvertFrom-Json).PSObject.Properties.Name -contains $fixtureRef))
    if ($kt) {
        $ktCred = ((Invoke-Api -Method GET -Uri "$kt/api/ai/providers").body |
            Where-Object { $_.id -eq 'e2e-cred-check' }).credential
        Assert-That 'Kotlin 也看到没了' ($ktCred.configured -eq $false) ($ktCred | ConvertTo-Json -Compress)
    }

    Write-Host "`n[5] 真机上原有的那几把 key 一个都没被动（写侧只碰自己那个 ref）" -ForegroundColor Cyan
    if ($realRefs.Count -eq 0) {
        Skip-That '原有 ref 仍可解开' '真机上没有托管凭据文件'
    } else {
        $joined = ($realRefs -join ',')
        $down = Invoke-Api -Method GET -Uri "$rs/api/ai/credentials?refs=$joined"
        foreach ($ref in $realRefs) {
            $value = $down.body.credentials.$ref
            Assert-That "  $ref（只报长度，不打印值）" ($null -ne $value -and $value.Length -gt 0) "长度=$(if ($value) { $value.Length } else { 0 })"
        }
    }

    Write-Host "`n[6] 错误档与输入校验（Rust 侧）" -ForegroundColor Cyan
    $missing = Invoke-Api -Method GET -Uri "$rs/api/ai/providers/nope/credentials"
    Assert-That '不存在的 provider：400 CONFIG_ERROR + detail' (
        $missing.status -eq 400 -and $missing.body.error -eq 'CONFIG_ERROR' -and $missing.body.detail -eq 'Provider 不存在: nope'
    ) "$($missing.status) $($missing.raw)"

    $bad = Invoke-Api -Method PUT -Uri "$rs/api/ai/providers/e2e-cred-check/credentials" -Body @{ value = 'E2E_CRED_CHECK=sk-x' }
    Assert-That 'NAME=value 整行：400（不许静默存下来）' (
        $bad.status -eq 400 -and $bad.body.detail -eq '请只粘贴密钥的值，不要粘贴 NAME=value 这种整行'
    ) "$($bad.status) $($bad.raw)"

    $blank = Invoke-Api -Method PUT -Uri "$rs/api/ai/providers/e2e-cred-check/credentials" -Body @{ value = '   ' }
    Assert-That '空值 = 不修改（不是清空）' (
        $blank.status -eq 200 -and $blank.body.configured -eq $false
    ) "$($blank.status) $($blank.raw)"

    # 凭空删除也得是 200 + none（幂等：界面重复点不会报错）
    $delAgain = Invoke-Api -Method DELETE -Uri "$rs/api/ai/providers/e2e-cred-check/credentials"
    Assert-That '重复 DELETE 仍 200 + none（幂等）' (
        $delAgain.status -eq 200 -and $delAgain.body.configured -eq $false
    ) "$($delAgain.status) $($delAgain.raw)"

    Write-Host "`n[7] 联网搜索那把 key（免密钥出厂可用，只是个可选入口）" -ForegroundColor Cyan
    $rsWeb = Invoke-Api -Method GET -Uri "$rs/api/ai/web-search/credential"
    Assert-That 'Rust：形状就是那三个键' (
        (($rsWeb.body.PSObject.Properties.Name | Sort-Object) -join ',') -eq 'configured,source,writable'
    ) $rsWeb.raw
    if ($kt) {
        $ktWeb = Invoke-Api -Method GET -Uri "$kt/api/ai/web-search/credential"
        Assert-That 'Kotlin：同一个状态（形状 + 取值）' (
            (($ktWeb.body.PSObject.Properties.Name | Sort-Object) -join ',') -eq 'configured,source,writable' -and
            $ktWeb.raw -eq $rsWeb.raw
        ) "rust=$($rsWeb.raw) kotlin=$($ktWeb.raw)"
    }
} catch {
    # ⚠ `exit` 只能写在 finally **之外**：写在 finally 里会把异常一起吞掉 ——
    # 表现是"脚本什么都没说就结束了、还报通过 0 失败 0"（第一版就是这样）。
    Write-Host "`n脚本自身出错：$($_.Exception.Message)" -ForegroundColor Red
    if ($_.ScriptStackTrace) { Write-Host $_.ScriptStackTrace -ForegroundColor DarkGray }
    $script:Fail++
} finally {
    foreach ($proc in @($rustProc, $ktProc)) {
        if ($proc -and -not $proc.HasExited) { try { $proc.Kill() } catch { } }
    }
    Remove-Item Env:COMFYHUB_JDBC_URL -ErrorAction SilentlyContinue
    Remove-Item Env:COMFYHUB_STORAGE -ErrorAction SilentlyContinue
    Remove-Item Env:COMFYHUB_PROJECT_ROOT -ErrorAction SilentlyContinue
    Remove-Item Env:COMFYHUB_PORT -ErrorAction SilentlyContinue
    if (-not $KeepData -and (Test-Path $WorkDir)) { Remove-Item $WorkDir -Recurse -Force -ErrorAction SilentlyContinue }
    Write-Host "`n通过 $script:Pass、失败 $script:Fail、跳过 $script:Skip" -ForegroundColor Cyan
    if ($script:Skip -gt 0) {
        Write-Host "⚠ 有 $script:Skip 项**没验**（跨端互操作那几条靠 Kotlin 后端）：别把这次当成通过。" -ForegroundColor Yellow
    }
}

if ($script:Pass -le 0) {
    Write-Host '  ✗ 一项检查都没跑（后端没起？前置条件没过？）—— 这不叫通过。' -ForegroundColor Red
    exit 1
}
if ($script:Fail -gt 0) { exit 1 }
if ($script:Skip -gt 0) { exit 2 }
exit 0

