<#
    API 端口的**唯一解析入口**（2026-09-22，用户要求"不再显式指定端口、启动时自动挑一个"）。

    为什么需要这个文件：端口过去写死在 5 个地方，而且**互不知道** ——
    `comfyhub.ps1`（探测）、`server.ps1`（启动）、`Config.kt`（默认值）、
    `settings_store.dart`（App 连谁）、`capture_core.py`（ComfyUI 节点推给谁）。
    `comfyhub.ps1` 甚至从来没把 `-Port` 透传给 `server.ps1`，所以只要有人动过一处，
    就会出现"后端明明在跑、脚本却说没起来"这种自相矛盾的现场。

    现在只有一个真源：**后端绑好端口后写的 `<项目根>\.run\api.json`**（见 `ApiPortFile.kt`）。
    本文件负责把它读出来，并把优先级钉死：

        显式参数 -Port  >  环境变量 COMFYHUB_PORT  >  .run\api.json 里上次的端口  >  0（自动）

    · `0` = 让操作系统挑一个空闲端口（后端 `port = 0` 的语义）；
    · 上一次的端口**只要真的有人在听就复用**（"粘住"），这样 App 与单独重启后端不会打架；
    · 被占 / 落在 Windows 保留端口段里 → 后端自己会回落成随机端口并改写状态文件，
      `fellBack=true` 时脚本要**如实说一句**"你要的端口没绑上，换成了 X"。

    读取端一律**不许抛异常**：文件不存在、半截 JSON、类型不对都返回 $null / 0，
    让调用方回落到下一档。端口这种东西读不到最多是多试一次，绝不该让服务起不来。
#>

$Script:ApiPortFileName = 'api.json'
$Script:ApiPortRunDirName = '.run'

<#
    读状态文件。返回 $null 表示"没有 / 读不动"。

    @param ProjectRoot 项目根（`<根>\.run\api.json`）
#>
function Read-ApiPortState {
    param([Parameter(Mandatory)][string]$ProjectRoot)

    $path = Join-Path (Join-Path $ProjectRoot $Script:ApiPortRunDirName) $Script:ApiPortFileName
    if (-not (Test-Path $path)) { return $null }

    $raw = $null
    try { $raw = Get-Content -Path $path -Raw -Encoding UTF8 -ErrorAction Stop } catch { return $null }
    if ([string]::IsNullOrWhiteSpace($raw)) { return $null }

    # 首选 ConvertFrom-Json（pwsh 7 / 5.1 都有）。失败再退到正则 —— 不引任何依赖。
    try {
        $obj = $raw | ConvertFrom-Json -ErrorAction Stop
        $port = 0
        if (-not [int]::TryParse([string]$obj.port, [ref]$port)) { return $null }
        if ($port -le 0 -or $port -gt 65535) { return $null }
        return [pscustomobject]@{
            Port          = $port
            Pid           = [int64]("0" + [string]$obj.pid)
            Host          = [string]$obj.host
            Url           = [string]$obj.url
            StartedAt     = [string]$obj.startedAt
            RequestedPort = [int]("0" + [string]$obj.requestedPort)
            FellBack      = [bool]$obj.fellBack
            Path          = $path
        }
    } catch {
        $m = [regex]::Match($raw, '"port"\s*:\s*(\d+)')
        if (-not $m.Success) { return $null }
        $port = [int]$m.Groups[1].Value
        if ($port -le 0) { return $null }
        $req = [regex]::Match($raw, '"requestedPort"\s*:\s*(\d+)')
        return [pscustomobject]@{
            Port          = $port
            Pid           = 0
            Host          = ''
            Url           = ''
            StartedAt     = ''
            RequestedPort = if ($req.Success) { [int]$req.Groups[1].Value } else { 0 }
            FellBack      = $false
            Path          = $path
        }
    }
}

<# 端口上有没有人在听（不依赖 server.ps1 里的实现，两边各自成立）。 #>
function Test-ApiPortListening {
    param(
        [int]$Port,
        [int]$TimeoutMs = 200
    )
    if ($Port -le 0) { return $false }
    $client = [System.Net.Sockets.TcpClient]::new()
    try {
        $async = $client.BeginConnect('127.0.0.1', $Port, $null, $null)
        if (-not $async.AsyncWaitHandle.WaitOne($TimeoutMs, $false)) { return $false }
        $client.EndConnect($async)
        return $true
    } catch {
        return $false
    } finally {
        $client.Close()
    }
}

