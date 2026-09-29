<#
    后端是哪一个：**Rust**（P4 目标）还是 **Kotlin**（旧的那一份）。

    为什么单独一个文件：`server.ps1`（起停）与 `comfyhub.ps1`（整体生命周期 / doctor）都要知道
    这件事，各写一份必然漂移 —— 而漂移的表现是"doctor 说 Kotlin 在跑、server.ps1 去停 Rust"。
    AGENTS §5 那条踩过的坑（共享小工具必须住在被 dot-source 的公共文件里）就是这个意思。

    ## 选择规则（检测优先，别乱指）

        -Backend rust|kotlin  >  环境变量 COMFYHUB_BACKEND  >  auto

    `auto` 的事实依据只有一条：**Rust 二进制在不在**。
    找的顺序（与发布包/源码树两种布局对齐，见 AGENTS §7）：

        1. `<根>\server\comfyhub-server-rs.exe`            —— 发布包（就地装配）
        2. `<根>\server-rs\target\release\comfyhub-server-rs.exe` —— 源码树（正式构建）
        3. `<根>\server-rs\target\debug\comfyhub-server-rs.exe`   —— 源码树（debug，仅在没 release 时）

    ⚠ 找不到时**不报错、回退 Kotlin**：这条是为了"切过去之后如果 Rust 还没编出来，
    用户至少还能用"。回退会**显式说一句**（doctor 里也看得到），绝不静默。
#>

# 项目根：往上找 scripts\comfyhub.ps1（与其它脚本同一套算法，两种布局都认）
function Resolve-ProjectRootForBackend {
    param([string]$Start)
    $dir = if ($Start) { (Resolve-Path $Start).Path } else { $PSScriptRoot }
    while ($dir) {
        if (Test-Path (Join-Path $dir 'scripts\comfyhub.ps1')) { return $dir }
        $parent = Split-Path $dir -Parent
        if (-not $parent -or $parent -eq $dir) { break }
        $dir = $parent
    }
    return (Split-Path $PSScriptRoot -Parent)
}

# 一个进程对象都没有时返回 $null（调用方常见写法是 `if (-not $p)`）
function Get-RustServerProcess {
    Get-CimInstance Win32_Process -Filter "Name='comfyhub-server-rs.exe'" -ErrorAction SilentlyContinue
}

function Get-KotlinServerProcess {
    Get-CimInstance Win32_Process -Filter "Name='java.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -and $_.CommandLine -like '*com.comfyhub.ApplicationKt*' }
}

<#  两个后端**都可能**在跑（切流期间尤其）：停的时候要一起停，否则两个进程抢同一个端口，
    表现是"新后端起来了、健康检查却打到旧进程上"（或直接 bind 失败）。  #>
function Get-AnyBackendProcess {
    @(Get-RustServerProcess) + @(Get-KotlinServerProcess)
}

function Get-BackendProcess {
    param([string]$Kind)
    if ($Kind -eq 'rust') { return Get-RustServerProcess }
    return Get-KotlinServerProcess
}

