<#
.SYNOPSIS
    可选能力：**本地 LoRA 训练引擎**（anima_lora 下游整合版 + 自有 WebUI）的探测 / 起停 / 打开。

.DESCRIPTION
    为什么要有这个脚本：训练引擎是**可选能力**（缺了只是"不能在本机训 LoRA"，不影响画图），
    所以按 AGENTS §8 的纪律来 —— **检测优先、安装只作兜底、装不上不拦启动**。
    与 `mcp-runtime.ps1` 的分工：那个管 MCP（AI 的工具），这个管训练引擎（另一套 Python 栈）。

    引擎 = 两个仓库（都在你自己的 GitHub 上）：
      · 训练内核  https://github.com/pixelllip/anima-lora-next   （上游 sorryhyun/anima_lora + 中文补丁 + Krea-2 内核）
      · Web 界面  https://github.com/pixelllip/anima-lora-webui  （零依赖，25MB 常驻；不装也行，脚本只做探测）

    落点（AGENTS §13：内容都在自己文件夹里）：
      <根>\training\anima_lora           引擎目录（`-Dir` 可指到别处；建议放工作区外，§12）
      <根>\.run\train-engine.json        start 记下的 pid / 端口 / WebUI 地址（stop 靠它）
      <引擎>\output\daemon\daemon.json   守护进程 pidfile（端口的唯一真源）

    动作：
      status   （默认）只看：引擎在不在、守护进程起没起、WebUI 起没起 —— 不联网、不起进程
      start    起守护进程 + WebUI（pythonw，无控制台窗口；§2）
      stop     关掉 start 起的那些（谁起的谁关）
      open     确保起着，然后打开浏览器
      install  引擎目录为空时，调上游那条 One-liner 装进去（ANIMA_DIR 指到 -Dir）

    用法：
      pwsh -File scripts\train-engine.ps1 status
      pwsh -File scripts\train-engine.ps1 start -Dir "$env:USERPROFILE\anima_lora"
      pwsh -File scripts\train-engine.ps1 open
      pwsh -File scripts\train-engine.ps1 stop
#>
param(
    [Parameter(Position = 0)]
    [ValidateSet('status', 'start', 'stop', 'open', 'install')]
    [string]$Action = 'status',
    # 引擎目录；默认 <根>\training\anima_lora（<根> 从脚本位置往上找）
    [string]$Dir = '',
    # WebUI 源码目录（没装成包时用 PYTHONPATH 挂上）；默认同级 ../anima-lora-webui 或 <根>\training\anima-lora-webui
    [string]$WebUiDir = '',
    [int]$WebUiPort = 8810
)

$ErrorActionPreference = 'Stop'

# ---- 找 <根>：与 lib/core/backend_paths.dart 同一判据（源码树 / 发布包两种布局）--------
function Get-ProjectRoot {
    $here = Split-Path -Parent $PSScriptRoot
    foreach ($cand in @($here, (Join-Path $here '..'))) {
        $c = (Resolve-Path $cand -ErrorAction SilentlyContinue).Path
        if (-not $c) { continue }
        if ((Test-Path (Join-Path $c 'server-rs\Cargo.toml')) -or
            (Test-Path (Join-Path $c 'db\schema.sqlite.sql')) -or
            (Test-Path (Join-Path $c 'backend\comfyhub-server-rs.exe'))) { return $c }
    }
    throw '找不到项目根目录（server-rs\Cargo.toml / db\schema.sqlite.sql / backend\*.exe 三选一）'
}

$Root = Get-ProjectRoot
if (-not $Dir) { $Dir = Join-Path $Root 'training\anima_lora' }
if (-not $WebUiDir) {
    # ⚠ 顺序要紧：**引擎自带的 `anima_lora_webui` 优先**。
    # 2026-10-18 起 WebUI 已并回下游仓库（`anima-lora-next` 的提交 aa3d5dea），
    # 兄弟目录 `anima-lora-webui` 只是历史副本；先命中它就会起一个**没有
    # /studio 接口的旧界面**（新页面能加载、每个请求都 404，且不报错）。
    $up1 = Split-Path -Parent $Root
    $up2 = Split-Path -Parent $up1
    foreach ($cand in @($Dir,
                        (Join-Path $up1 'anima-lora-webui'),
                        (Join-Path $up2 'anima-lora-webui'),
                        (Join-Path $up1 'anima-lora-next'),
                        (Join-Path $Root 'training\anima-lora-webui'))) {
        if (Test-Path (Join-Path $cand 'anima_lora_webui\server.py')) { $WebUiDir = $cand; break }
    }
}
$StatePath = Join-Path $Root '.run\train-engine.json'
$DaemonFile = Join-Path $Dir 'output\daemon\daemon.json'
$EnginePy = Join-Path $Dir '.venv\Scripts\python.exe'
$EnginePyW = Join-Path $Dir '.venv\Scripts\pythonw.exe'