<#
    端口现在**能不能被绑上**（比"有没有人在听"更严）。

    为什么要这一道：`Test-ApiPortListening` 只看"有没有监听者"，而**刚被杀掉的进程**
    那一瞬间已经不再应答、socket 却还没完全放掉 —— 这时探测说"空闲"，紧接着后端去 bind
    却失败（实测：restart 路径上必然撞上，单独启动却必然成功，差别就在这个窗口）。
    所以"要复用一个端口"之前，先用同样的方式**真的绑一下**再放掉。

    返回 $true = 现在可以安全地 bind 这个端口。
#>
function Test-ApiPortBindable {
    param([int]$Port, [string]$BindHost = '127.0.0.1')
    if ($Port -le 0) { return $true }
    $listener = $null
    try {
        $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Parse($BindHost), $Port)
        $listener.Start()
        return $true
    } catch {
        return $false
    } finally {
        if ($listener) { try { $listener.Stop() } catch { } }
    }
}

<#
    解析"这次该用哪个端口"。

    返回对象：`Port`（0 = 自动）、`Source`（flag / env / state / auto）、`State`（上次的状态，可能为 $null）。

    注意 **0 是合法值**：它表示"交给操作系统挑"。所以调用方判断"有没有指定"要看 `Source`，
    不能看 `Port -gt 0`。
#>
function Resolve-ApiPort {
    param(
        [string]$ProjectRoot,
        [int]$Explicit = 0,
        [string]$EnvValue = $env:COMFYHUB_PORT
    )

    if ($Explicit -gt 0) {
        return [pscustomobject]@{ Port = $Explicit; Source = 'flag'; State = $null }
    }

    $envPort = 0
    if (-not [string]::IsNullOrWhiteSpace($EnvValue) -and [int]::TryParse($EnvValue.Trim(), [ref]$envPort)) {
        if ($envPort -ge 0 -and $envPort -le 65535) {
            return [pscustomobject]@{ Port = $envPort; Source = 'env'; State = $null }
        }
    }

    $state = Read-ApiPortState -ProjectRoot $ProjectRoot
    if ($state -and $state.Port -gt 0) {
        # 判据只有一条：**真的有人在听**。
        #
        # ⚠️ 不要在这里加"端口可绑"（`Test-ApiPortBindable`）—— 那是个陷阱：
        # 正常 `up` 的时候后端**正占着**这个端口，可绑性必然是 false，
        # 于是每个 up 都会当"没有可复用的端口"重新随机（2026-09-22 实测踩到，
        # 表现是"端口每次都在变"）。"可绑"只对**重启**有意义，用在 [Wait-ApiPortReleased] 里。
        if (Test-ApiPortListening -Port $state.Port) {
            return [pscustomobject]@{ Port = $state.Port; Source = 'state'; State = $state }
        }
    }

    return [pscustomobject]@{ Port = 0; Source = 'auto'; State = $state }
}

<#
    等后端把真实端口写进状态文件（`-Port 0` 时脚本事先不知道会绑到哪个）。

    返回状态对象或 $null。**只读文件，不做 HTTP** —— 健康与否由调用方判断。
#>
function Wait-ApiPortState {
    param(
        [Parameter(Mandatory)][string]$ProjectRoot,
        [int]$Seconds = 30,
        [int]$MinPort = 1
    )
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $deadline) {
        $state = Read-ApiPortState -ProjectRoot $ProjectRoot
        if ($state -and $state.Port -ge $MinPort) { return $state }
        Start-Sleep -Milliseconds 200
    }
    return $null
}

<#
    端口落不落在 **Windows 保留端口段**里（Hyper-V / WSL 预留）。

    为什么值得单独一个函数：那种端口**谁都绑不上，而 `netstat` / `Get-NetTCPConnection`
    又看不见占用** —— 后端只会报一句 `Address already in use`，
    查起来像撞鬼（2026-09-22 本机 8080 正落在 `7991-8090` 里，就是靠这个查出来的）。

    返回命中的范围字符串（如 `7991-8090`）；不在里面 / 查不出来返回 $null。**绝不抛异常。**
#>
function Get-ExcludedTcpPortRange {
    param([int]$Port)
    if ($Port -le 0) { return $null }
    try {
        $out = & netsh int ipv4 show excludedportrange protocol=tcp 2>$null
    } catch {
        return $null
    }
    foreach ($line in @($out)) {
        $m = [regex]::Match([string]$line, '^\s*(\d+)\s+(\d+)')
        if (-not $m.Success) { continue }
        $start = [int]$m.Groups[1].Value
        $end = [int]$m.Groups[2].Value
        if ($Port -ge $start -and $Port -le $end) { return "$start-$end" }
    }
    return $null
}