function Resolve-RustExe {
    param([string]$ProjectRoot)
    $root = if ($ProjectRoot) { $ProjectRoot } else { Resolve-ProjectRootForBackend }
    # ⚠ `COMFYHUB_SERVER_EXE` 优先级最高，**这不是给多套构建用的**（那是
    #   `server-rs\target\release` vs `debug` 的事，下面按顺序认就行）——
    #   它唯一非做不可的理由是：**把后端映像挪出工作区**。
    #
    # 2026-09-29 实测：写不进工作区外的那个限制是按**映像位置**判的，不是按路径、
    # 也不是按用户 —— 同一个二进制逐字节相同，映像放在工作区内就写不了工作区外，
    # 复制到 `%TEMP%` 立刻哪儿都能写（`docs/pitfalls.md` 第 159 条的现场）。
    # 而下面三个候选**全在项目根下**，所以源码树布局下后端必然被限住：
    # `comfy_start` 写不进 ComfyUI 的 temp/、`comfy_use_media` 写不进 input。
    # 以前"无退路"的说法就是漏了这个开关 —— 现在有：把 exe 复制到工作区外，指过来。
    if ($env:COMFYHUB_SERVER_EXE) {
        $explicit = $env:COMFYHUB_SERVER_EXE.Trim()
        if ($explicit -and (Test-Path $explicit)) { return $explicit }
        Write-Host "==> COMFYHUB_SERVER_EXE=$explicit 不存在，当没设" -ForegroundColor Yellow
    }
    foreach ($cand in @(
            (Join-Path $root 'server\comfyhub-server-rs.exe'),
            (Join-Path $root 'server-rs\target\release\comfyhub-server-rs.exe'),
            (Join-Path $root 'server-rs\target\debug\comfyhub-server-rs.exe')
        )) {
        if (Test-Path $cand) { return $cand }
    }
    return $null
}

function Resolve-Backend {
    param(
        [string]$Preference,
        [string]$ProjectRoot
    )
    $root = if ($ProjectRoot) { $ProjectRoot } else { Resolve-ProjectRootForBackend }
    $want = if ($Preference) { $Preference } elseif ($env:COMFYHUB_BACKEND) { $env:COMFYHUB_BACKEND } else { 'auto' }
    $want = $want.Trim().ToLowerInvariant()
    if ($want -notin @('auto', 'rust', 'kotlin')) {
        Write-Host "==> COMFYHUB_BACKEND=$want 不认识（只认 auto / rust / kotlin），按 auto 处理" -ForegroundColor Yellow
        $want = 'auto'
    }
    $rust = Resolve-RustExe -ProjectRoot $root

    if ($want -eq 'kotlin') {
        return @{ Kind = 'kotlin'; Exe = $null; Note = 'COMFYHUB_BACKEND=kotlin（显式指定）'; RustExe = $rust }
    }
    if ($rust) {
        $note = if ($want -eq 'rust') { 'COMFYHUB_BACKEND=rust（显式指定）' } else { '自动：找到 Rust 二进制' }
        return @{ Kind = 'rust'; Exe = $rust; Note = $note; RustExe = $rust }
    }
    if ($want -eq 'rust') {
        Write-Host '==> 指定了 Rust 后端，但找不到二进制；回退 Kotlin（构建：cargo build --release --manifest-path server-rs\Cargo.toml）' -ForegroundColor Yellow
        return @{ Kind = 'kotlin'; Exe = $null; Note = '指定 rust 但没有二进制（已回退 Kotlin）'; RustExe = $null }
    }
    return @{ Kind = 'kotlin'; Exe = $null; Note = '自动：没找到 Rust 二进制，用 Kotlin'; RustExe = $null }
}

<#  二进制构建新鲜度：`server-rs\src` 里最新的 .rs 是否比 exe 新。
    界面类修复踩过这个坑（AGENTS §1："我改了但界面没变"先查构建新鲜度）——
    后端同理：`comfyhub.ps1 doctor` 要能一眼看出"你改的 Rust 源码还没编译"。  #>
function Get-RustBuildFreshness {
    param([string]$Exe, [string]$ProjectRoot)
    if (-not $Exe -or -not (Test-Path $Exe)) { return $null }
    $root = if ($ProjectRoot) { $ProjectRoot } else { Resolve-ProjectRootForBackend }
    $srcDir = Join-Path $root 'server-rs\src'
    $exeTime = (Get-Item $Exe).LastWriteTime
    if (-not (Test-Path $srcDir)) {
        return @{ ExeTime = $exeTime; NewestSource = $null; Stale = $false }
    }
    $newest = Get-ChildItem -Path $srcDir -Recurse -Filter *.rs -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
    $stale = $false
    if ($newest) { $stale = $newest.LastWriteTime -gt $exeTime }
    return @{ ExeTime = $exeTime; NewestSource = $newest; Stale = $stale }
}