function Say($m) { Write-Host "==> $m" -ForegroundColor Cyan }
function Info($m) { Write-Host "    $m" }

function Read-Json($path) {
    if (-not (Test-Path $path)) { return $null }
    try { return (Get-Content $path -Raw | ConvertFrom-Json) } catch { return $null }
}

function Get-Daemon {
    $d = Read-Json $DaemonFile
    if (-not $d) { return $null }
    try {
        $h = Invoke-RestMethod ("http://127.0.0.1:{0}/health" -f $d.port) -TimeoutSec 3
        return [pscustomobject]@{ port = $d.port; pid = $h.pid; root = $h.root; alive = $true }
    } catch {
        return [pscustomobject]@{ port = $d.port; pid = $d.pid; root = $d.root; alive = $false }
    }
}

function Get-WebUi {
    $s = Read-Json $StatePath
    if (-not $s -or -not $s.webuiPid) { return $null }
    $p = Get-Process -Id $s.webuiPid -ErrorAction SilentlyContinue
    if (-not $p) { return $null }
    return $s
}

function Show-Status {
    Say "引擎目录: $Dir"
    if (Test-Path (Join-Path $Dir 'train.py')) {
        $files = (Get-ChildItem $Dir -File -ErrorAction SilentlyContinue | Measure-Object).Count
        Info "已安装（train.py 在；顶层 $files 个文件）"
        Info ("解释器: {0}" -f $(if (Test-Path $EnginePy) { '有 .venv' } else { '缺 .venv（跑不了）' }))
    } else {
        Info "**没装**（目录里没有 train.py）→ 可选动作: pwsh -File scripts\train-engine.ps1 install"
    }
    $d = Get-Daemon
    if ($d) { Info ("守护进程: 端口 {0}，pid {1}，{2}" -f $d.port, $d.pid, $(if ($d.alive) { '健康' } else { '**没响应**（pidfile 是旧的？）' })) }
    else { Info "守护进程: 没起（提交作业时它也会自动拉起，见 anima_daemon/README.md）" }
    $w = Get-WebUi
    if ($w) {
        $auth = if ($w.webuiToken) { '（带访问令牌，用 open 动作打开）' } else { '' }
        Info ("WebUI: http://127.0.0.1:{0}（pid {1}）{2}" -f $w.webuiPort, $w.webuiPid, $auth)
    }
    else { Info "WebUI: 没起（可选；不起也能用引擎自带的 GUI/CLI）" }
    if ($WebUiDir) { Info "WebUI 源码: $WebUiDir" } else { Info "WebUI 源码: 没找到（只用引擎自带界面）" }
}