<#
    写状态文件（**只有 `restart` 用它**）。

    为什么需要它：`restart` = down + up，而 `down`（`server.ps1 stop`）会把这个文件删掉
    （后端已停，记录没意义）。但 down 与 up 之间有个真实竞态：
    被强杀的旧后端**可能刚好在那一刻跑完自己的收尾钩子**，让文件在"删掉之后"又冒出来 ——
    于是紧接着的 up 读到的是**旧记录**，bind 到刚刚释放、还没完全关闭的端口上，
    表现就是"重启一次换一个端口、还偶尔健康检查失败"（2026-09-22 实测）。

    所以 restart 在 down **之后**把刚才记得的端口**原样写回**：不管中间被谁动过，
    up 看到的都是"这次要用的那个端口"。写入是 `.tmp` + move，读端不会看到半截 JSON。
#>
function Write-ApiPortState {
    param(
        [Parameter(Mandatory)][string]$ProjectRoot,
        [Parameter(Mandatory)][int]$Port,
        [int]$RequestedPort = 0
    )
    if ($Port -le 0) { return $false }
    $dir = Join-Path $ProjectRoot $Script:ApiPortRunDirName
    $path = Join-Path $dir $Script:ApiPortFileName
    $tmp = "$path.tmp"
    $payload = [ordered]@{
        port          = $Port
        pid           = 0
        host          = '127.0.0.1'
        url           = "http://127.0.0.1:$Port"
        startedAt     = (Get-Date).ToUniversalTime().ToString('o')
        requestedPort = $RequestedPort
        fellBack      = $false
    }
    try {
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
        # ConvertTo-Json 在 5.1 会把单元素结构压成标量，这里显式 -Depth 4 并**强制**成对象数组
        ($payload | ConvertTo-Json -Compress -Depth 4) | Set-Content -Path $tmp -Encoding UTF8
        Move-Item -Path $tmp -Destination $path -Force
        return $true
    } catch {
        Remove-Item $tmp -Force -ErrorAction SilentlyContinue
        return $false
    }
}

<#
    从"候选端口"里挑出**真的在响应 /health 的那个**。

    为什么需要它：状态文件是"后写的赢"，而 restart 期间会**先后有两个后端**在写它
    （旧进程的收尾钩子 + 新进程的启动），中间还有强杀留下的陈旧记录。
    于是"文件里写的"和"真的在听的"会短暂地对不上 —— 只信其中一个都会误判：
    只信文件 → 撞上旧记录（探一个死端口）；只信自己的候选 → 漏掉后端实际回落的随机端口。

    所以这里对**两个候选都真的打一次 /api/health**，谁回 200 就用谁。
    状态文件里的端口排第一（它通常就是刚起来的那个）。
#>
function Find-LiveApiPort {
    param(
        [Parameter(Mandatory)][string]$ProjectRoot,
        [int]$PreferredPort = 0,
        [int]$TimeoutSec = 3
    )

    $candidates = @()
    $state = Read-ApiPortState -ProjectRoot $ProjectRoot
    if ($state -and $state.Port -gt 0) { $candidates += $state.Port }
    if ($PreferredPort -gt 0) { $candidates += $PreferredPort }
    $candidates = $candidates | Select-Object -Unique

    foreach ($port in $candidates) {
        if (-not (Test-ApiPortListening -Port $port)) { continue }
        try {
            $r = Invoke-RestMethod "http://127.0.0.1:$port/api/health" -TimeoutSec $TimeoutSec
            if ($r) { return [pscustomobject]@{ Port = $port; Health = $r } }
        } catch {
            # 有人在听但没回应（旧进程正在退出 / 还没铺好）：换下一个候选
        }
    }
    return $null
}

<#
    给"人"看的一句话：这次端口是怎么定的。

    如实说清三件事：用了哪个端口、从哪来的、有没有被换掉（用户要求"失败了回到随机端口"，
    那就必须让他看得见"我给的 8280 被占了"）。
#>
function Format-ApiPortNote {
    param($Resolved, $State)

    $port = if ($Resolved) { $Resolved.Port } else { 0 }
    $source = if ($Resolved) { $Resolved.Source } else { 'auto' }
    $state = if ($State) { $State } elseif ($Resolved) { $Resolved.State } else { $null }

    switch ($source) {
        'flag' { return "端口 $port（-Port 指定）" }
        'env' { return "端口 $port（环境变量 COMFYHUB_PORT 指定）" }
        'state' { return "端口 $port（沿用上次 .run\api.json 里记录的端口）" }
        default {
            if ($state -and $state.FellBack -and $state.RequestedPort -gt 0) {
                return "端口 $port（**候选 $($state.RequestedPort) 绑不上** —— 被占或落在 Windows 保留段里，已改用随机端口）"
            }
            return '端口自动（0 = 交给操作系统挑一个空闲端口；真实端口写在 .run\api.json）'
        }
    }
}