function Start-Engine {
    if (-not (Test-Path (Join-Path $Dir 'train.py'))) { throw "引擎没装：$Dir（先跑 install）" }
    if (-not (Test-Path $EnginePyW)) { throw "引擎缺 .venv：$EnginePyW" }

    $d = Get-Daemon
    if ($d -and $d.alive) { Say "守护进程已经在跑（端口 $($d.port)）" }
    else {
        Say '起守护进程（pythonw，无窗口）'
        # §2：Start-Process 给的不是管道，所以**不能**带 --watch-stdin；
        # 谁起的谁关：pid 记进 state，stop 负责收（守护进程自己也写 pidfile）
        # §13：引擎还会写一份用户级镜像（~/.anima/daemon.json）——用环境变量把它
        # 钉进项目内，和 Rust `train::start` 完全同一口径。
        $env:ANIMA_DAEMON_PIDFILE = Join-Path $Root '.run\anima-daemon.json'
        Start-Process -FilePath $EnginePyW -ArgumentList '-m', 'anima_daemon' -WorkingDirectory $Dir -WindowStyle Hidden
        for ($i = 0; $i -lt 30; $i++) {
            Start-Sleep -Milliseconds 500
            $d = Get-Daemon
            if ($d -and $d.alive) { break }
        }
        if (-not ($d -and $d.alive)) { throw '守护进程 15 秒内没起来（看 <引擎>\output\daemon\daemon.log）' }
        Say "守护进程就绪：端口 $($d.port)"
    }

    if (-not $WebUiDir) { Say '没找到 WebUI 源码，跳过后半段'; return }
    $w = Get-WebUi
    if ($w) { Say "WebUI 已经在跑：http://127.0.0.1:$($w.webuiPort)" }
    else {
        Say "起 WebUI（端口 $WebUiPort，pythonw，无窗口）"
        # PYTHONPATH 挂上源码目录 —— 没 pip install 也能跑
        $env:PYTHONPATH = $WebUiDir
        Start-Process -FilePath $EnginePyW `
            -ArgumentList '-m', 'anima_lora_webui.server', '--root', $Dir, '--port', "$WebUiPort" `
            -WorkingDirectory $WebUiDir -WindowStyle Hidden
        $ok = $false
        for ($i = 0; $i -lt 20; $i++) {
            Start-Sleep -Milliseconds 400
            try { $null = Invoke-WebRequest -UseBasicParsing ("http://127.0.0.1:{0}/api/_env" -f $WebUiPort) -TimeoutSec 2; $ok = $true; break } catch { }
        }
        if (-not $ok) { throw "WebUI 8 秒内没起来（端口 $WebUiPort 被占？）" }
        Say "WebUI 就绪：http://127.0.0.1:$WebUiPort"
    }

    $webPid = (Get-NetTCPConnection -LocalPort $WebUiPort -State Listen -ErrorAction SilentlyContinue |
            Select-Object -First 1).OwningProcess
    @{ webuiPid = $webPid; webuiPort = $WebUiPort; engineDir = $Dir; startedAt = (Get-Date).ToString('s') } |
        ConvertTo-Json | Set-Content -Encoding utf8 $StatePath
    Say "状态写入 $StatePath"
}

function Stop-Engine {
    $s = Read-Json $StatePath
    if ($s -and $s.webuiPid) {
        $p = Get-Process -Id $s.webuiPid -ErrorAction SilentlyContinue
        if ($p) { Say "停 WebUI（pid $($s.webuiPid)）"; Stop-Process -Id $s.webuiPid -Force -ErrorAction SilentlyContinue }
    }
    $d = Get-Daemon
    if ($d -and $d.alive) {
        Say "停守护进程（端口 $($d.port)，走它自己的 /shutdown 契约）"
        try { Invoke-RestMethod -Method Post ("http://127.0.0.1:{0}/shutdown" -f $d.port) -Body '{"kill_jobs":false}' -ContentType 'application/json' -TimeoutSec 10 | Out-Null } catch { }
    }
    Remove-Item $StatePath -Force -ErrorAction SilentlyContinue
    Remove-Item (Join-Path $Root '.run\anima-daemon.json') -Force -ErrorAction SilentlyContinue
    Say '已停（作业若在跑，守护进程会先让它跑完；要强停用 /jobs/{id}/stop）'
}

function Open-WebUi {
    Start-Engine
    $s = Read-Json $StatePath
    $url = if ($s) { "http://127.0.0.1:$($s.webuiPort)" } else { "http://127.0.0.1:$WebUiPort" }
    # Rust 起的 WebUI 会带 --token（绑 0.0.0.0）；首跳必须带 token 才能种 Cookie。
    # 手动 / 脚本起的实例没有这个字段，URL 保持不带参数。
    if ($s -and $s.webuiToken) { $url += "?token=$($s.webuiToken)" }
    Say "打开 $url"
    Start-Process $url
}

function Install-Engine {
    if (Test-Path (Join-Path $Dir 'train.py')) { Say "已经装了：$Dir（要升级用引擎里的 python tasks.py update）"; return }
    Say "装引擎 -> $Dir（上游 install.ps1；release zipball，不需要 git）"
    Say '提示：这条命令会下 5GB 上下的 torch 依赖，且会建桌面快捷方式/尝试加 Defender 排除项'
    $env:ANIMA_DIR = $Dir
    if (-not $env:HTTPS_PROXY) { $env:HTTPS_PROXY = 'http://127.0.0.1:7890' }
    irm https://raw.githubusercontent.com/sorryhyun/anima_lora/main/install.ps1 | iex
}

switch ($Action) {
    'status' { Show-Status }
    'start' { Start-Engine; Show-Status }
    'stop' { Stop-Engine; Show-Status }
    'open' { Open-WebUi }
    'install' { Install-Engine; Show-Status }
}
