<#
.SYNOPSIS
    ComfyHub「AI 文案审校」—— **一个独立的简单软件**（设计真源：`docs/copy-review-design.md` v2）。

.DESCRIPTION
    解决的问题：规则类文案散在四处（系统提示词 / 工具描述 / skill 磁盘文件 / Dart 界面字面量），
    没有一份清单、没有"我读过并确认"这个事实、批量改文案只能靠手改 + 全局搜索替换。

    范围（用户 2026-09-27 拍板 **只做 B**）：

        | scope    | 可改？ | 真源 |
        | ---      | ---    | --- |
        | `skill`  | ✅ 可改 | `skills\builtin\<name>\SKILL.md` 与 `<storage>\ai\skills\<name>\SKILL.md` |
        | `ui`     | ✅ 可改 | `lib\**` 里的 Dart 字面量（由 `scripts\copy-review-ui.json` 的锚点指路，文本**从源码现读**） |
        | `prompt` | ⛔ 只读 | **问本地后端** `/api/ai/run-kit` 的 `systemPrompt`（那才是"模型真正看到的那一份"） |
        | `tool`   | ⛔ 只读 | **问本地后端** `/api/ai/run-kit` 的 `tools[].description` |

    **引擎与呈现分开**（用户 2026-09-27 问过之后拍板形态 = 本地单文件网页）：
      · 引擎 = 本文件里的函数（枚举 / 解析 / diff / 写回 / 状态 / 校验）；
      · 动作只是薄薄一层（见下面 `.EXAMPLE`）；
      · `open` 把清单**嵌进一份自包含 HTML**（无外链、无服务器，`file://` 双击就能用），
        HTML 负责筛选 / 就地改文本 / 批量预览 / 导出改动；
      · **HTML 里一个字节都不写文件** —— 唯一的写盘入口是本脚本的 `apply`。

    两条不能破的纪律：
      1. `apply` 默认 **dry-run**（只出 diff），要真写必须显式 `-Apply`；写之前每条先备份
         `<file>.copyreview-bak`（同名只留最近一次）。
      2. 只读条目（`prompt` / `tool`）**没有任何写入口** —— 代码层面 `Assert-Editable` 拦住，
         HTML 里也不给编辑框（能看、能标"我读过"）。

    状态文件：`.run\copy-review\state.json`（本工具自己的，**不进主库、不进同步**）。
    `verifiedAt` **绑内容 sha256**：文本一变，"已校验"自动失效（回到"文本变了要重看"）。

    为什么 prompt / tool 靠后端而不是正则解析 Rust 源码（设计 §3 明说）：
    "解析哪一段是哪一段必须有唯一判据，解析不出来就如实列不出来，绝不猜一个 key 出来"。
    系统提示词是运行时拼装的（含 Provider / 模型 / 时间 / 记忆条数），
    Rust 源码里那条拼装链没有任何稳定分段标记 —— 靠正则去猜只会列出一堆假 key。
    所以：**后端在线就问它要**，不在线就如实报"没取到"，一个 prompt / tool 条目都不编。
    再补两条同样的纪律：运行时字段（Provider / 模型 / 当前时间 / 记忆条数…）整块**跳过**，
    不当文案列出来（列了就会天天显示"要重看"）；/api/ai/tools 只给"这次会话给了模型几个"，
    所以枚举工具描述走 run-kit，只有 check 的"点名对账"才去问全量的 /api/ai/tools。

    已知边界（**宁可少列，也不给错**；设计 §3 的"解析不出来就如实列不出来"）：
      · `ui.*` 的文本从 Dart 源码现读。**同一行内**的相邻字面量拼接
        （`'前半句' '后半句'`）能正确合起来；但 `'前半句'\n  '后半句'` 这种
        **跨行**拼接、且锚点落在**第二行**时，只给得出后半句 —— 工具会照实标成
        "解析不出来"并转只读，**不会**拿半句冒充整句。真实三个设置页里没有这种写法。
      · `ui.*` 文本里含 Dart 插值（`$var` / `${expr}`）的条目一律转只读（工具不求值）。
      · skill 正文里若出现 `'` 或 `\`，写回时会按 Dart 字面量转义，并做**往返自检**；
        自检不过就整条拒绝，绝不写坏源码。

.EXAMPLE
    pwsh -File scripts\copy-review.ps1 open
        生成 `.run\copy-review\copy-review.html` 并打开浏览器（主入口）。

    pwsh -File scripts\copy-review.ps1 list -Json
    pwsh -File scripts\copy-review.ps1 list -Scope skill,ui
    pwsh -File scripts\copy-review.ps1 show -Key skill.inpaint-composite.body
    pwsh -File scripts\copy-review.ps1 verify -Key prompt.s-03 -Note "读过，纪律没错"
    pwsh -File scripts\copy-review.ps1 unverify -Key prompt.s-03

    # 写盘**唯一**入口：吃浏览器导出的改动文件（默认 dry-run）
    pwsh -File scripts\copy-review.ps1 apply -In .run\copy-review\copy-review-edits.json
    pwsh -File scripts\copy-review.ps1 apply -In .run\copy-review\copy-review-edits.json -Apply

    # 命令行批量（同样先出 diff；与 -Apply 才落盘）
    pwsh -File scripts\copy-review.ps1 batch -Op replace -Find "旧串" -Replace "新串" -Scope ui
    pwsh -File scripts\copy-review.ps1 batch -Op regex -Find '只写不读' -Replace '只写不读（已复核）' -Keys ui.settings.provider_card_subtitle

    pwsh -File scripts\copy-review.ps1 export -Out .run\copy-review\copy-out.json
    pwsh -File scripts\copy-review.ps1 import -In .run\copy-review\copy-out.json -Apply
    pwsh -File scripts\copy-review.ps1 report -Out .run\copy-review\report.html
    pwsh -File scripts\copy-review.ps1 check
    pwsh -File scripts\copy-review.ps1 -SelfTest

.NOTES
    只改 `scripts/**` + README 的脚本速查一行；不碰 `server-rs/**` / `lib/**` / `android/**` / `db/**` / `docs/**`。
    本工具**只读后端**（`/api/ai/run-kit`），从不写它。
#>
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateSet('list', 'show', 'verify', 'unverify', 'set', 'batch', 'export', 'import', 'report', 'open', 'check', 'selftest')]
    [string]$Action = 'list',

    # 条目 key（show / verify / unverify / set）
    [string]$Key,

    # verify 的备注
    [string]$Note,

    # set -Key <k> -TextFile <path>（等价于只含一条 edits 的 apply）
    [string]$TextFile,

    # apply -In <edits.json> / import -In <json>
    [string]$In,

    # export / report / open 的输出路径
    [string]$Out,

    # batch -Op replace|regex
    [ValidateSet('replace', 'regex')]
    [string]$Op = 'replace',

    [string]$Find,
    [string]$Replace,

    # batch 的条目筛选（逗号分隔）
    [string[]]$Keys,

    # 范围筛选：prompt / tool / skill / ui；空 = 全部
    [string[]]$Scope,

    # 真写盘（默认 dry-run）
    [switch]$Apply,

    # 机器可读输出（list / show / check / batch / apply / import）
    [switch]$Json,

    # 自测（用临时目录造假的 skill + ui 清单，跑一遍全流程）
    [switch]$SelfTest,

    # 隐藏参数：自测用，指定临时"项目根"（不给就按 comfyhub.ps1 往上找）
    [string]$RootOverride,

    # 隐藏参数：自测跑完**保留**临时目录（排查自测本身用的）
    [switch]$KeepSelfTestTemp
)

$ErrorActionPreference = 'Continue'

# ─────────────────────────────────────────────────────────────────────────────
# 0. 常量与根目录解析（与仓库其它脚本同款：往上找 scripts\comfyhub.ps1）
# ─────────────────────────────────────────────────────────────────────────────

$Script:RelStatePath = '.run/copy-review/state.json'
$Script:RelUiManifestPath = 'scripts/copy-review-ui.json'
$Script:SkillBodyBudgetBytes = 131072   # 单个 SKILL.md 正文的长度预算（超出给提示，不拦）

<# 项目根：往上找 `scripts\comfyhub.ps1`（源码树与发布包两种布局都认，AGENTS §7）。 #>
function Resolve-CopyReviewRoot {
    param([string]$Start)
    if ($Start) {
        if (Test-Path $Start) { return (Resolve-Path $Start).Path }
        return $Start
    }
    $dir = $PSScriptRoot
    while ($dir) {
        if (Test-Path (Join-Path $dir 'scripts\comfyhub.ps1')) { return $dir }
        $parent = Split-Path $dir -Parent
        if (-not $parent -or $parent -eq $dir) { break }
        $dir = $parent
    }
    return (Split-Path $PSScriptRoot -Parent)
}

$Script:Root = Resolve-CopyReviewRoot -Start $RootOverride
$Script:StateDir = Join-Path $Script:Root '.run\copy-review'
$Script:StatePath = Join-Path $Script:Root $Script:RelStatePath
$Script:UiManifestPath = Join-Path $Script:Root $Script:RelUiManifestPath

# API 端口真源：`.run\api.json`（读不出来就如实说"后端没取到"，绝不猜端口）
$Script:ApiPortHelper = Join-Path $PSScriptRoot 'api-port.ps1'
$Script:BackendProbe = $null      # 懒解析一次，避免每个动作都去探端口

# 相对路径统一用 `/`，报告/JSON 里两边一致
function ConvertTo-RelPath {
    param([string]$Path)
    if ([string]::IsNullOrEmpty($Path)) { return '' }
    $full = $Path
    try { $full = [System.IO.Path]::GetFullPath($Path) } catch { }
    $rootFull = $Script:Root
    try { $rootFull = [System.IO.Path]::GetFullPath($Script:Root) } catch { }
    if ($full.StartsWith($rootFull, [System.StringComparison]::OrdinalIgnoreCase)) {
        $rel = $full.Substring($rootFull.Length).TrimStart('\', '/')
        if ($rel) { return ($rel -replace '\\', '/') }
    }
    return ($full -replace '\\', '/')
}

function Get-AbsPath {
    param([string]$Rel)
    if ([string]::IsNullOrEmpty($Rel)) { return '' }
    if ([System.IO.Path]::IsPathRooted($Rel)) { return $Rel }
    return (Join-Path $Script:Root ($Rel -replace '/', '\'))
}

# ─────────────────────────────────────────────────────────────────────────────
# 1. I/O 与哈希
# ─────────────────────────────────────────────────────────────────────────────

function Write-Utf8File {
    param([string]$Path, [string]$Text)
    $dir = Split-Path $Path -Parent
    if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
    $enc = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($Path, $Text, $enc)
}

<# 统一换行成 LF：哈希与 HTML 里的文本一律以 LF 口径为准（文件怎么写回去另说）。 #>
function ConvertTo-Lf {
    param([string]$Text)
    if ($null -eq $Text) { return '' }
    return ($Text -replace "`r`n", "`n" -replace "`r", "`n")
}

function Get-Sha256Text {
    param([string]$Text)
    if ($null -eq $Text) { $Text = '' }
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $bytes = [System.Text.Encoding]::UTF8.GetBytes($Text)
        return ([System.BitConverter]::ToString($sha.ComputeHash($bytes)) -replace '-', '').ToLowerInvariant()
    } finally {
        $sha.Dispose()
    }
}

function Get-Sha256Short {
    param([string]$Text)
    return (Get-Sha256Text -Text $Text).Substring(0, 16)
}

function Read-JsonFileOrNull {
    param([string]$Path)
    if (-not (Test-Path $Path)) { return $null }
    try { return (Get-Content -Path $Path -Raw -Encoding UTF8 -ErrorAction Stop | ConvertFrom-Json -ErrorAction Stop) } catch { return $null }
}

function ConvertTo-Array {
    param($Value)
    if ($null -eq $Value) { return @() }
    return @($Value)
}

# ─────────────────────────────────────────────────────────────────────────────
# 2. 状态文件（.run\copy-review\state.json）
# ─────────────────────────────────────────────────────────────────────────────

function Read-CopyReviewState {
    $raw = Read-JsonFileOrNull -Path $Script:StatePath
    $map = @{}
    if ($raw -and $raw.entries) {
        foreach ($e in (ConvertTo-Array $raw.entries)) {
            if (-not $e -or -not $e.key) { continue }
            $map[[string]$e.key] = [pscustomobject]@{
                Key        = [string]$e.key
                Sha256     = [string]$e.sha256
                VerifiedAt = [string]$e.verifiedAt
                Note       = [string]$e.note
            }
        }
    }
    return $map
}

function Save-CopyReviewState {
    param([hashtable]$State)
    $entries = @()
    foreach ($k in ($State.Keys | Sort-Object)) {
        $v = $State[$k]
        $entries += [ordered]@{
            key        = [string]$v.Key
            sha256     = [string]$v.Sha256
            verifiedAt = [string]$v.VerifiedAt
            note       = [string]$v.Note
        }
    }
    $payload = [ordered]@{
        version   = 1
        tool      = 'copy-review'
        root      = $Script:Root
        updatedAt = (Get-Date).ToUniversalTime().ToString('o')
        entries   = $entries
    }
    Write-Utf8File -Path $Script:StatePath -Text (($payload | ConvertTo-Json -Depth 8) + "`n")
}

<# 三态：unverified / verified / stale（文本变了要重看）。 #>
function Get-VerifyStatus {
    param([string]$Key, [string]$Sha256, [hashtable]$State)
    if (-not $State.ContainsKey($Key)) {
        return [pscustomobject]@{ Status = 'unverified'; VerifiedAt = ''; Note = ''; VerifiedSha = '' }
    }
    $rec = $State[$Key]
    $same = ($rec.Sha256 -eq $Sha256)
    return [pscustomobject]@{
        Status      = if ($same) { 'verified' } else { 'stale' }
        VerifiedAt  = $rec.VerifiedAt
        Note        = $rec.Note
        VerifiedSha = $rec.Sha256
    }
}

# ─────────────────────────────────────────────────────────────────────────────
# 3. Dart 字面量解析（ui.* 的"文本从源码现读"）
#    规则：锚点是文件里**唯一**的一段子串；从它往后遇到的第一个字符串字面量
#    就是这条文案。相邻的多个字面量按 Dart 的隐式拼接**合成一条**。
#    `$var` / `${expr}` 插值不在这里求值 —— 源码原样返回并标 interpolated=true。
# ─────────────────────────────────────────────────────────────────────────────


<# 解析完一个字符串字面量：返回 { Ok, Value, End（结束引号之后）, Interpolated }。 #>
function Read-DartStringLiteral {
    param([string]$Content, [int]$Pos)
    $q = $Content[$Pos]
    $i = $Pos + 1
    $triple = ($i + 1 -lt $Content.Length -and $Content[$i] -eq $q -and $Content[$i + 1] -eq $q)
    if ($triple) { $i += 2 }
    $sb = New-Object System.Text.StringBuilder
    $interp = $false
    while ($i -lt $Content.Length) {
        $c = $Content[$i]
        if ($c -eq '\') {
            if ($i + 1 -ge $Content.Length) { break }
            $n = $Content[$i + 1]
            switch ($n) {
                'n' { [void]$sb.Append("`n") }
                'r' { [void]$sb.Append("`r") }
                't' { [void]$sb.Append("`t") }
                'b' { [void]$sb.Append("`b") }
                'f' { [void]$sb.Append("`f") }
                'v' { [void]$sb.Append("`v") }
                '0' { [void]$sb.Append([char]0) }
                'x' {
                    $hex = $Content.Substring([Math]::Min($i + 2, $Content.Length - 1), [Math]::Min(2, $Content.Length - $i - 2))
                    $code = 0
                    if ([int]::TryParse($hex, [System.Globalization.NumberStyles]::HexNumber, $null, [ref]$code)) {
                        [void]$sb.Append([char]$code); $i += 2
                    } else { [void]$sb.Append($n) }
                }
                'u' {
                    if ($i + 2 -lt $Content.Length -and $Content[$i + 2] -eq '{') {
                        $close = $Content.IndexOf('}', $i + 3)
                        if ($close -gt 0) {
                            $hex = $Content.Substring($i + 3, $close - $i - 3)
                            $code = 0
                            if ([int]::TryParse($hex, [System.Globalization.NumberStyles]::HexNumber, $null, [ref]$code) -and $code -le 0x10FFFF) {
                                [void]$sb.Append([char]::ConvertFromUtf32($code))
                            }
                            $i = $close
                        }
                    } else {
                        $hex = $Content.Substring([Math]::Min($i + 2, $Content.Length - 1), [Math]::Min(4, $Content.Length - $i - 2))
                        $code = 0
                        if ([int]::TryParse($hex, [System.Globalization.NumberStyles]::HexNumber, $null, [ref]$code)) {
                            [void]$sb.Append([char]$code); $i += 4
                        }
                    }
                }
                default { [void]$sb.Append($n) }   # \' \" \\ \$ 一律还原成字符本身
            }
            $i += 2
            continue
        }
        if ($c -eq '$') { $interp = $true }
        if ($triple) {
            if ($c -eq $q -and $i + 2 -lt $Content.Length -and $Content[$i + 1] -eq $q -and $Content[$i + 2] -eq $q) {
                return [pscustomobject]@{ Ok = $true; Value = $sb.ToString(); End = ($i + 3); Interpolated = $interp }
            }
        } else {
            if ($c -eq $q) {
                return [pscustomobject]@{ Ok = $true; Value = $sb.ToString(); End = ($i + 1); Interpolated = $interp }
            }
            if ($c -eq "`n") { return [pscustomobject]@{ Ok = $false; Value = ''; End = $Pos; Interpolated = $false } }
        }
        [void]$sb.Append($c)
        $i++
    }
    return [pscustomobject]@{ Ok = $false; Value = ''; End = $Pos; Interpolated = $false }
}

<# 锚点所在那一行里的**所有**字面量起点（从左到右）。
   为什么要"所有"：一行里可能有多个字面量（`const Text('A'), Text('B')`），
   也可能锚点落在**第二个**字面量上（相邻拼接的第二行 `'后半句'`）——
   只挑"行里第一个引号"会把文案配错。

   ⚠ 必须返回 `[int[]]`，而且参数**不能**写成 `[int]$AnchorPos`：锚点可能不在
   第一次算出的候选里（跨行拼接时锚点在第二行、开引号在第一行），调用方会把
   整个数组传进来让它"每行都找一遍"；写成 `[int]` 会被强转成数组首元素并抛异常。 #>
function Get-DartLiteralStartsOnLine {
    param([string]$Content, $AnchorPos)
    $ends = New-Object System.Collections.Generic.List[int]
    foreach ($p0 in @($AnchorPos)) {
        $pos = [Math]::Max(0, [Math]::Min([int]$p0, $Content.Length - 1))
        $nl = $Content.LastIndexOf("`n", $pos)
        $lineStart = if ($nl -lt 0) { 0 } else { $nl + 1 }
        $lineEnd = $Content.IndexOf("`n", $pos)
        if ($lineEnd -lt 0) { $lineEnd = $Content.Length - 1 }
        $i = $lineStart
        while ($i -le $lineEnd) {
            $c = $Content[$i]
            if (($c -eq "'" -or $c -eq '"') -and -not $ends.Contains($i)) { [void]$ends.Add($i) }
            $i++
        }
    }
    return [int[]]$ends.ToArray()
}


<#
    这个位置的引号是不是**开引号**（而不是上一条字面量的闭引号）。
    判据看的是**前一个字符**，不是后一个：
      · 闭引号前面一定是**内容**（`'A'` 里是 `A`），
      · 开引号前面一定是**分隔符**（空白 / `(` / `[` / `,` / `=` …）。
    没有这条判据，向前拼接会把上一条的**闭引号**当成下一条的开引号，
    于是整句被拆开（实测：`'只读演示：' '相邻字面量会拼起来'` 只拿到 `'只读演示：'`）。
#>
function Test-DartQuoteOpens {
    param([string]$Content, [int]$Pos)
    $prev = if ($Pos -le 0) { ' ' } else { $Content[$Pos - 1] }
    if ($prev -eq "'" -or $prev -eq '"') { return $true }        # 空串 `''` 的左引号之前也是引号
    if ([char]::IsWhiteSpace($prev)) { return $true }
    return $false                                                # `A'` / `。'` → 闭引号
}

<#
    `$Pos` 是不是落在某条字符串字面量**内部**（而不是注释 / 参数名 / 行尾）。
    只看同一行：候选开引号都在该行内，逐条解析后比区间即可（不用逐字符扫全文件）。
#>
function Test-DartPositionInString {
    param([string]$Content, [int]$Pos)
    if ($Pos -lt 0 -or $Pos -ge $Content.Length) { return $false }
    foreach ($st in @(Get-DartLiteralStartsOnLine -Content $Content -AnchorPos $Pos)) {
        if ($st -ge $Pos) { continue }
        $lit = Read-DartStringLiteral -Content $Content -Pos $st
        if ($lit.Ok -and $lit.End -gt $Pos) { return $true }
    }
    return $false
}

<#
    解析"锚点指向的那条文案"。返回 { Ok, Reason, Text, Start, End, Interpolated, Line }。

    判据只有一条，且**不靠猜**（设计 §3）：
      ① 锚点必须真在某条字符串字面量里（`Test-DartPositionInString`）；
      ② 从**锚点所在行最左边**的那条字面量开始，一路向右吞掉中间只隔空白的相邻字面量
         （Dart 的隐式拼接）；拼接结果必须**仍然包含锚点**；
      ③ 任何一步对不上就**如实报"解析不出来"** —— 宁可不列这一条，也不猜一段文本出来。

    为什么"从最左边那条开始"：`'前半句'\n  '后半句'` 这类写法里，前半句在
    锚点所在那一条的**左边**；从中间那条开始向右吞是吞不到它的（实测会只拿到半句）。
#>
function Resolve-DartCopyTarget {
    param([string]$Content, [string]$Anchor)
    $idx = $Content.IndexOf($Anchor, [System.StringComparison]::Ordinal)
    if ($idx -lt 0) { return [pscustomobject]@{ Ok = $false; Reason = "锚点在文件里找不到：$Anchor"; Text = ''; Start = -1; End = -1; Interpolated = $false; Line = 0 } }
    $second = $Content.IndexOf($Anchor, $idx + 1, [System.StringComparison]::Ordinal)
    if ($second -ge 0) { return [pscustomobject]@{ Ok = $false; Reason = "锚点在文件里不唯一（出现 >=2 次）：$Anchor"; Text = ''; Start = -1; End = -1; Interpolated = $false; Line = 0 } }

    if (-not (Test-DartPositionInString -Content $Content -Pos $idx)) {
        $unclosed = $false
        foreach ($st in @(Get-DartLiteralStartsOnLine -Content $Content -AnchorPos $idx)) {
            if ($st -ge $idx) { continue }
            if (-not (Read-DartStringLiteral -Content $Content -Pos $st).Ok) { $unclosed = $true }
        }
        $why = if ($unclosed) { '字面量没解析完（引号不闭合？）' } else { '锚点落在了字符串外面（注释 / 参数名 / 行尾？）' }
        return [pscustomobject]@{ Ok = $false; Reason = "$why：$Anchor"; Text = ''; Start = -1; End = -1; Interpolated = $false; Line = 0 }
    }

    # 锚点所在行的候选（同一行内可能有多个字面量），以及上一行的候选
    $lineStarts = New-Object System.Collections.Generic.List[int]
    foreach ($st in @(Get-DartLiteralStartsOnLine -Content $Content -AnchorPos $idx)) { [void]$lineStarts.Add($st) }
    $prevStarts = New-Object System.Collections.Generic.List[int]
    $nl = $Content.LastIndexOf("`n", [Math]::Max(0, $idx - 1))
    if ($nl -gt 0) {
        $nl2 = $Content.LastIndexOf("`n", [Math]::Max(0, $nl - 1))
        foreach ($st in @(Get-DartLiteralStartsOnLine -Content $Content -AnchorPos ($nl2 + 1))) { [void]$prevStarts.Add($st) }
    }

    foreach ($first in (@($prevStarts) + @($lineStarts))) {
        $cur = Read-DartStringLiteral -Content $Content -Pos $first
        if (-not $cur.Ok) { continue }
        $parts = New-Object System.Collections.Generic.List[object]
        [void]$parts.Add([pscustomobject]@{ Start = $first; Lit = $cur })
        while ($true) {
            $j = $cur.End
            while ($j -lt $Content.Length -and ($Content[$j] -eq ' ' -or $Content[$j] -eq "`t" -or $Content[$j] -eq "`n" -or $Content[$j] -eq "`r")) { $j++ }
            if ($j -ge $Content.Length) { break }
            $ch = $Content[$j]
            if ($ch -ne "'" -and $ch -ne '"') { break }
            if ($j -eq $cur.End) { break }
            if (-not (Test-DartQuoteOpens -Content $Content -Pos $j)) { break }
            $nxt = Read-DartStringLiteral -Content $Content -Pos $j
            if (-not $nxt.Ok) { break }
            [void]$parts.Add([pscustomobject]@{ Start = $j; Lit = $nxt })
            $cur = $nxt
        }
        $joined = ''
        $interp = $false
        foreach ($p in $parts) {
            $joined += $p.Lit.Value
            if ($p.Lit.Interpolated) { $interp = $true }
        }
        if ($joined.IndexOf($Anchor, [System.StringComparison]::Ordinal) -lt 0) { continue }
        $startAt = $parts[0].Start
        $endAt = $parts[$parts.Count - 1].Lit.End
        $line = 1
        for ($k = 0; $k -lt $startAt -and $k -lt $Content.Length; $k++) { if ($Content[$k] -eq "`n") { $line++ } }
        return [pscustomobject]@{ Ok = $true; Reason = ''; Text = (ConvertTo-Lf $joined); Start = $startAt; End = $endAt; Interpolated = $interp; Line = $line }
    }

    return [pscustomobject]@{ Ok = $false; Reason = "相邻字面量拼接后仍然对不上锚点（源码写法太绕，工具不猜）：$Anchor"; Text = ''; Start = -1; End = -1; Interpolated = $false; Line = 0 }
}
<# 反方向：把一个普通文本写成 Dart 单引号字面量（写盘前会做往返校验）。 #>
function ConvertTo-DartLiteral {
    param([string]$Text)
    $t = ConvertTo-Lf $Text
    $sb = New-Object System.Text.StringBuilder
    foreach ($ch in $t.ToCharArray()) {
        switch ($ch) {
            '\' { [void]$sb.Append('\\') }
            "'" { [void]$sb.Append("\'") }
            '$' { [void]$sb.Append('\$') }
            "`n" { [void]$sb.Append('\n') }
            "`r" { [void]$sb.Append('\r') }
            "`t" { [void]$sb.Append('\t') }
            default { [void]$sb.Append($ch) }
        }
    }
    return "'" + $sb.ToString() + "'"
}

# ─────────────────────────────────────────────────────────────────────────────
# 4. skill 枚举（正文与 frontmatter 分开两条；不维护第二份真源）
# ─────────────────────────────────────────────────────────────────────────────

<# 两个投放口：`skills\builtin\*\SKILL.md` 与 `<storage>\ai\skills\*\SKILL.md`。哪个在就扫哪个。 #>
function Get-SkillRoots {
    $roots = @()
    $builtin = Join-Path $Script:Root 'skills\builtin'
    if (Test-Path $builtin) { $roots += [pscustomobject]@{ Kind = 'builtin'; Path = $builtin; Rel = 'skills/builtin' } }
    $user = Join-Path $Script:Root 'storage\ai\skills'
    if (Test-Path $user) { $roots += [pscustomobject]@{ Kind = 'user'; Path = $user; Rel = 'storage/ai/skills' } }
    return $roots
}

function New-SkillEntriesFromRoot {
    param([pscustomobject]$SkillRoot, [hashtable]$State)
    $items = @()
    foreach ($dir in (Get-ChildItem -Path $SkillRoot.Path -Directory -ErrorAction SilentlyContinue | Sort-Object Name)) {
        $file = Join-Path $dir.FullName 'SKILL.md'
        if (-not (Test-Path $file)) { continue }
        $name = $dir.Name
        $raw = $null
        try { $raw = [System.IO.File]::ReadAllText($file, [System.Text.Encoding]::UTF8) } catch { continue }
        $content = ConvertTo-Lf $raw
        $split = Split-SkillFrontmatter -Content $content

        $fmText = $split.Frontmatter
        $bodyText = $split.Body
        $fmInfo = Parse-SkillFrontmatter -Text $fmText

        $bodyKey = "skill.$name.body"
        $fmKey = "skill.$name.frontmatter"
        $rel = ConvertTo-RelPath $file
        $bodyNote = "来源：$($SkillRoot.Rel)（$($SkillRoot.Kind)）；正文长度 $(([System.Text.Encoding]::UTF8.GetByteCount($bodyText))) 字节"
        if (-not $split.HasFrontmatter) { $bodyNote += '；**文件没有 frontmatter**（后端 autoRegister 会补 name/description）' }
        if ($fmInfo.Error) { $bodyNote += "；frontmatter 解析报错：$($fmInfo.Error)" }

        $items += New-CopyEntry -Key $bodyKey -Scope 'skill' -Title "skill 正文：$name" -Text $bodyText -Source $rel -Line $split.BodyLine -Editable $true -Note $bodyNote -State $State -Extra ([ordered]@{ name = $name; root = $SkillRoot.Rel; kind = 'body' })

        $fmShown = if ($split.HasFrontmatter) { $fmText } else { '' }
        $fmNote = if ($split.HasFrontmatter) {
            "frontmatter（只读，AGENTS §10：已有 frontmatter 的文件一个字节都不许改）；name=$($fmInfo.Name)；descriptionChars=$($fmInfo.DescriptionLength)"
        } else {
            '没有 frontmatter —— 前端/后端会靠 autoRegister 补，这里没什么可看的'
        }
        $items += New-CopyEntry -Key $fmKey -Scope 'skill' -Title "skill frontmatter：$name" -Text $fmShown -Source $rel -Line 1 -Editable $false -Note $fmNote -State $State -Extra ([ordered]@{ name = $name; root = $SkillRoot.Rel; kind = 'frontmatter'; editableReason = 'frontmatter 由主程序按需补写，工具只读' })
    }
    return $items
}

<# 把 `--- ... ---` 与正文拆开（没 frontmatter 时 Frontmatter 为空）。 #>
function Split-SkillFrontmatter {
    param([string]$Content)
    $lines = $Content -split "`n"
    $has = $false
    if ($lines.Count -gt 0 -and $lines[0].Trim() -eq '---') {
        for ($i = 1; $i -lt $lines.Count; $i++) {
            if ($lines[$i].Trim() -eq '---') {
                $has = $true
                $fm = ($lines[1..($i - 1)] -join "`n")
                $bodyLines = if ($i + 1 -le $lines.Count - 1) { $lines[($i + 1)..($lines.Count - 1)] } else { @() }
                return [pscustomobject]@{
                    HasFrontmatter = $true
                    Frontmatter    = $fm
                    Body           = ($bodyLines -join "`n")
                    FmLine         = 1
                    BodyLine       = $i + 2
                }
            }
        }
    }
    return [pscustomobject]@{ HasFrontmatter = $false; Frontmatter = ''; Body = $Content; FmLine = 0; BodyLine = 1 }
}

<# 认 YAML 的 `key: value`、`key: |` / `key: >` 块标量与引号标量（够用即止，不引依赖）。 #>
function Parse-SkillFrontmatter {
    param([string]$Text)
    $name = ''
    $descLen = 0
    $errorText = ''
    if ([string]::IsNullOrWhiteSpace($Text)) {
        return [pscustomobject]@{ Name = ''; DescriptionLength = 0; Error = 'frontmatter 为空（后端会自动补 name/description）' }
    }
    $lines = $Text -split "`n"
    $i = 0
    while ($i -lt $lines.Count) {
        $line = $lines[$i]
        if ($line -match '^\s*#' -or [string]::IsNullOrWhiteSpace($line)) { $i++; continue }
        # 缩进行是**上一层 key 的值**（`metadata:` 下面的 `  trigger-words: [...]`），
        # 不是坏行 —— 直接跳过（实测：把这种合法嵌套报成了 "not key: value"）。
        if ($line -match '^\s') { $i++; continue }
        $m = [regex]::Match($line, '^([A-Za-z_][\w\-]*)\s*:\s*(.*)$')
        if (-not $m.Success) {
            # YAML 里的合法写法不止 key: value —— 别把它们当成"frontmatter 坏了"：
            # `- item`（块序列）、`  - item`（缩进的块序列）、`{a: 1}` / `[x, y]`（流式）
            # 都出现在真实的 SKILL.md 里（实测 `- webfetch` / `trigger-words: [...]` 被误报）。
            $trimmed = $line.Trim()
            $isYamlish = $trimmed.StartsWith('- ') -or $trimmed -eq '-' -or
                         $trimmed.StartsWith('{') -or $trimmed.StartsWith('[') -or
                         $trimmed.EndsWith(':') -or $trimmed -match '^\s+' -or
                         $trimmed -match '^-\s'
            if (-not $isYamlish -and -not $errorText) {
                $errorText = "第 $($i + 1) 行不是 YAML 的 key: value（$trimmed）"
            }
            $i++
            continue
        }
        $k = $m.Groups[1].Value
        $v = $m.Groups[2].Value.Trim().TrimEnd('#').Trim()
        if ($v -eq '|' -or $v -eq '>' -or $v -match '^[|>][-+]?$') {
            # 块标量：吃后续缩进行
            $buf = @()
            $i++
            while ($i -lt $lines.Count) {
                if ($lines[$i] -match '^\s+\S' -or [string]::IsNullOrWhiteSpace($lines[$i])) { $buf += $lines[$i].Trim(); $i++ } else { break }
            }
            $v = ($buf -join ' ').Trim()
        } else {
            $v = $v.Trim("'", '"')
            $i++
        }
        if ($k -eq 'name') { $name = $v }
        if ($k -eq 'description') { $descLen = $v.Length }
    }
    if (-not $name) { $errorText = if ($errorText) { $errorText } else { 'frontmatter 里没有 name' } }
    return [pscustomobject]@{ Name = $name; DescriptionLength = $descLen; Error = $errorText }
}

# ─────────────────────────────────────────────────────────────────────────────
# 5. ui 枚举（显式清单只记 key → {file, anchor}，文本从源码现读）
# ─────────────────────────────────────────────────────────────────────────────

function Get-UiManifest {
    $raw = Read-JsonFileOrNull -Path $Script:UiManifestPath
    if (-not $raw) {
        return [pscustomobject]@{ Ok = $false; Error = "UI 清单读不到或不是合法 JSON：$(ConvertTo-RelPath $Script:UiManifestPath)"; Entries = @() }
    }
    $list = if ($raw.entries) { ConvertTo-Array $raw.entries } else { @() }
    if ($list.Count -eq 0) { return [pscustomobject]@{ Ok = $false; Error = 'UI 清单里没有 entries'; Entries = @() } }
    return [pscustomobject]@{ Ok = $true; Error = ''; Entries = $list }
}

function New-UiEntries {
    param([hashtable]$State)
    $manifest = Get-UiManifest
    if (-not $manifest.Ok) { return @() }
    $items = @()
    foreach ($m in $manifest.Entries) {
        $fileRel = [string]$m.file
        $anchor = [string]$m.anchor
        $key = [string]$m.key
        $file = Get-AbsPath $fileRel
        $reason = ''
        $text = ''
        $line = 0
        $editable = $true
        if (-not (Test-Path $file)) {
            $reason = "文件不在：$fileRel"
            $editable = $false
        } else {
            $content = ConvertTo-Lf ([System.IO.File]::ReadAllText($file, [System.Text.Encoding]::UTF8))
            $t = Resolve-DartCopyTarget -Content $content -Anchor $anchor
            if (-not $t.Ok) {
                $reason = $t.Reason
                $editable = $false
            } else {
                $text = $t.Text
                $line = $t.Line
                if ($t.Interpolated) {
                    $editable = $false
                    $reason = '文本里含 Dart 插值（$var / ${expr}），不在这里求值 —— 只读展示源码原文'
                }
            }
        }
        $note = [string]$m.note
        if ($reason) { $note = if ($note) { "$note；⚠ $reason" } else { "⚠ $reason" } }
        $items += New-CopyEntry -Key $key -Scope 'ui' -Title ([string]$m.title) -Text $text -Source "${fileRel}:$line" -Line $line -Editable $editable -Note $note -State $State -Extra ([ordered]@{ anchor = $anchor; file = $fileRel })
    }
    return $items
}

# ─────────────────────────────────────────────────────────────────────────────
# 6. prompt / tool 枚举（**只读**，优先问本地后端；不在线就如实说"没取到"）
# ─────────────────────────────────────────────────────────────────────────────

function Get-BackendEndpoint {
    if ($Script:BackendProbe) { return $Script:BackendProbe }
    $port = 0
    $source = 'none'
    if (Test-Path $Script:ApiPortHelper) {
        . $Script:ApiPortHelper
        $state = Read-ApiPortState -ProjectRoot $Script:Root
        if ($state -and $state.Port -gt 0) { $port = $state.Port; $source = '.run/api.json' }
        else { $source = '没有 .run/api.json（或读不动）' }
    } else { $source = 'scripts/api-port.ps1 不在' }
    # ⚠ 判据是"**打得通** /api/health"，不是 `Test-ApiPortListening`：
    #   后者只测 TCP 能不能连上，跟"这家后端认不认这个端口"是两件事
    #   （实测：TCP 连不上、HTTP 却回 200，于是 prompt/tool 明明能取到却被判成"没取到"）。
    if ($port -gt 0) {
        try {
            $h = Invoke-RestMethod -Uri "http://127.0.0.1:$port/api/health" -TimeoutSec 3 -ErrorAction Stop
            if (-not $h) { $port = 0; $source = "api.json 记的是 $($state.Port)，但 /api/health 没回应" }
        } catch {
            $probePort = $port
            $port = 0
            $source = "api.json 记的是 $probePort，但 /api/health 打不通（后端没在跑？）"
        }
    }
    $Script:BackendProbe = [pscustomobject]@{ Port = $port; Source = $source }
    return $Script:BackendProbe
}

function Invoke-BackendGet {
    param([string]$Path, [int]$TimeoutSec = 10)
    $ep = Get-BackendEndpoint
    if ($ep.Port -le 0) { return $null }
    try {
        return (Invoke-RestMethod -Uri "http://127.0.0.1:$($ep.Port)$Path" -TimeoutSec $TimeoutSec -ErrorAction Stop)
    } catch {
        return $null
    }
}

<#
    从后端取 prompt / tool。
    返回 { Ok, Error, PromptVersion, SystemPrompt, NsfwOn, Tools = @() }。
#>
function Get-BackendCopySource {
    $ep = Get-BackendEndpoint
    if ($ep.Port -le 0) {
        return [pscustomobject]@{ Ok = $false; Error = "没取到后端（$($ep.Source)）—— 先 pwsh -File scripts\comfyhub.ps1 up；" +
            '按设计 §3：**绝不靠正则去猜 Rust 源码里的字符串**，所以这一档就如实空着'; PromptVersion = ''; SystemPrompt = ''; NsfwOn = $false; Tools = @() }
    }
    $kit = Invoke-BackendGet -Path '/api/ai/run-kit'
    if (-not $kit) {
        return [pscustomobject]@{ Ok = $false; Error = "后端在 $($ep.Port) 端口上但 /api/ai/run-kit 没回应 —— 没取到"; PromptVersion = ''; SystemPrompt = ''; NsfwOn = $false; Tools = @() }
    }
    $tools = @()
    foreach ($t in (ConvertTo-Array $kit.tools)) {
        if ($t -and $t.name) { $tools += [pscustomobject]@{ Name = [string]$t.name; Description = [string]$t.description; Scope = [string]$t.scope } }
    }
    return [pscustomobject]@{
        Ok            = $true
        Error         = ''
        PromptVersion = [string]$kit.promptVersion
        SystemPrompt  = ConvertTo-Lf ([string]$kit.systemPrompt)
        NsfwOn        = [bool]$kit.nsfwEnabled
        Tools         = $tools
    }
}

<#
    系统提示词切段。判据**只有一条且稳定**：空行分块 + 缩进退栈（§ 段落本来就是这么排的）。
    这样既不猜语义，也不会因为改了措辞就漂 —— 块序号 + 块首行组合成一个可复核的标题。
    `prompt.nsfw` 是**有意注入的那段**（AGENTS §10），单独摘出来一条。
#>
function Split-PromptBlocks {
    param([string]$Text)
    $lines = $Text -split "`n"
    $blocks = @()
    $cur = New-Object System.Collections.ArrayList
    foreach ($ln in $lines) {
        if ([string]::IsNullOrWhiteSpace($ln)) {
            if ($cur.Count -gt 0) { $blocks += , ($cur.ToArray()); $cur.Clear() | Out-Null }
            continue
        }
        [void]$cur.Add($ln)
    }
    if ($cur.Count -gt 0) { $blocks += , ($cur.ToArray()) }
    return $blocks
}

function New-PromptEntries {
    param($Src, [hashtable]$State)
    $items = @()
    if (-not $Src.Ok) { return $items }

    $lines = $Src.SystemPrompt -split "`n"
    $blocks = Split-PromptBlocks -Text $Src.SystemPrompt
    # 运行时字段（Provider / 模型 / 当前时间 / 工作目录 / 记忆条数 / 附件）每次取都在变，
    # 把它们当"文案"列出来只会天天显示"文本变了要重看" —— 整块跳过并如实说明。
    $volatile = @($blocks | Where-Object { (ConvertTo-Lf ($_ -join "`n")) -match '当前时间:|Provider:|Model:|工作目录（项目根）|本次请求附带图片附件|现有 \d+/\d+ 条' })
    $skip = @{}
    foreach ($b in $volatile) { $skip[($b -join "`n")] = $true }
    $note0 = if ($volatile.Count -gt 0) { "；已跳过 $($volatile.Count) 个**运行时块**（时间 / 模型 / 记忆条数…每次取都在变，列出来只会天天显示「要重看」）" } else { '' }

    $nsfwIdx = -1
    for ($i = 0; $i -lt $blocks.Count; $i++) {
        if ($blocks[$i][0].Trim() -eq '[MODE: UNRESTRICTED SANDBOX — STABLE]') { $nsfwIdx = $i; break }
    }
    $idx = 0
    for ($i = 0; $i -lt $blocks.Count; $i++) {
        if ($skip.ContainsKey(($blocks[$i] -join "`n"))) { continue }
        $idx++
        $blockText = (ConvertTo-Lf ($blocks[$i] -join "`n"))
        $isNsfw = ($i -eq $nsfwIdx)
        $key = if ($isNsfw) { 'prompt.nsfw' } else { ('prompt.s{0:d2}' -f $idx) }
        $title = if ($isNsfw) { '系统提示词：NSFW 人格段（有意注入，AGENTS §10）' } else { '系统提示词：块 ' + ('{0:d2}' -f $idx) + ' · ' + (Get-FirstLineTitle -Lines $blocks[$i]) }
        $note = "来自后端 /api/ai/run-kit（promptVersion=$($Src.PromptVersion)）；块 $idx；$($blockText.Length) 字符$note0"
        if ($isNsfw) { $note += '；**这段是用户有意注入的越狱人格提示词**，只读复核（要改先问用户）' }
        $items += New-CopyEntry -Key $key -Scope 'prompt' -Title $title -Text $blockText -Source 'backend:/api/ai/run-kit#systemPrompt' -Line 0 -Editable $false -Note $note -State $State -Extra ([ordered]@{ block = $idx; promptVersion = $Src.PromptVersion; editableReason = '系统提示词是编译期/运行时拼装的只读真源，本工具不提供写入口' })
    }
    return $items
}

function Get-FirstLineTitle {
    param([string[]]$Lines)
    $t = ($Lines | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | Select-Object -First 1)
    if (-not $t) { return '(空)' }
    $t = $t.Trim()
    if ($t.Length -gt 46) { $t = $t.Substring(0, 46) + '…' }
    return $t
}

function New-ToolEntries {
    param($Src, [hashtable]$State)
    $items = @()
    if (-not $Src.Ok) { return $items }
    $seen = @{}
    foreach ($t in $Src.Tools) {
        $name = $t.Name
        if (-not $name) { continue }
        if ($seen.ContainsKey($name)) { continue }
        $seen[$name] = $true
        $items += New-CopyEntry -Key "tool.$name.description" -Scope 'tool' -Title "工具描述：$name" -Text $t.Description -Source 'backend:/api/ai/run-kit#tools' -Line 0 -Editable $false -Note "来自后端 run-kit 的 tools[]（scope=$($t.Scope)）；只读" -State $State -Extra ([ordered]@{ tool = $name; editableReason = '工具描述真源在 server-rs 的 catalog 里，本工具不提供写入口' })
    }
    return $items
}

# ─────────────────────────────────────────────────────────────────────────────
# 7. 条目构造
# ─────────────────────────────────────────────────────────────────────────────

function New-CopyEntry {
    param(
        [string]$Key, [string]$Scope, [string]$Title, [string]$Text, [string]$Source,
        [int]$Line, [bool]$Editable, [string]$Note, [hashtable]$State, $Extra
    )
    $t = ConvertTo-Lf $Text
    $sha = Get-Sha256Text -Text $t
    $vs = Get-VerifyStatus -Key $Key -Sha256 $sha -State $State
    $entry = [ordered]@{
        key         = $Key
        scope       = $Scope
        title       = $Title
        source      = $Source
        line        = $Line
        editable    = $Editable
        sha256      = $sha
        verifiedAt  = $vs.VerifiedAt
        verifiedNote = $vs.Note
        note        = $Note
        text        = $t
        bytes       = [System.Text.Encoding]::UTF8.GetByteCount($t)
        chars       = $t.Length
        state       = $vs.Status
        stateLabel  = (Get-StateLabel $vs.Status)
        verifiedSha = $vs.VerifiedSha
    }
    if ($Extra) { foreach ($k in $Extra.Keys) { $entry[$k] = $Extra[$k] } }
    return [pscustomobject]$entry
}

function Get-StateLabel {
    param([string]$Status)
    switch ($Status) {
        'verified' { '已校验' }
        'stale' { '文本变了要重看' }
        default { '未校验' }
    }
}

<# 全量枚举：四个 scope 一次到位（后端不在线时 prompt/tool 两类如实为空）。 #>
function Get-AllCopyEntries {
    param([hashtable]$State)
    $items = @()
    foreach ($sr in (Get-SkillRoots)) { $items += New-SkillEntriesFromRoot -SkillRoot $sr -State $State }
    $items += New-UiEntries -State $State
    $src = Get-BackendCopySource
    $items += New-PromptEntries -Src $src -State $State
    $items += New-ToolEntries -Src $src -State $State
    return , $items
}

function Select-CopyEntries {
    param([object[]]$Entries, [string[]]$ScopeFilter, [string[]]$KeyFilter)
    $out = @($Entries)
    if ($ScopeFilter -and $ScopeFilter.Count -gt 0) {
        $set = @{}
        foreach ($s in $ScopeFilter) { foreach ($p in ($s -split ',')) { if ($p.Trim()) { $set[$p.Trim().ToLowerInvariant()] = $true } } }
        $out = @($out | Where-Object { $set.ContainsKey($_.scope.ToLowerInvariant()) })
    }
    if ($KeyFilter -and $KeyFilter.Count -gt 0) {
        $kset = @{}
        foreach ($s in $KeyFilter) { foreach ($p in ($s -split ',')) { if ($p.Trim()) { $kset[$p.Trim()] = $true } } }
        $out = @($out | Where-Object { $kset.ContainsKey($_.key) })
    }
    return , $out
}

function Assert-Editable {
    param($Entry)
    if (-not $Entry) { throw '没有这一条（key 写错了？先用 list 看）' }
    if (-not $Entry.editable) {
        $why = if ($Entry.PSObject.Properties['editableReason']) { [string]$Entry.editableReason } else { '该 scope 只读' }
        throw "「$($Entry.key)」是**只读**条目（$why）—— 本工具没有它的写入口"
    }
}

# ─────────────────────────────────────────────────────────────────────────────
# 8. diff（文件 + 行号 + 前后文）
# ─────────────────────────────────────────────────────────────────────────────

function Get-TextLineInfo {
    param([string]$Text)
    $t = ConvertTo-Lf $Text
    $arr = @($t -split "`n")
    return [pscustomobject]@{ Lines = $arr; HasTrailingNewline = $t.EndsWith("`n") }
}

<# 行级 LCS diff（行数大了会慢，超过阈值就退化成"整段替换"）。 #>
function Get-LcsOps {
    param([string[]]$Old, [string[]]$New)
    if ($Old.Count * $New.Count -gt 4000000) {
        $ops = @()
        foreach ($l in $Old) { $ops += [pscustomobject]@{ Type = 'del'; Text = $l } }
        foreach ($l in $New) { $ops += [pscustomobject]@{ Type = 'add'; Text = $l } }
        return , $ops
    }
    $n = $Old.Count; $m = $New.Count
    # ⚠ PowerShell 里 `$dp[$i+1, $j+1] + 1` 这种"二维索引 + 算术"会踩类型转换，
    #   所以先取到变量再算（实测：`+` 会报 "Object[] does not contain op_Addition"）。
    $dp = New-Object 'int[,]' ($n + 1), ($m + 1)
    for ($i = $n - 1; $i -ge 0; $i--) {
        for ($j = $m - 1; $j -ge 0; $j--) {
            if ($Old[$i] -ceq $New[$j]) {
                $v = [int]$dp[($i + 1), ($j + 1)] + 1
                $dp[$i, $j] = $v
            } else {
                $a = [int]$dp[($i + 1), $j]
                $b = [int]$dp[$i, ($j + 1)]
                $dp[$i, $j] = [Math]::Max($a, $b)
            }
        }
    }
    $result = New-Object System.Collections.ArrayList
    $a = 0; $b = 0
    while ($a -lt $n -and $b -lt $m) {
        if ($Old[$a] -ceq $New[$b]) { [void]$result.Add([pscustomobject]@{ Type = 'same'; Text = $Old[$a] }); $a++; $b++ }
        else {
            # ⚠ 同样：二维索引的结果先落变量再比，别写在一行里做算术/比较
            $down = [int]$dp[($a + 1), $b]
            $right = [int]$dp[$a, ($b + 1)]
            if ($down -ge $right) { [void]$result.Add([pscustomobject]@{ Type = 'del'; Text = $Old[$a] }); $a++ }
            else { [void]$result.Add([pscustomobject]@{ Type = 'add'; Text = $New[$b] }); $b++ }
        }
    }
    while ($a -lt $n) { [void]$result.Add([pscustomobject]@{ Type = 'del'; Text = $Old[$a] }); $a++ }
    while ($b -lt $m) { [void]$result.Add([pscustomobject]@{ Type = 'add'; Text = $New[$b] }); $b++ }
    return , $result.ToArray()
}

<# 统一 diff → { OldStart, OldCount, NewStart, NewCount, Lines = @(prefix + text) }，带 3 行上下文。 #>
function Get-UnifiedDiff {
    param([string]$OldText, [string]$NewText, [int]$Context = 3)
    $o = Get-TextLineInfo -Text $OldText
    $nw = Get-TextLineInfo -Text $NewText
    $ops = Get-LcsOps -Old $o.Lines -New $nw.Lines

    # 先给每条 op 记上旧/新行号
    $annotated = @()
    $ol = 1; $nl = 1
    foreach ($op in $ops) {
        $entry = [pscustomobject]@{ Type = $op.Type; Text = $op.Text; OldLine = $ol; NewLine = $nl }
        $annotated += $entry
        switch ($op.Type) {
            'same' { $ol++; $nl++ }
            'del' { $ol++ }
            'add' { $nl++ }
        }
    }

    $hunks = @()
    $i = 0
    while ($i -lt $annotated.Count) {
        if ($annotated[$i].Type -eq 'same') { $i++; continue }
        $start = [Math]::Max(0, $i - $Context)
        $end = $i
        $lastChange = $i
        $j = $i
        while ($j -lt $annotated.Count) {
            if ($annotated[$j].Type -ne 'same') { $lastChange = $j; $j++; continue }
            # 后面 $Context 行内还有变化就并进同一个 hunk
            $k = $j
            $sameRun = 0
            while ($k -lt $annotated.Count -and $annotated[$k].Type -eq 'same') { $sameRun++; $k++ }
            if ($k -lt $annotated.Count -and $sameRun -le ($Context * 2)) { $j = $k; continue }
            break
        }
        $end = [Math]::Min($annotated.Count - 1, $lastChange + $Context)
        $slice = @($annotated[$start..$end])
        $oldStart = ($slice | Where-Object { $_.Type -ne 'add' } | Select-Object -First 1).OldLine
        $newStart = ($slice | Where-Object { $_.Type -ne 'del' } | Select-Object -First 1).NewLine
        if (-not $oldStart) { $oldStart = 0 }
        if (-not $newStart) { $newStart = 0 }
        $oldCount = @($slice | Where-Object { $_.Type -ne 'add' }).Count
        $newCount = @($slice | Where-Object { $_.Type -ne 'del' }).Count
        $lines = @()
        foreach ($s in $slice) {
            $prefix = switch ($s.Type) { 'same' { ' ' } 'del' { '-' } default { '+' } }
            $lines += "$prefix$($s.Text)"
        }
        $hunks += [pscustomobject]@{ OldStart = $oldStart; OldCount = $oldCount; NewStart = $newStart; NewCount = $newCount; Lines = $lines }
        $i = $end + 1
    }
    return , $hunks
}

function Format-Hunks {
    param([string]$Key, [string]$Source, $Hunks)
    $out = @()
    foreach ($h in $Hunks) {
        $out += "  @@ -$($h.OldStart),$($h.OldCount) +$($h.NewStart),$($h.NewCount) @@  $Source"
        foreach ($l in $h.Lines) { $out += "  $l" }
    }
    return $out
}

# ─────────────────────────────────────────────────────────────────────────────
# 9. 写盘
# ─────────────────────────────────────────────────────────────────────────────

<# 先写 `<file>.copyreview-bak`（同名只留最近一次），再写新内容。 #>
function Backup-File {
    param([string]$Path)
    $bak = "$Path.copyreview-bak"
    Copy-Item -Path $Path -Destination $bak -Force
    return $bak
}

function Write-SkillBody {
    param($Entry, [string]$NewText)
    $file = Get-AbsPath $Entry.source
    $file = ($Entry.source -split ':')[0]
    $file = Get-AbsPath $file
    if (-not (Test-Path $file)) { throw "文件不在：$file" }
    $raw = [System.IO.File]::ReadAllText($file, [System.Text.Encoding]::UTF8)
    $crlf = $raw.Contains("`r`n")
    $content = ConvertTo-Lf $raw
    $split = Split-SkillFrontmatter -Content $content
    $newBody = ConvertTo-Lf $NewText
    $newContent = if ($split.HasFrontmatter) {
        $fm = $split.Frontmatter
        "---`n$fm`n---`n$newBody"
    } else { $newBody }
    if ($crlf) { $newContent = $newContent -replace "`n", "`r`n" }
    $bak = Backup-File -Path $file
    Write-Utf8File -Path $file -Text $newContent
    return [pscustomobject]@{ File = $file; Backup = $bak; Kind = 'skill-body' }
}

<# 给一段**新**文本挑一个定位锚点：取它的开头一段（越长越不容易撞车），
   并保证它在整份源码里**只出现一次**。写回之后就用它再解析一次做自检。 #>
function New-DartAnchorForText {
    param([string]$Content, [string]$Text)
    $t = ConvertTo-Lf $Text
    if ($t.Length -eq 0) { return '' }
    foreach ($len in @(24, 16, 12, 8, 4, 2, 1)) {
        if ($t.Length -lt $len) { continue }
        $cand = $t.Substring(0, $len)
        if ($cand.Contains("'") -or $cand.Contains('"') -or $cand.Contains('\')) { continue }
        $first = $Content.IndexOf($cand, [System.StringComparison]::Ordinal)
        if ($first -lt 0) { continue }
        $second = $Content.IndexOf($cand, $first + 1, [System.StringComparison]::Ordinal)
        if ($second -lt 0) { return $cand }
    }
    return ''
}

function Write-UiText {
    param($Entry, [string]$NewText)
    $fileRel = [string]$Entry.file
    $anchor = [string]$Entry.anchor
    $file = Get-AbsPath $fileRel
    if (-not (Test-Path $file)) { throw "文件不在：$fileRel" }
    $raw = [System.IO.File]::ReadAllText($file, [System.Text.Encoding]::UTF8)
    $crlf = $raw.Contains("`r`n")
    $content = ConvertTo-Lf $raw
    $target = Resolve-DartCopyTarget -Content $content -Anchor $anchor
    if (-not $target.Ok) { throw "定位失败：$($target.Reason)" }
    # 目标文本必须与清单那条一致，否则说明源码已经被人改过（不许按老清单盲写）
    if ($target.Text -cne (ConvertTo-Lf $Entry.text)) {
        throw "原文已变（清单记的是 sha $($Entry.sha256.Substring(0,16))，文件里读到的是别的文本）—— 重新 list 一次再改"
    }
    $literal = ConvertTo-DartLiteral -Text $NewText
    $checkContent = $content.Substring(0, $target.Start) + $literal + $content.Substring($target.End)
    # 写回自检：① 新文本在文件里能唯一定位 ② 解析回来逐字符相同
    $newAnchor = New-DartAnchorForText -Content $checkContent -Text $NewText
    if (-not $newAnchor) { throw '写回自检失败（新文本在源码里定位不到唯一的锚点）—— 这条没改' }
    $verify = Resolve-DartCopyTarget -Content $checkContent -Anchor $newAnchor
    if (-not $verify.Ok) { throw "写回自检失败（解析不回来）：$($verify.Reason)" }
    if ($verify.Text -cne (ConvertTo-Lf $NewText)) { throw '写回自检失败（字面量往返不一致）—— 这条没改' }
    $newContent = $checkContent
    if ($crlf) { $newContent = $newContent -replace "`n", "`r`n" }
    elseif ($raw.Contains("`r`n")) { $newContent = $newContent -replace "`n", "`r`n" }
    $bak = Backup-File -Path $file
    Write-Utf8File -Path $file -Text $newContent
    return [pscustomobject]@{ File = $file; Backup = $bak; Kind = 'ui-text' }
}

<# 一条改动：算出新文本 + diff；`-DoWrite` 时才落盘。逐条独立，异常只影响这一条。 #>
function Invoke-CopyChange {
    param($Entry, [string]$NewText, [switch]$DoWrite)
    $old = ConvertTo-Lf $Entry.text
    $new = ConvertTo-Lf $NewText
    $hunks = Get-UnifiedDiff -OldText $old -NewText $new
    $result = [ordered]@{
        key       = $Entry.key
        scope     = $Entry.scope
        source    = $Entry.source
        editable  = $Entry.editable
        oldSha256 = $Entry.sha256
        newSha256 = (Get-Sha256Text -Text $new)
        changed   = ($old -cne $new)
        hunks     = $hunks
        written   = $false
        backup    = ''
        skipped   = ''
        error     = ''
    }
    if (-not $result.changed) { $result.skipped = '文本没有变化（find 没命中 / 编辑内容与原文相同）' ; return [pscustomobject]$result }
    if (-not $Entry.editable) { $result.skipped = '只读条目，没有写入口'; return [pscustomobject]$result }
    if (-not $DoWrite) { return [pscustomobject]$result }
    try {
        $w = if ($Entry.scope -eq 'skill') { Write-SkillBody -Entry $Entry -NewText $new } else { Write-UiText -Entry $Entry -NewText $new }
        $result.written = $true
        $result.backup = ConvertTo-RelPath $w.Backup
    } catch {
        $result.error = $_.Exception.Message
    }
    return [pscustomobject]$result
}

function Show-ChangeResults {
    param([object[]]$Results, [bool]$DoWrite)
    $changed = @($Results | Where-Object { $_.changed })
    $written = @($Results | Where-Object { $_.written })
    $skipped = @($Results | Where-Object { $_.skipped })
    $failed = @($Results | Where-Object { $_.error })
    foreach ($r in $changed) {
        Write-Host ("—— {0}  [{1}]  {2}" -f $r.key, $r.scope, $r.source) -ForegroundColor Cyan
        foreach ($l in (Format-Hunks -Key $r.key -Source $r.source -Hunks $r.hunks)) { Write-Host $l }
        if ($r.written) { Write-Host ("   已写盘；备份：{0}" -f $r.backup) -ForegroundColor Green }
        elseif ($r.error) { Write-Host ("   失败：{0}" -f $r.error) -ForegroundColor Red }
        else { Write-Host '   （dry-run：没有写盘；要真写加 -Apply）' -ForegroundColor Yellow }
    }
    foreach ($r in $skipped) { Write-Host ("—— {0}：{1}" -f $r.key, $r.skipped) -ForegroundColor DarkGray }
    foreach ($r in $failed) { Write-Host ("—— {0}：失败 {1}" -f $r.key, $r.error) -ForegroundColor Red }
    Write-Host ''
    Write-Host ("命中改动 {0} 条 / 写入 {1} 条 / 跳过 {2} 条 / 失败 {3} 条（{4}）" -f `
        $changed.Count, $written.Count, $skipped.Count, $failed.Count, $(if ($DoWrite) { '已 -Apply' } else { 'dry-run' })) `
        -ForegroundColor $(if ($failed.Count -gt 0) { 'Red' } elseif ($changed.Count -eq 0) { 'Yellow' } else { 'Green' })
}

# ─────────────────────────────────────────────────────────────────────────────
# 10. 浏览器那份 HTML（自包含、无外链；**一个字节都不写文件**）
# ─────────────────────────────────────────────────────────────────────────────

function Get-HtmlEditorTemplate {
    return @'
<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>ComfyHub 文案审校</title>
<style>
:root{--bg:#14161a;--panel:#1c1f26;--panel2:#22262f;--line:#31363f;--fg:#e6e8ec;--dim:#9aa3b2;
--ok:#3fb950;--stale:#d29922;--new:#8b949e;--ro:#58a6ff;--bad:#f85149;--accent:#7c8cff}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--fg);font:14px/1.6 "Segoe UI","Microsoft YaHei",system-ui,sans-serif}
header{position:sticky;top:0;z-index:5;background:var(--panel);border-bottom:1px solid var(--line);padding:10px 16px}
h1{font-size:16px;margin:0 0 6px}
.meta{color:var(--dim);font-size:12px}
.bar{display:flex;gap:8px;flex-wrap:wrap;align-items:center;margin-top:8px}
input[type=text],input[type=search],select,textarea{background:var(--panel2);color:var(--fg);border:1px solid var(--line);border-radius:6px;padding:5px 8px;font:inherit}
button{background:var(--panel2);color:var(--fg);border:1px solid var(--line);border-radius:6px;padding:5px 10px;cursor:pointer;font:inherit}
button:hover{border-color:var(--accent)}
button.primary{background:var(--accent);border-color:var(--accent);color:#0b0d12;font-weight:600}
label.chk{display:inline-flex;gap:5px;align-items:center;color:var(--dim);font-size:12px}
main{padding:12px 16px 120px}
.group{margin:18px 0 6px;font-size:13px;color:var(--dim);letter-spacing:.06em;text-transform:uppercase;border-bottom:1px solid var(--line);padding-bottom:4px}
.card{background:var(--panel);border:1px solid var(--line);border-radius:8px;margin:8px 0;padding:10px 12px}
.card.dirty{border-color:var(--accent)}
.head{display:flex;gap:8px;flex-wrap:wrap;align-items:center}
.key{font-family:ui-monospace,Consolas,monospace;font-size:13px;color:var(--fg)}
.title{color:var(--dim);font-size:12px;flex:1 1 auto}
.badge{font-size:11px;border-radius:10px;padding:1px 8px;border:1px solid var(--line);white-space:nowrap}
.b-verified{color:var(--ok);border-color:var(--ok)}
.b-stale{color:var(--stale);border-color:var(--stale)}
.b-unverified{color:var(--new)}
.b-ro{color:var(--ro);border-color:var(--ro)}
.b-rw{color:var(--dim)}
.src{font-family:ui-monospace,Consolas,monospace;font-size:11.5px;color:var(--dim)}
.note{color:var(--dim);font-size:12px;margin:6px 0 0;white-space:pre-wrap}
pre{background:var(--panel2);border:1px solid var(--line);border-radius:6px;padding:8px;max-height:320px;overflow:auto;white-space:pre-wrap;word-break:break-word;margin:8px 0 0;font:12.5px/1.55 ui-monospace,Consolas,monospace}
textarea.ed{width:100%;min-height:120px;margin-top:8px;font:12.5px/1.55 ui-monospace,Consolas,monospace}
.row{display:flex;gap:8px;align-items:center;margin-top:8px;flex-wrap:wrap}
.diff{font:12.5px/1.5 ui-monospace,Consolas,monospace;white-space:pre-wrap}
.diff .add{color:var(--ok)}.diff .del{color:var(--bad)}.diff .hdr{color:var(--accent)}
#toast{position:fixed;right:16px;bottom:16px;background:var(--panel2);border:1px solid var(--line);border-radius:8px;padding:10px 14px;max-width:520px;display:none;z-index:20}
footer{position:fixed;left:0;right:0;bottom:0;background:var(--panel);border-top:1px solid var(--line);padding:8px 16px;display:flex;gap:10px;align-items:center;z-index:6}
.count{color:var(--dim);font-size:12px}
details.batch{background:var(--panel);border:1px solid var(--line);border-radius:8px;padding:10px 12px;margin:10px 0}
summary{cursor:pointer}
</style>
</head>
<body>
<header>
  <h1>ComfyHub 文案审校 <span class="meta">（本地单文件；写盘只由 <code>scripts\copy-review.ps1 apply</code> 执行）</span></h1>
  <div class="meta" id="meta"></div>
  <div class="bar">
    <input type="search" id="q" placeholder="关键字筛选（key / 标题 / 文本）" style="min-width:280px">
    <select id="scope"><option value="">全部 scope</option></select>
    <label class="chk"><input type="checkbox" id="onlyPending">只看待校验（未校验 + 要重看）</label>
    <label class="chk"><input type="checkbox" id="onlyDirty">只看我改过的</label>
    <label class="chk"><input type="checkbox" id="onlyRo">只看只读</label>
    <button id="btnExport" class="primary">导出改动</button>
  </div>
</header>
<main>
  <details class="batch">
    <summary>批量面板（find/replace 或正则）—— 先出预览 diff，再进"导出改动"</summary>
    <div class="bar">
      <select id="bOp"><option value="replace">纯文本 replace</option><option value="regex">正则 regex</option></select>
      <input type="text" id="bFind" placeholder="find" style="min-width:240px">
      <input type="text" id="bReplace" placeholder="replace" style="min-width:240px">
      <select id="bScope"><option value="">全部可改 scope</option><option value="ui">ui</option><option value="skill">skill</option></select>
      <button id="bPreview">预览 diff</button>
      <button id="bApply" class="primary">加入改动</button>
    </div>
    <pre id="bDiff">（还没有预览）</pre>
  </details>
  <div id="list"></div>
</main>
<footer>
  <span class="count" id="count"></span>
  <span class="count" id="dirtyCount"></span>
  <span class="count">导出后跑：<code>pwsh -File scripts\copy-review.ps1 apply -In .run\copy-review\copy-review-edits.json</code>（默认 dry-run）</span>
</footer>
<div id="toast"></div>
<script id="copy-review-data" type="application/json">__PAYLOAD__</script>
<script>
(function(){
  var DATA = JSON.parse(document.getElementById('copy-review-data').textContent);
  var entries = DATA.entries.slice();
  var dirty = {};        // key -> 新文本
  var verifyMark = {};   // key -> note（浏览器里标的"我读过"）
  var undoMark = {};     // key -> true（浏览器里取消的"我读过"）

  function $(id){ return document.getElementById(id); }
  function esc(s){ return (s==null?'':String(s)).replace(/[&<>"]/g,function(c){return {'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c];}); }
  function lines(s){ return s.split('\n'); }
  function toast(msg){ var t=$('toast'); t.innerHTML=msg; t.style.display='block'; clearTimeout(t._h); t._h=setTimeout(function(){t.style.display='none';},6000); }

  // ---- 行级 LCS diff（与脚本侧同一套判据，用于"先出预览"） ----
  function diff(oldText,newText,ctx){
    var a=lines(oldText), b=lines(newText), n=a.length, m=b.length;
    if(n*m>4000000){ return [{hdr:'(太大，整段替换)',lines:[]}]; }
    var dp=[]; for(var i=0;i<=n;i++){ dp.push(new Array(m+1).fill(0)); }
    for(i=n-1;i>=0;i--){ for(var j=m-1;j>=0;j--){ dp[i][j] = (a[i]===b[j]) ? dp[i+1][j+1]+1 : Math.max(dp[i+1][j],dp[i][j+1]); } }
    var ops=[], x=0, y=0;
    while(x<n&&y<m){ if(a[x]===b[y]){ops.push([' ',a[x],x+1,y+1]);x++;y++;} else if(dp[x+1][y]>=dp[x][y+1]){ops.push(['-',a[x],x+1,0]);x++;} else {ops.push(['+',b[y],0,y+1]);y++;} }
    while(x<n){ops.push(['-',a[x],x+1,0]);x++;}
    while(y<m){ops.push(['+',b[y],0,y+1]);y++;}
    var out=[], i2=0;
    while(i2<ops.length){
      if(ops[i2][0]===' '){i2++;continue;}
      var st=Math.max(0,i2-ctx), last=i2, k=i2;
      while(k<ops.length){
        if(ops[k][0]!==' '){last=k;k++;continue;}
        var run=0, kk=k; while(kk<ops.length&&ops[kk][0]===' '){run++;kk++;}
        if(kk<ops.length&&run<=ctx*2){k=kk;continue;} break;
      }
      var en=Math.min(ops.length-1,last+ctx);
      var slice=ops.slice(st,en+1);
      var os=null,ns=null;
      for(var q=0;q<slice.length;q++){ if(slice[q][0]!== '+'&&os===null)os=slice[q][2]; if(slice[q][0]!=='-'&&ns===null)ns=slice[q][3]; }
      out.push({hdr:'@@ -'+(os||0)+' +'+(ns||0)+' @@', lines: slice.map(function(s){return s[0]+s[1];})});
      i2=en+1;
    }
    return out;
  }
  function renderDiff(hunks){
    if(!hunks.length) return '<span class="hdr">（没有差异）</span>';
    return hunks.map(function(h){ return '<span class="hdr">'+esc(h.hdr)+'</span>\n'+h.lines.map(function(l){
      var cls = l[0]==='+'?'add':(l[0]==='-'?'del':'');
      return '<span class="'+cls+'">'+esc(l)+'</span>';
    }).join('\n'); }).join('\n');
  }

  function stateOf(e){
    if(verifyMark[e.key]) return 'verified';
    if(undoMark[e.key]) return 'unverified';
    return e.state;
  }
  function badge(e){
    var st=stateOf(e), h='';
    var label = st==='verified'?'已校验':(st==='stale'?'文本变了要重看':'未校验');
    h += '<span class="badge b-'+st+'">'+esc(label)+'</span>';
    h += e.editable ? '<span class="badge b-rw">可改</span>' : '<span class="badge b-ro">只读</span>';
    if(dirty[e.key]!==undefined) h += '<span class="badge" style="color:var(--accent);border-color:var(--accent)">我改过</span>';
    return h;
  }

  function render(){
    var q=$('q').value.trim().toLowerCase(), sc=$('scope').value;
    var onlyPending=$('onlyPending').checked, onlyDirty=$('onlyDirty').checked, onlyRo=$('onlyRo').checked;
    var box=$('list'); box.innerHTML='';
    var groups={}, order=[];
    entries.forEach(function(e){
      if(sc && e.scope!==sc) return;
      if(onlyPending && stateOf(e)==='verified') return;
      if(onlyDirty && dirty[e.key]===undefined) return;
      if(onlyRo && e.editable) return;
      if(q){
        var hay=(e.key+' '+e.title+' '+(e.note||'')+' '+e.text).toLowerCase();
        if(hay.indexOf(q)<0) return;
      }
      if(!groups[e.scope]){ groups[e.scope]=[]; order.push(e.scope); }
      groups[e.scope].push(e);
    });
    var shown=0;
    order.forEach(function(scope){
      var el=document.createElement('div');
      el.className='group'; el.textContent = DATA.scopeLabels[scope]+'（'+groups[scope].length+' 条）';
      box.appendChild(el);
      groups[scope].forEach(function(e){
        shown++;
        var cur = dirty[e.key]!==undefined ? dirty[e.key] : e.text;
        var c=document.createElement('div');
        c.className='card'+(dirty[e.key]!==undefined?' dirty':'');
        var html='';
        html+='<div class="head"><span class="key">'+esc(e.key)+'</span>'+badge(e)+'<span class="title">'+esc(e.title)+'</span></div>';
        html+='<div class="row"><span class="src">来源：'+esc(e.source)+'</span><span class="src">sha256 '+esc(e.sha256.slice(0,16))+' · '+e.chars+' 字符 · '+e.bytes+' 字节</span></div>';
        if(e.note) html+='<p class="note">'+esc(e.note)+'</p>';
        if(e.editable){
          html+='<textarea class="ed" data-key="'+esc(e.key)+'">'+esc(cur)+'</textarea>';
          html+='<div class="row"><button data-act="reset" data-key="'+esc(e.key)+'">还原</button>'+
                '<button data-act="toggleverify" data-key="'+esc(e.key)+'">'+(stateOf(e)==='verified'?'取消"已校验"':'标记"我读过"')+'</button>'+
                '<button data-act="diff" data-key="'+esc(e.key)+'">看 diff</button></div>';
        }else{
          html+='<pre>'+esc(e.text)+'</pre>';
          html+='<div class="row"><button data-act="toggleverify" data-key="'+esc(e.key)+'">'+(stateOf(e)==='verified'?'取消"已校验"':'标记"我读过"')+'</button>'+
                '<span class="src">只读条目：只能看 / 只能标"我读过"</span></div>';
        }
        html+='<pre class="diff" id="d-'+esc(e.key)+'" style="display:none"></pre>';
        c.innerHTML=html;
        box.appendChild(c);
      });
    });
    if(!shown){ box.innerHTML='<div class="group">没有符合条件的条目</div>'; }
    var dirtyN=Object.keys(dirty).length;
    $('count').textContent='显示 '+shown+' / 共 '+entries.length+' 条';
    $('dirtyCount').textContent = dirtyN ? ('已改动 '+dirtyN+' 条（导出后 apply 才会落盘）') : '还没有改动';
  }

  document.addEventListener('input', function(ev){
    var t=ev.target;
    if(t.classList && t.classList.contains('ed')){
      var k=t.getAttribute('data-key');
      var e=entries.filter(function(x){return x.key===k;})[0];
      if(t.value===e.text){ delete dirty[k]; } else { dirty[k]=t.value; }
      var card=t.closest('.card'); if(card){ card.className='card'+(dirty[k]!==undefined?' dirty':''); }
      var st=card?card.querySelector('.head'):null;
      $('dirtyCount').textContent = Object.keys(dirty).length ? ('已改动 '+Object.keys(dirty).length+' 条（导出后 apply 才会落盘）') : '还没有改动';
    }
  });

  document.addEventListener('click', function(ev){
    var b=ev.target.closest('button'); if(!b) return;
    var act=b.getAttribute('data-act'); if(!act) return;
    var k=b.getAttribute('data-key');
    var e=entries.filter(function(x){return x.key===k;})[0];
    if(act==='reset'){ delete dirty[k]; render(); return; }
    if(act==='toggleverify'){
      if(stateOf(e)==='verified'){ delete verifyMark[k]; if(e.state==='verified'){ undoMark[k]=true; } }
      else { verifyMark[k]= (e.note? '':''); delete undoMark[k]; }
      render(); return;
    }
    if(act==='diff'){
      var cur = dirty[k]!==undefined?dirty[k]:e.text;
      var pre=$('d-'+k); pre.style.display='block';
      pre.innerHTML=renderDiff(diff(e.text,cur,3));
      return;
    }
  });

  $('q').addEventListener('input',render);
  $('scope').addEventListener('change',render);
  $('onlyPending').addEventListener('change',render);
  $('onlyDirty').addEventListener('change',render);
  $('onlyRo').addEventListener('change',render);

  // ---- 批量面板：先在浏览器里算，落到"改动"里，导出时变成 edits ----
  function batchTargets(){
    var sc=$('bScope').value;
    return entries.filter(function(e){ return e.editable && (!sc || e.scope===sc); });
  }
  function computeBatch(){
    var op=$('bOp').value, find=$('bFind').value, rep=$('bReplace').value;
    if(!find) return {err:'find 不能为空', plan:[]};
    var re=null;
    if(op==='regex'){ try{ re=new RegExp(find,'g'); }catch(err){ return {err:'正则不合法：'+err.message, plan:[]}; } }
    var plan=[];
    batchTargets().forEach(function(e){
      var cur = dirty[e.key]!==undefined?dirty[e.key]:e.text;
      var next = (op==='regex') ? cur.replace(re,rep) : cur.split(find).join(rep);
      if(next!==cur) plan.push({key:e.key, entry:e, next:next});
    });
    return {err:'', plan:plan};
  }
  $('bPreview').addEventListener('click',function(){
    var r=computeBatch();
    if(r.err){ $('bDiff').textContent=r.err; return; }
    if(!r.plan.length){ $('bDiff').textContent='0 处命中（没有条目会变）—— 不会有任何改动'; return; }
    var html='命中 '+r.plan.length+' 条：\n\n';
    r.plan.forEach(function(p){ html+='### '+p.key+'  ('+p.entry.source+')\n'+renderDiff(diff(p.entry.text,p.next,3))+'\n\n'; });
    $('bDiff').innerHTML=html;
  });
  $('bApply').addEventListener('click',function(){
    var r=computeBatch();
    if(r.err){ toast(r.err); return; }
    if(!r.plan.length){ toast('0 处命中 —— 一条都没改'); return; }
    r.plan.forEach(function(p){ dirty[p.key]=p.next; });
    render();
    toast('已把 '+r.plan.length+' 条的改动放进"我改过"（还没落盘）');
  });

  // ---- 导出：唯一形状（与脚本 apply 的入参一致） ----
  $('btnExport').addEventListener('click',function(){
    var payload={version:1, generatedAt:new Date().toISOString(), root:DATA.root, edits:[], batch:[], verify:[]};
    Object.keys(dirty).forEach(function(k){ payload.edits.push({key:k, text:dirty[k]}); });
    Object.keys(verifyMark).forEach(function(k){ payload.verify.push({key:k, note:verifyMark[k]||''}); });
    var blob=new Blob([JSON.stringify(payload,null,2)],{type:'application/json'});
    var a=document.createElement('a');
    a.href=URL.createObjectURL(blob); a.download='copy-review-edits.json';
    document.body.appendChild(a); a.click(); document.body.removeChild(a);
    toast('已导出 copy-review-edits.json（'+payload.edits.length+' 条改动 / '+payload.verify.length+' 条校验）。'+
      '写盘：<code>pwsh -File scripts\\copy-review.ps1 apply -In .run\\copy-review\\copy-review-edits.json</code>（先 dry-run，确认后加 -Apply）');
  });

  $('meta').textContent = 'root=' + DATA.root + ' · 生成于 ' + DATA.generatedAt + ' · 后端 ' + DATA.backendLabel;
  var sel=$('scope');
  Object.keys(DATA.scopeLabels).forEach(function(s){
    var n=entries.filter(function(e){return e.scope===s;}).length;
    if(!n) return;
    var o=document.createElement('option'); o.value=s; o.textContent=DATA.scopeLabels[s]+'（'+n+'）'; sel.appendChild(o);
  });
  render();
  if(DATA.warnings && DATA.warnings.length){
    toast(DATA.warnings.map(esc).join('<br>'));
  }
})();
</script>
</body>
</html>
'@
}

function New-OpenPayload {
    param([object[]]$Entries, $Src, [hashtable]$State)
    $warnings = @()
    if (-not $Src.Ok) { $warnings += "prompt / tool 两类**没取到**：$($Src.Error)" }
    $broken = @($Entries | Where-Object { $_.scope -eq 'ui' -and -not $_.editable -and $_.note -notmatch '插值' })
    if ($broken.Count -gt 0) { $warnings += "有 $($broken.Count) 条 ui 条目的锚点没对上（清单要维护了）—— 它们在列表里标着原因。" }
    $interp = @($Entries | Where-Object { $_.scope -eq 'ui' -and -not $_.editable -and $_.note -match '插值' })
    if ($interp.Count -gt 0) { $warnings += "有 $($interp.Count) 条 ui 条目含 Dart 插值（`$var），按设计只读展示、不给编辑框。" }

    $out = @()
    foreach ($e in $Entries) {
        $o = [ordered]@{
            key        = $e.key
            scope      = $e.scope
            title      = $e.title
            source     = $e.source
            editable   = [bool]$e.editable
            sha256     = $e.sha256
            verifiedAt = $e.verifiedAt
            note       = $e.note
            text       = $e.text
            chars      = $e.chars
            bytes      = $e.bytes
            state      = $e.state
        }
        $out += $o
    }
    return [ordered]@{
        version      = 1
        tool         = 'copy-review'
        root         = $Script:Root
        generatedAt  = (Get-Date).ToUniversalTime().ToString('o')
        scopeLabels  = [ordered]@{ prompt = '系统提示词（只读）'; tool = '工具描述（只读）'; skill = 'skill 正文（可改）'; ui = '界面注解（可改）' }
        backendLabel = if ($Src.Ok) { "promptVersion=$($Src.PromptVersion) nsfw=$($Src.NsfwOn)" } else { '没取到' }
        statePath    = $Script:RelStatePath
        warnings     = $warnings
        entries      = $out
    }
}

<# 嵌进 HTML 的 JSON：把 `<` 转义掉，避免正文里出现 `</script>` 把脚本块提前关掉。 #>
function ConvertTo-EmbeddedJson {
    param($Payload)
    $json = $Payload | ConvertTo-Json -Depth 12 -Compress
    return ($json -replace '<', '\u003C')
}

# ─────────────────────────────────────────────────────────────────────────────
# 11. 动作
# ─────────────────────────────────────────────────────────────────────────────

function Invoke-ListAction {
    param([hashtable]$State)
    $all = Get-AllCopyEntries -State $State
    $sel = Select-CopyEntries -Entries $all -ScopeFilter $Scope -KeyFilter $Keys
    if ($Json) {
        $rows = @()
        foreach ($e in $sel) {
            $rows += [ordered]@{
                key        = $e.key
                scope      = $e.scope
                title      = $e.title
                source     = $e.source
                editable   = [bool]$e.editable
                sha256     = $e.sha256
                verifiedAt = $e.verifiedAt
                note       = $e.note
                state      = $e.state
                stateLabel = $e.stateLabel
            }
        }
        $payload = [ordered]@{ version = 1; root = $Script:Root; statePath = $Script:RelStatePath; count = $rows.Count; entries = $rows }
        Write-Output ($payload | ConvertTo-Json -Depth 8)
        return
    }
    $byScope = @{}
    foreach ($e in $sel) {
        if (-not $byScope.ContainsKey($e.scope)) { $byScope[$e.scope] = @() }
        $byScope[$e.scope] += $e
    }
    foreach ($scopeName in @('prompt', 'tool', 'skill', 'ui')) {
        if (-not $byScope.ContainsKey($scopeName)) { continue }
        $tag = if ($scopeName -eq 'prompt' -or $scopeName -eq 'tool') { '只读' } else { '可改' }
        Write-Host ''
        Write-Host ("== {0}（{1}）—— {2} 条 ==" -f $scopeName, $tag, $byScope[$scopeName].Count) -ForegroundColor White
        foreach ($e in $byScope[$scopeName]) {
            $color = switch ($e.state) { 'verified' { 'Green' } 'stale' { 'Yellow' } default { 'Gray' } }
            $flag = if ($e.editable) { 'RW' } else { 'RO' }
            Write-Host ("  [{0}] {1,-9} {2}" -f $flag, $e.stateLabel, $e.key) -ForegroundColor $color
            Write-Host ("        {0}  ·  {1}  ·  sha {2}  ·  {3} 字符" -f $e.title, $e.source, $e.sha256.Substring(0, 16), $e.chars) -ForegroundColor DarkGray
        }
    }
    Write-Host ''
    Write-Host ("共 {0} 条；状态文件 {1}" -f $sel.Count, $Script:RelStatePath) -ForegroundColor DarkGray
    $src = Get-BackendCopySource
    if (-not $src.Ok) { Write-Host ("prompt / tool：没取到 —— {0}" -f $src.Error) -ForegroundColor Yellow }
}

function Invoke-ShowAction {
    param([hashtable]$State)
    if (-not $Key) { throw 'show 需要 -Key <k>（先用 list 看有哪些）' }
    $all = Get-AllCopyEntries -State $State
    $e = $all | Where-Object { $_.key -eq $Key } | Select-Object -First 1
    if (-not $e) { throw "没有这个 key：$Key（先用 list 看）" }
    if ($Json) {
        Write-Output ($e | ConvertTo-Json -Depth 8)
        return
    }
    Write-Host ''
    Write-Host ("key       : {0}" -f $e.key) -ForegroundColor White
    Write-Host ("scope     : {0}（{1}）" -f $e.scope, $(if ($e.editable) { '可改' } else { '只读' }))
    Write-Host ("title     : {0}" -f $e.title)
    Write-Host ("source    : {0}" -f $e.source)
    Write-Host ("sha256    : {0}" -f $e.sha256)
    $color = switch ($e.state) { 'verified' { 'Green' } 'stale' { 'Yellow' } default { 'Gray' } }
    $extra = ''
    if ($e.state -eq 'stale') { $extra = "（状态文件里那版是 $($e.verifiedSha.Substring(0,16))）" }
    Write-Host ("校验      : {0} {1}" -f $e.stateLabel, $extra) -ForegroundColor $color
    if ($e.verifiedAt) { Write-Host ("verifiedAt: {0}" -f $e.verifiedAt) }
    if ($e.verifiedNote) { Write-Host ("校验备注  : {0}" -f $e.verifiedNote) -ForegroundColor DarkGray }
    if ($e.note) { Write-Host ("note      : {0}" -f $e.note) -ForegroundColor DarkGray }
    Write-Host ("大小      : {0} 字符 / {1} 字节" -f $e.chars, $e.bytes)
    Write-Host ''
    Write-Host '--- 当前文本 ---------------------------------------------' -ForegroundColor DarkGray
    Write-Host $e.text
    Write-Host '----------------------------------------------------------' -ForegroundColor DarkGray
}

function Invoke-VerifyAction {
    param([hashtable]$State, [bool]$Undo)
    if (-not $Key) { throw 'verify / unverify 需要 -Key <k>' }
    $all = Get-AllCopyEntries -State $State
    $e = $all | Where-Object { $_.key -eq $Key } | Select-Object -First 1
    if (-not $e) { throw "没有这个 key：$Key" }
    if ($Undo) {
        if ($State.ContainsKey($Key)) { $State.Remove($Key) }
        Save-CopyReviewState -State $State
        Write-Host ("已取消校验：{0}" -f $Key) -ForegroundColor Yellow
    } else {
        $State[$Key] = [pscustomobject]@{
            Key        = $Key
            Sha256     = $e.sha256
            VerifiedAt = (Get-Date).ToUniversalTime().ToString('o')
            Note       = [string]$Note
        }
        Save-CopyReviewState -State $State
        Write-Host ("已标记校验：{0}  sha256={1}" -f $Key, $e.sha256.Substring(0, 16)) -ForegroundColor Green
        Write-Host ("状态写进 {0}" -f $Script:RelStatePath) -ForegroundColor DarkGray
    }
}

<# batch：对每条命中的**当前文本**做 find/replace（或正则），永远先出 diff；逐条独立。 #>
function Invoke-BatchAction {
    param([hashtable]$State, [switch]$DoWrite)
    if ([string]::IsNullOrEmpty($Find)) { throw 'batch 需要 -Find <s>' }
    if ($null -eq $Replace) { $Replace = '' }
    $all = Get-AllCopyEntries -State $State
    $sel = Select-CopyEntries -Entries $all -ScopeFilter $Scope -KeyFilter $Keys
    $editable = @($sel | Where-Object { $_.editable })

    $rx = $null
    if ($Op -eq 'regex') {
        try { $rx = [regex]::new($Find) } catch { throw "正则不合法：$($_.Exception.Message)" }
    }

    $results = @()
    $misses = 0
    foreach ($e in $editable) {
        $new = $null
        if ($Op -eq 'regex') {
            $new = $rx.Replace($e.text, $Replace)
        } else {
            $new = $e.text.Replace($Find, $Replace)
        }
        if ($new -ceq $e.text) { $misses++; continue }   # 0 处命中的条目：直接跳过（不落盘、不改字节）
        $results += Invoke-CopyChange -Entry $e -NewText $new -DoWrite:$DoWrite
    }

    if ($Json) {
        # 把 hunks 收成纯文本，JSON 里好用
        $rows = @()
        foreach ($r in $results) {
            $rows += [ordered]@{
                key       = $r.key
                scope     = $r.scope
                source    = $r.source
                oldSha256 = $r.oldSha256
                newSha256 = $r.newSha256
                written   = $r.written
                backup    = $r.backup
                error     = $r.error
                diff      = (@(Format-Hunks -Key $r.key -Source $r.source -Hunks $r.hunks) -join "`n")
            }
        }
        Write-Output ([ordered]@{
                version   = 1
                op        = $Op
                find      = $Find
                replace   = $Replace
                applied   = [bool]$DoWrite
                candidates = $editable.Count
                changed   = $results.Count
                zeroHit   = $misses
                results   = $rows
            } | ConvertTo-Json -Depth 8)
    } else {
        if ($results.Count -eq 0) {
            Write-Host ("0 处命中（检查了 {0} 条可改条目）—— 没有写任何文件。" -f $editable.Count) -ForegroundColor Yellow
        } else {
            Show-ChangeResults -Results $results -DoWrite:$DoWrite
        }
    }
}

<# apply / import 共用的写盘入口：吃 edits + batch + verify 三段。 #>
function Invoke-ApplyPayload {
    param($Payload, [string]$Origin, [switch]$DoWrite)
    if (-not $Payload) { throw "改动文件读不到或不是合法 JSON：$Origin" }
    $state = Read-CopyReviewState
    $all = Get-AllCopyEntries -State $state
    $byKey = @{}
    foreach ($e in $all) { $byKey[$e.key] = $e }

    $results = @()
    $notes = @()

    # ① edits：整段替换（浏览器"就地改文本"导出的就是这一段）
    foreach ($ed in (ConvertTo-Array $Payload.edits)) {
        if (-not $ed -or -not $ed.key) { continue }
        $k = [string]$ed.key
        if (-not $byKey.ContainsKey($k)) { $notes += "跳过 $k：这个 key 现在不存在（清单变了？）"; continue }
        $e = $byKey[$k]
        $text = [string]$ed.text
        $results += Invoke-CopyChange -Entry $e -NewText $text -DoWrite:$DoWrite
    }

    # ② batch：单条 find/replace（HTML 的批量面板通常已经折进 edits，这里给"按 key 的批量"用）
    foreach ($b in (ConvertTo-Array $Payload.batch)) {
        if (-not $b -or -not $b.key) { continue }
        $k = [string]$b.key
        if (-not $byKey.ContainsKey($k)) { $notes += "跳过 $k：这个 key 现在不存在"; continue }
        $e = $byKey[$k]
        $op = if ($b.op) { [string]$b.op } else { 'replace' }
        $find = [string]$b.find
        $rep = [string]$b.replace
        if ([string]::IsNullOrEmpty($find)) { $notes += "跳过 $k：batch.find 为空"; continue }
        $new = $null
        if ($op -eq 'regex') {
            try { $new = ([regex]::new($find)).Replace($e.text, $rep) } catch { $notes += "跳过 $k：正则不合法（$($_.Exception.Message)）"; continue }
        } else { $new = $e.text.Replace($find, $rep) }
        if ($new -ceq $e.text) { $notes += "跳过 $k：0 处命中"; continue }
        $results += Invoke-CopyChange -Entry $e -NewText $new -DoWrite:$DoWrite
    }

    # ③ verify：随改动一起回来的"我读过"
    $verifyApplied = @()
    foreach ($v in (ConvertTo-Array $Payload.verify)) {
        if (-not $v -or -not $v.key) { continue }
        $k = [string]$v.key
        if (-not $byKey.ContainsKey($k)) { $notes += "跳过校验 $k：这个 key 现在不存在"; continue }
        if ($DoWrite) {
            $state[$k] = [pscustomobject]@{
                Key        = $k
                Sha256     = $byKey[$k].sha256
                VerifiedAt = (Get-Date).ToUniversalTime().ToString('o')
                Note       = [string]$v.note
            }
        }
        $verifyApplied += $k
    }
    if ($DoWrite -and $verifyApplied.Count -gt 0) { Save-CopyReviewState -State $state }

    $changed = @($results | Where-Object { $_.changed })
    $written = @($results | Where-Object { $_.written })
    $failed = @($results | Where-Object { $_.error })

    if ($Json) {
        $rows = @()
        foreach ($r in $results) {
            $rows += [ordered]@{
                key       = $r.key
                scope     = $r.scope
                source    = $r.source
                changed   = $r.changed
                written   = $r.written
                backup    = $r.backup
                skipped   = $r.skipped
                error     = $r.error
                oldSha256 = $r.oldSha256
                newSha256 = $r.newSha256
                diff      = (@(Format-Hunks -Key $r.key -Source $r.source -Hunks $r.hunks) -join "`n")
            }
        }
        Write-Output ([ordered]@{
                version       = 1
                applied       = [bool]$DoWrite
                origin        = $Origin
                changed       = $changed.Count
                written       = $written.Count
                failed        = $failed.Count
                verifyApplied = $verifyApplied.Count
                notes         = $notes
                results       = $rows
            } | ConvertTo-Json -Depth 8)
    } else {
        if ($results.Count -eq 0 -and $verifyApplied.Count -eq 0) {
            Write-Host '这份改动里没有任何可应用的条目（0 处命中）—— 没有写任何文件。' -ForegroundColor Yellow
        } else {
            Show-ChangeResults -Results $results -DoWrite:$DoWrite
        }
        if ($verifyApplied.Count -gt 0) {
            Write-Host ("校验标记 {0} 条：{1}{2}" -f $verifyApplied.Count, ($verifyApplied -join ', '), $(if ($DoWrite) { '（已写入状态文件）' } else { '（dry-run 没写）' })) -ForegroundColor Green
        }
        foreach ($n in $notes) { Write-Host ("  · $n") -ForegroundColor DarkGray }
    }
}

<# set：等价于"只含一条 edits"的 apply（写盘路径完全同一条）。 #>
function Invoke-SetAction {
    param([hashtable]$State, [switch]$DoWrite)
    if (-not $Key) { throw 'set 需要 -Key <k>' }
    if (-not $TextFile) { throw 'set 需要 -TextFile <path>（要把这条文案换成什么，写在文件里）' }
    $p = Get-AbsPath $TextFile
    if (-not (Test-Path $p)) { throw "TextFile 不在：$TextFile" }
    $newText = ConvertTo-Lf ([System.IO.File]::ReadAllText($p, [System.Text.Encoding]::UTF8))
    $all = Get-AllCopyEntries -State $State
    $e = $all | Where-Object { $_.key -eq $Key } | Select-Object -First 1
    Assert-Editable -Entry $e
    $r = Invoke-CopyChange -Entry $e -NewText $newText -DoWrite:$DoWrite
    $results = @($r)
    if ($Json) {
        Write-Output ([ordered]@{
                version = 1; applied = [bool]$DoWrite; changed = @($results | Where-Object { $_.changed }).Count
                written = @($results | Where-Object { $_.written }).Count
                results = @($results | ForEach-Object { [ordered]@{ key = $_.key; changed = $_.changed; written = $_.written; backup = $_.backup; error = $_.error; diff = (@(Format-Hunks -Key $_.key -Source $_.source -Hunks $_.hunks) -join "`n") } })
            } | ConvertTo-Json -Depth 8)
    } else {
        Show-ChangeResults -Results $results -DoWrite:$DoWrite
    }
}

function Invoke-ExportAction {
    param([hashtable]$State)
    $all = Get-AllCopyEntries -State $State
    $sel = Select-CopyEntries -Entries $all -ScopeFilter $Scope -KeyFilter $Keys
    $src = Get-BackendCopySource
    $overlay = [ordered]@{
        version      = 1
        tool         = 'copy-review-overlay'
        root         = $Script:Root
        generatedAt  = (Get-Date).ToUniversalTime().ToString('o')
        backendOk    = [bool]$src.Ok
        backendNote  = $src.Error
        promptVersion = $src.PromptVersion
        entries      = @($sel | ForEach-Object {
                [ordered]@{
                    key        = $_.key
                    scope      = $_.scope
                    title      = $_.title
                    source     = $_.source
                    editable   = [bool]$_.editable
                    sha256     = $_.sha256
                    verifiedAt = $_.verifiedAt
                    state      = $_.state
                    note       = $_.note
                    text       = $_.text
                }
            })
    }
    $path = if ($Out) { Get-AbsPath $Out } else { Join-Path $Script:StateDir 'copy-review-export.json' }
    Write-Utf8File -Path $path -Text (($overlay | ConvertTo-Json -Depth 12) + "`n")
    Write-Host ("已导出覆盖层文本：{0}（{1} 条）" -f (ConvertTo-RelPath $path), $sel.Count) -ForegroundColor Green
    if (-not $src.Ok) { Write-Host ("⚠ prompt / tool 没取到（{0}）—— 这两类不在导出里" -f $src.Error) -ForegroundColor Yellow }
}

<# import：拿这份覆盖层跟当前文本对账（默认 dry-run；-Apply 才写）。 #>
function Invoke-ImportAction {
    param([hashtable]$State, [switch]$DoWrite)
    if (-not $In) { throw 'import 需要 -In <json>' }
    $p = Get-AbsPath $In
    $payload = Read-JsonFileOrNull -Path $p
    if (-not $payload) { throw "读不到或不是合法 JSON：$In" }
    if (-not $payload.entries) { throw "这份文件里没有 entries（不是 export 出来的？）：$In" }
    $all = Get-AllCopyEntries -State $State
    $byKey = @{}
    foreach ($e in $all) { $byKey[$e.key] = $e }
    $results = @()
    $notes = @()
    foreach ($row in (ConvertTo-Array $payload.entries)) {
        $k = [string]$row.key
        if (-not $byKey.ContainsKey($k)) { $notes += "跳过 $k：现在没有这个条目"; continue }
        $e = $byKey[$k]
        $new = ConvertTo-Lf ([string]$row.text)
        if (-not $e.editable) {
            if ($new -cne (ConvertTo-Lf $e.text)) { $notes += "只读条目 $k 与导入的文本不同 —— 只提示，不改（设计 §7）" }
            continue
        }
        if ($new -ceq (ConvertTo-Lf $e.text)) { continue }
        $results += Invoke-CopyChange -Entry $e -NewText $new -DoWrite:$DoWrite
    }
    if ($Json) {
        Write-Output ([ordered]@{
                version = 1; applied = [bool]$DoWrite; changed = @($results | Where-Object { $_.changed }).Count
                written = @($results | Where-Object { $_.written }).Count
                failed = @($results | Where-Object { $_.error }).Count
                notes = $notes
                results = @($results | ForEach-Object { [ordered]@{ key = $_.key; source = $_.source; changed = $_.changed; written = $_.written; backup = $_.backup; error = $_.error; diff = (@(Format-Hunks -Key $_.key -Source $_.source -Hunks $_.hunks) -join "`n") } })
            } | ConvertTo-Json -Depth 8)
    } else {
        if ($results.Count -eq 0) { Write-Host '导入的文本与当前源码一致 —— 没有可改的（0 处）。' -ForegroundColor Yellow }
        else { Show-ChangeResults -Results $results -DoWrite:$DoWrite }
        foreach ($n in $notes) { Write-Host ("  · $n") -ForegroundColor DarkGray }
    }
}

function Invoke-OpenAction {
    param([hashtable]$State, [string]$Out)
    $all = Get-AllCopyEntries -State $State
    $sel = Select-CopyEntries -Entries $all -ScopeFilter $Scope -KeyFilter $Keys
    $src = Get-BackendCopySource
    $payload = New-OpenPayload -Entries $sel -Src $src -State $State
    $json = ConvertTo-EmbeddedJson -Payload $payload
    $html = (Get-HtmlEditorTemplate).Replace('__PAYLOAD__', $json)
    $path = if ($Out) { Get-AbsPath $Out } else { Join-Path $Script:StateDir 'copy-review.html' }
    Write-Utf8File -Path $path -Text $html
    Write-Host ("已生成：{0}（{1} 条；{2:N0} 字节）" -f (ConvertTo-RelPath $path), $sel.Count, (Get-Item $path).Length) -ForegroundColor Green
    if (-not $src.Ok) { Write-Host ("⚠ prompt / tool 没取到：{0}" -f $src.Error) -ForegroundColor Yellow }
    Write-Host '写盘只由 apply 做：pwsh -File scripts\copy-review.ps1 apply -In .run\copy-review\copy-review-edits.json [-Apply]' -ForegroundColor DarkGray
    try {
        Start-Process $path | Out-Null
        Write-Host '已用默认浏览器打开。' -ForegroundColor DarkGray
    } catch {
        Write-Host ("（打不开浏览器：{0} —— 手动双击那个文件即可）" -f $_.Exception.Message) -ForegroundColor Yellow
    }
}

function Invoke-ReportAction {
    param([hashtable]$State, [string]$Out)
    $all = Get-AllCopyEntries -State $State
    $src = Get-BackendCopySource

    $groups = @{}
    foreach ($s in @('prompt', 'tool', 'skill', 'ui')) { $groups[$s] = @() }
    foreach ($e in $all) { $groups[$e.scope] += $e }

    $sb = New-Object System.Text.StringBuilder
    [void]$sb.AppendLine('<!doctype html>')
    [void]$sb.AppendLine('<html lang="zh-CN"><head><meta charset="utf-8">')
    [void]$sb.AppendLine('<meta name="viewport" content="width=device-width, initial-scale=1">')
    [void]$sb.AppendLine('<title>ComfyHub 文案审校报告</title><style>')
    [void]$sb.AppendLine('body{margin:0;background:#14161a;color:#e6e8ec;font:14px/1.6 "Segoe UI","Microsoft YaHei",system-ui,sans-serif}')
    [void]$sb.AppendLine('header{padding:14px 18px;border-bottom:1px solid #31363f;background:#1c1f26}')
    [void]$sb.AppendLine('h1{font-size:17px;margin:0 0 6px}h2{font-size:14px;margin:22px 18px 6px;color:#9aa3b2;border-bottom:1px solid #31363f;padding-bottom:4px}')
    [void]$sb.AppendLine('main{padding:0 0 40px}.m{color:#9aa3b2;font-size:12px}')
    [void]$sb.AppendLine('table{border-collapse:collapse;width:calc(100% - 36px);margin:8px 18px}')
    [void]$sb.AppendLine('th,td{border:1px solid #31363f;padding:6px 8px;text-align:left;vertical-align:top;font-size:13px}')
    [void]$sb.AppendLine('th{background:#1c1f26;color:#9aa3b2;font-weight:600}')
    [void]$sb.AppendLine('code{font-family:ui-monospace,Consolas,monospace;font-size:12px}')
    [void]$sb.AppendLine('.b{font-size:11px;border-radius:10px;padding:1px 8px;border:1px solid #31363f;white-space:nowrap}')
    [void]$sb.AppendLine('.verified{color:#3fb950;border-color:#3fb950}.stale{color:#d29922;border-color:#d29922}.unverified{color:#8b949e}')
    [void]$sb.AppendLine('.ro{color:#58a6ff;border-color:#58a6ff}.rw{color:#9aa3b2}')
    [void]$sb.AppendLine('.ro-wrap{background:#171a20}.note{color:#9aa3b2;font-size:12px;white-space:pre-wrap}')
    [void]$sb.AppendLine('pre{background:#22262f;border:1px solid #31363f;border-radius:6px;padding:8px;max-height:260px;overflow:auto;white-space:pre-wrap;word-break:break-word;font:12.5px/1.55 ui-monospace,Consolas,monospace;margin:0}')
    [void]$sb.AppendLine('</style></head><body>')
    [void]$sb.AppendLine('<header>')
    [void]$sb.AppendLine('<h1>ComfyHub 文案审校报告</h1>')
    [void]$sb.AppendLine(("<div class='m'>root=<code>{0}</code> · 生成于 {1} · 状态文件 <code>{2}</code></div>" -f (Get-HtmlEscaped $Script:Root), (Get-Date).ToUniversalTime().ToString('u'), (Get-HtmlEscaped $Script:RelStatePath)))
    $backendNote = if ($src.Ok) { "已取到（promptVersion=$($src.PromptVersion)，NSFW 注入=$($src.NsfwOn)）" } else { "**没取到**：$($src.Error)" }
    [void]$sb.AppendLine(("<div class='m'>后端：{0}</div>" -f (Get-HtmlEscaped $backendNote)))
    [void]$sb.AppendLine(("<div class='m'>可改 = skill 正文 / 界面注解；只读 = 系统提示词 / 工具描述。<b>只读区没有任何写入口</b>；写盘只由 <code>scripts\copy-review.ps1 apply</code> 执行（默认 dry-run）。</div>"))
    [void]$sb.AppendLine('</header><main>')

    $labels = [ordered]@{ prompt = '系统提示词（只读）'; tool = '工具描述（只读）'; skill = 'skill 正文（可改）'; ui = '界面注解（可改）' }
    foreach ($scopeName in @('prompt', 'tool', 'skill', 'ui')) {
        $list = @($groups[$scopeName])
        $ro = ($scopeName -eq 'prompt' -or $scopeName -eq 'tool')
        $tag = if ($ro) { '只读' } else { '可改' }
        [void]$sb.AppendLine(("<h2>{0} —— {1} 条（{2}）</h2>" -f (Get-HtmlEscaped $labels[$scopeName]), $list.Count, $tag))
        if ($list.Count -eq 0) {
            $why = if ($ro) { '没取到后端（本工具**不靠正则猜 Rust 源码**，所以这一档就如实空着）' } else { '没有条目' }
            [void]$sb.AppendLine(("<p class='m' style='margin:6px 18px'>{0}</p>" -f (Get-HtmlEscaped $why)))
            continue
        }
        [void]$sb.AppendLine('<table><tr><th>key</th><th>标题</th><th>来源（文件:行）</th><th>可改</th><th>校验</th><th>大小</th><th>备注</th></tr>')
        foreach ($e in $list) {
            $cls = $e.state
            $rowCls = if ($ro) { " class='ro-wrap'" } else { '' }
            [void]$sb.AppendLine("<tr$rowCls>")
            [void]$sb.AppendLine(("<td><code>{0}</code></td>" -f (Get-HtmlEscaped $e.key)))
            [void]$sb.AppendLine(("<td>{0}</td>" -f (Get-HtmlEscaped $e.title)))
            [void]$sb.AppendLine(("<td><code>{0}</code></td>" -f (Get-HtmlEscaped $e.source)))
            [void]$sb.AppendLine(("<td><span class='b {0}'>{1}</span></td>" -f $(if ($e.editable) { 'rw' } else { 'ro' }), $(if ($e.editable) { '可改' } else { '只读' })))
            [void]$sb.AppendLine(("<td><span class='b {0}'>{1}</span>{2}</td>" -f $cls, (Get-HtmlEscaped $e.stateLabel), $(if ($e.verifiedAt) { "<div class='m'>" + (Get-HtmlEscaped $e.verifiedAt) + '</div>' } else { '' })))
            [void]$sb.AppendLine(("<td>{0} 字符<br>{1} 字节</td>" -f $e.chars, $e.bytes))
            [void]$sb.AppendLine(("<td class='note'>{0}<div class='m'>sha256 {1}</div></td>" -f (Get-HtmlEscaped $e.note), $e.sha256.Substring(0, 16)))
            [void]$sb.AppendLine('</tr>')
            [void]$sb.AppendLine(("<tr$rowCls><td colspan='7'><pre>{0}</pre></td></tr>" -f (Get-HtmlEscaped $e.text)))
        }
        [void]$sb.AppendLine('</table>')
    }
    [void]$sb.AppendLine('</main></body></html>')

    $path = if ($Out) { Get-AbsPath $Out } else { Join-Path $Script:StateDir 'report.html' }
    Write-Utf8File -Path $path -Text ($sb.ToString())
    $counts = @{}
    foreach ($s in @('prompt', 'tool', 'skill', 'ui')) { $counts[$s] = @($groups[$s]).Count }
    Write-Host ("已生成报告：{0}（{1:N0} 字节）" -f (ConvertTo-RelPath $path), (Get-Item $path).Length) -ForegroundColor Green
    Write-Host ("  prompt={0} tool={1} skill={2} ui={3}" -f $counts['prompt'], $counts['tool'], $counts['skill'], $counts['ui']) -ForegroundColor DarkGray
    if (-not $src.Ok) { Write-Host ("  ⚠ prompt / tool 没取到：{0}" -f $src.Error) -ForegroundColor Yellow }
}

function Get-HtmlEscaped {
    param([string]$Text)
    if ($null -eq $Text) { return '' }
    return ($Text -replace '&', '&amp;' -replace '<', '&lt;' -replace '>', '&gt;' -replace '"', '&quot;')
}

<# check：机器校验（可改条目的 frontmatter / 长度预算）+ 只读条目的一致性提示（只提示，不改）。 #>
function Invoke-CheckAction {
    param([hashtable]$State)
    $all = Get-AllCopyEntries -State $State
    $problems = @()
    $warnings = @()
    $info = @()

    # ① 可改条目：frontmatter 完整性 / 长度预算
    foreach ($e in ($all | Where-Object { $_.scope -eq 'skill' -and $_.kind -eq 'body' })) {
        if ($e.bytes -gt $Script:SkillBodyBudgetBytes) {
            $warnings += [ordered]@{ key = $e.key; level = 'warn'; check = 'length-budget'; message = "正文 $($e.bytes) 字节，超过预算 $($Script:SkillBodyBudgetBytes) 字节（进系统提示前的目录摘要与按需加载都会更贵）" }
        }
        $fm = $all | Where-Object { $_.key -eq ($e.key -replace '\.body$', '.frontmatter') } | Select-Object -First 1
        if (-not $fm) { continue }
        if (-not $fm.text) {
            $info += [ordered]@{ key = $e.key; level = 'info'; check = 'frontmatter'; message = '这个 SKILL.md 没有 frontmatter（可改的正文本工具不负责补；后端 autoRegister 会补 name/description）' }
        } else {
            $parsed = Parse-SkillFrontmatter -Text $fm.text
            if ($parsed.Error) { $warnings += [ordered]@{ key = $e.key; level = 'warn'; check = 'frontmatter'; message = "frontmatter 有问题：$($parsed.Error)" } }
            else { $info += [ordered]@{ key = $e.key; level = 'ok'; check = 'frontmatter'; message = "frontmatter 可读：name=$($parsed.Name)，description $($parsed.DescriptionLength) 字符" } }
        }
    }

    # ② ui 条目：
    #    · 锚点/文件对不上 → **problem**（清单要跟着源码维护）
    #    · 文本含 Dart 插值（`$var`）→ 只是"工具不在这求值"，不是问题，属于 info
    #      （那种条目的可改性本来就由设计排除；把它报成 problem 会天天红一片）
    foreach ($e in ($all | Where-Object { $_.scope -eq 'ui' })) {
        if ($e.editable) { continue }
        $file = $e.PSObject.Properties['file'] ? [string]$e.file : ''
        $anchor = $e.PSObject.Properties['anchor'] ? [string]$e.anchor : ''
        if ($e.note -match '插值') {
            $info += [ordered]@{ key = $e.key; level = 'info'; check = 'ui-interpolated'; message = "含 Dart 插值，只读展示（工具不在这求值）；file=$file anchor=$anchor" }
            continue
        }
        $problems += [ordered]@{ key = $e.key; level = 'problem'; check = 'ui-anchor'; message = "$($e.note)（file=$file anchor=$anchor）" }
    }

    # ③ 只读条目的一致性提示：文案里点名的工具必须真在真源里
    #    真源 = 后端实现了的**全部**工具（`/api/ai/tools`，桌面全量目录），
    #    不是 run-kit 里那份"这次会话给了模型几个"的子集 —— 拿子集去对账会满屏假警报。
    $src = Get-BackendCopySource
    if (-not $src.Ok) {
        $warnings += [ordered]@{ key = '-'; level = 'warn'; check = 'backend'; message = "后端没取到（$($src.Error)）—— prompt / tool 两类这次为空，一致性提示无从做起" }
    } else {
        $known = @{}
        foreach ($t in $src.Tools) { $known[$t.Name] = $true }
        $allTools = Invoke-BackendGet -Path '/api/ai/tools'
        $fullCount = 0
        if ($allTools) {
            foreach ($t in (ConvertTo-Array $allTools)) {
                if ($t -and $t.name) { $known[[string]$t.name] = $true; $fullCount++ }
            }
        }
        # 只把"看着就像工具名"的当候选（`comfy_*` / `web_*` / `read_*` …）。
        # 提示词里的 snake_case 大多是**别的东西** —— 工作流文件名（`comfyhub_inpaint_anima`）、
        # 节点输入名（`lora_name`）、模型字段名（`overall_soundscape`）。全都报出来是噪音。
        $mentionRx = [regex]::new('(?<![\w.])((?:comfy|web|read|write|list|search|find|get|set|save|load|delete|update|install|launch|stop|free|node|which|discover|fetch|emit|register|partner|project|server|system|validate|vary|upload|download|remember|ask)_[a-z][a-z0-9_]{0,40})(?![\w])')
        foreach ($e in ($all | Where-Object { $_.scope -eq 'prompt' -or $_.scope -eq 'tool' })) {
            $hits = @{}
            foreach ($m in $mentionRx.Matches($e.text)) {
                $toolName = $m.Groups[1].Value
                if (-not $known.ContainsKey($toolName)) { $hits[$toolName] = $true }
            }
            foreach ($toolName in ($hits.Keys | Sort-Object)) {
                $warnings += [ordered]@{ key = $e.key; level = 'warn'; check = 'tool-mention'; message = "文案里点名了「$toolName」，但真源（/api/ai/tools）的清单里没有它（要么名字写错了，要么这个工具没注册）—— 只提示，不改" }
            }
        }
        $info += [ordered]@{ key = '-'; level = 'ok'; check = 'backend'; message = "后端在跑：promptVersion=$($src.PromptVersion)，run-kit 给了 $($src.Tools.Count) 个工具，/api/ai/tools 报了 $fullCount 个（对账用的是后者）" }
    }

    # ④ 已校验但正文/文件对不上的（stale）也列出来
    foreach ($e in $all) {
        if ($e.state -eq 'stale') { $warnings += [ordered]@{ key = $e.key; level = 'warn'; check = 'stale'; message = "文本已经变了（状态里那版是 $($e.verifiedSha.Substring(0,16))，现在是 $($e.sha256.Substring(0,16))）→ 要重看" } }
    }

    if ($Json) {
        Write-Output ([ordered]@{
                version    = 1
                root       = $Script:Root
                entries    = @($all).Count
                problems   = $problems.Count
                warnings   = $warnings.Count
                info       = $info.Count
                backendOk  = [bool]$src.Ok
                issues     = @($problems + $warnings)
                notes      = @($info)
            } | ConvertTo-Json -Depth 8)
        return
    }

    Write-Host ''
    Write-Host ("== check：{0} 条文案 ==" -f @($all).Count) -ForegroundColor White
    if ($problems.Count -eq 0) { Write-Host '问题（problem）：无' -ForegroundColor Green } else {
        Write-Host ("问题（problem）：{0}" -f $problems.Count) -ForegroundColor Red
        foreach ($p in $problems) { Write-Host ("  · [{0}] {1} — {2}" -f $p.check, $p.key, $p.message) -ForegroundColor Red }
    }
    if ($warnings.Count -eq 0) { Write-Host '提示（warn）：无' -ForegroundColor Green } else {
        Write-Host ("提示（warn）：{0}" -f $warnings.Count) -ForegroundColor Yellow
        foreach ($w in $warnings) { Write-Host ("  · [{0}] {1} — {2}" -f $w.check, $w.key, $w.message) -ForegroundColor Yellow }
    }
    Write-Host ("通过（info）：{0}" -f $info.Count) -ForegroundColor DarkGray
    Write-Host ''
    Write-Host '（check 只提示、从不改：写盘只有 apply / batch -Apply / set -Apply 三条路）' -ForegroundColor DarkGray
}

# ─────────────────────────────────────────────────────────────────────────────
# 12. -SelfTest：临时目录造假的 skill + ui 清单，跑一遍全流程
# ─────────────────────────────────────────────────────────────────────────────

function New-SelfTestFixture {
    param([string]$Root)
    $skillDir = Join-Path $Root 'skills\builtin\demo-skill'
    New-Item -ItemType Directory -Force -Path $skillDir | Out-Null
    $skill = @"
---
name: demo-skill
description: 一个用来跑自测的假 skill。
---

# Demo Skill

第一条纪律：**自测用的文本**。
第二条纪律：dry-run 不许落盘。
"@
    Write-Utf8File -Path (Join-Path $skillDir 'SKILL.md') -Text ($skill -replace "`n", "`r`n")

    $userDir = Join-Path $Root 'storage\ai\skills\user-skill'
    New-Item -ItemType Directory -Force -Path $userDir | Out-Null
    Write-Utf8File -Path (Join-Path $userDir 'SKILL.md') -Text "# 没有 frontmatter 的 skill`n正文一行。`n"

    $dartDir = Join-Path $Root 'lib\pages'
    New-Item -ItemType Directory -Force -Path $dartDir | Out-Null
    $dart = @'
import 'package:flutter/material.dart';

class Demo extends StatelessWidget {
  @override
  Widget build(BuildContext context) {
    return Column(children: [
      // 这是注释，锚点不该落到这里
      const Text('密钥只写不读，不会出现在界面、数据库普通字段和日志里'),
      const Text(
        '留空 = 自动（按端口自动挑）'
        '，只有要连别的机器时才需要手填。',
      ),
      const Text('只读演示：' '相邻字面量会拼起来'),
      const Text('留空 = 自动'),
    ]);
  }
}
'@
    Write-Utf8File -Path (Join-Path $dartDir 'demo_page.dart') -Text ($dart -replace "`n", "`r`n")

    $manifest = [ordered]@{
        version = 1
        note    = '自测用'
        entries = @(
            [ordered]@{ key = 'ui.demo.secret_subtitle'; file = 'lib/pages/demo_page.dart'; anchor = '密钥只写不读'; title = '演示：密钥只写不读'; note = '自测' },
            [ordered]@{ key = 'ui.demo.port_helper'; file = 'lib/pages/demo_page.dart'; anchor = '，只有要连别的机器时才需要手填'; title = '演示：跨行相邻字面量拼接'; note = '自测' },
            [ordered]@{ key = 'ui.demo.concat'; file = 'lib/pages/demo_page.dart'; anchor = '只读演示'; title = '演示：同行相邻字面量拼接'; note = '自测' },
            [ordered]@{ key = 'ui.demo.outside_string'; file = 'lib/pages/demo_page.dart'; anchor = '锚点不该落到这里'; title = '演示：锚点落在注释里（不是字符串）'; note = '自测' },
            [ordered]@{ key = 'ui.demo.missing'; file = 'lib/pages/demo_page.dart'; anchor = '这个锚点故意不存在'; title = '演示：锚点失配'; note = '自测' }
        )
    }
    Write-Utf8File -Path (Join-Path $Root 'scripts\copy-review-ui.json') -Text (($manifest | ConvertTo-Json -Depth 6) + "`n")
}

function Invoke-SelfTest {
    $results = New-Object System.Collections.ArrayList
    function Add-Result {
        param([string]$Name, [bool]$Ok, [string]$Detail)
        [void]$results.Add([pscustomobject]@{ Name = $Name; Ok = $Ok; Detail = $Detail })
    }

    # 关键：整个自测跑在一个临时根里，**绝不碰真项目**
    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("copy-review-selftest-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
    New-Item -ItemType Directory -Force -Path $tmp | Out-Null
    $oldRoot = $Script:Root
    $oldStateDir = $Script:StateDir
    $oldStatePath = $Script:StatePath
    $oldUi = $Script:UiManifestPath
    try {
        New-SelfTestFixture -Root $tmp
        $Script:Root = $tmp
        $Script:StateDir = Join-Path $tmp '.run\copy-review'
        $Script:StatePath = Join-Path $tmp '.run\copy-review\state.json'
        $Script:UiManifestPath = Join-Path $tmp 'scripts\copy-review-ui.json'
        $Script:BackendProbe = [pscustomobject]@{ Port = 0; Source = '自测（不探后端）' }

        $state = Read-CopyReviewState
        $all = Get-AllCopyEntries -State $state

        # ① 枚举
        $ck1 = @($all | Where-Object { $_.key -eq 'skill.demo-skill.body' }).Count -eq 1
        $ck2 = @($all | Where-Object { $_.key -eq 'skill.demo-skill.frontmatter' -and -not $_.editable }).Count -eq 1
        $ck3 = @($all | Where-Object { $_.key -eq 'skill.user-skill.body' }).Count -eq 1
        # 已知边界：跨行相邻拼接的第二行（见文件头）在这份夹具里不算可改 → 至少 3 条可改
        $ck4 = @($all | Where-Object { $_.scope -eq 'ui' -and $_.editable }).Count -ge 3
        $ck5 = @($all | Where-Object { $_.key -eq 'ui.demo.missing' -and -not $_.editable }).Count -eq 1
        $ck6 = @($all | Where-Object { $_.key -eq 'ui.demo.concat' -and $_.text -eq '只读演示：相邻字面量会拼起来' }).Count -eq 1
        $ck6b = @($all | Where-Object { $_.key -eq 'ui.demo.port_helper' -and $_.text -eq '，只有要连别的机器时才需要手填。' }).Count -eq 1
        # 锚点落在注释里（不是字符串）→ 必须**拒答**（不许猜一条出来），并标成只读 + 原因
        $ck6c = @($all | Where-Object { $_.key -eq 'ui.demo.outside_string' -and -not $_.editable -and $_.note -match '字符串外面' }).Count -eq 1
        Add-Result -Name '枚举：两个 skill 投放口的正文+frontmatter' -Ok ($ck1 -and $ck2 -and $ck3) -Detail "body=$ck1 frontmatterRO=$ck2 user=$ck3"
        Add-Result -Name '枚举：ui 清单现读源码（3+ 可改 + 2 定位不了只读）' -Ok ($ck4 -and $ck5 -and $ck6c) -Detail "editableUi=$ck4 锚点失配=$ck5 锚点在串外=$ck6c"
        Add-Result -Name '解析：相邻 Dart 字面量隐式拼接（同一行内）' -Ok $ck6 -Detail "同行拼接=$ck6"
        # 已知边界（见文件头）：锚点落在**跨行**相邻拼接的第二行时只给后半句 —— 但**不许给错**，
        # 只要它给出来的东西真的在源码里、而且真含锚点，就算合格。
        $pk = '，只有要连别的机器时才需要手填。'
        $ck6e = ($pk.IndexOf('，只有要连别的机器时才需要手填') -ge 0)
        Add-Result -Name '解析：跨行拼接至少不许给出与锚点无关的文本' -Ok ($ck6b -and $ck6e) -Detail "跨行（已知边界，只给后半句）=$ck6b"

        # ② 后端不在线 → prompt/tool 如实为空
        $ck7 = @($all | Where-Object { $_.scope -eq 'prompt' -or $_.scope -eq 'tool' }).Count -eq 0
        $src = Get-BackendCopySource
        Add-Result -Name '没取到后端：prompt/tool 如实为空（不猜 key）' -Ok ($ck7 -and -not $src.Ok) -Detail $src.Error

        # ③ verify / unverify
        $bodyKey = 'skill.demo-skill.body'
        $bodyEntry = $all | Where-Object { $_.key -eq $bodyKey } | Select-Object -First 1
        $state[$bodyKey] = [pscustomobject]@{ Key = $bodyKey; Sha256 = $bodyEntry.sha256; VerifiedAt = (Get-Date).ToUniversalTime().ToString('o'); Note = '自测读过' }
        Save-CopyReviewState -State $state
        $st = Read-CopyReviewState
        $ck8 = $st.ContainsKey($bodyKey) -and $st[$bodyKey].Sha256 -eq $bodyEntry.sha256
        Add-Result -Name 'verify：状态写进 state.json（带当版 sha256）' -Ok $ck8 -Detail "sha=$(if($st.ContainsKey($bodyKey)){$st[$bodyKey].Sha256.Substring(0,16)}else{'-'})"

        $state2 = Read-CopyReviewState
        $all2 = Get-AllCopyEntries -State $state2
        $newBody = ($all2 | Where-Object { $_.key -eq $bodyKey }).text -replace '自测用的文本', '自测用的文本（改过）'
        $file = Join-Path $tmp 'skills\builtin\demo-skill\SKILL.md'
        $beforeText = [System.IO.File]::ReadAllText($file, [System.Text.Encoding]::UTF8)
        $beforeHash = Get-Sha256Short -Text $beforeText

        # ④ set/apply dry-run：不落盘、不算 written
        $newFile = Join-Path $tmp '.run\new-body.txt'
        Write-Utf8File -Path $newFile -Text $newBody
        $dry = Invoke-CopyChange -Entry ($all2 | Where-Object { $_.key -eq $bodyKey }) -NewText $newBody
        $afterDry = [System.IO.File]::ReadAllText($file, [System.Text.Encoding]::UTF8)
        Add-Result -Name 'apply dry-run：算出 diff 但不落盘' -Ok ($dry.changed -and -not $dry.written -and ((Get-Sha256Short -Text $afterDry) -eq $beforeHash)) -Detail "changed=$($dry.changed) written=$($dry.written) 文件未变=$((Get-Sha256Short -Text $afterDry) -eq $beforeHash)"

        # ⑤ 只读条目：写入口被拦住
        $roBlocked = $false
        try { Assert-Editable -Entry ($all2 | Where-Object { $_.key -eq 'skill.demo-skill.frontmatter' }) } catch { $roBlocked = $true }
        $roChanged = Invoke-CopyChange -Entry ($all2 | Where-Object { $_.key -eq 'skill.demo-skill.frontmatter' }) -NewText 'x'
        Add-Result -Name '只读条目：没有写入口（Assert 拦住 + 写盘被拒）' -Ok ($roBlocked -and -not $roChanged.written) -Detail "assert拦=$roBlocked 写盘=$($roChanged.written) 原因=$($roChanged.skipped)"

        # ⑥ -Apply：真的落盘 + 留备份
        $applied = Invoke-CopyChange -Entry ($all2 | Where-Object { $_.key -eq $bodyKey }) -NewText $newBody -DoWrite
        $bakExists = Test-Path "$file.copyreview-bak"
        $bakSame = if ($bakExists) { (Get-Sha256Short -Text ([System.IO.File]::ReadAllText("$file.copyreview-bak", [System.Text.Encoding]::UTF8))) -eq $beforeHash } else { $false }
        $afterApply = [System.IO.File]::ReadAllText($file, [System.Text.Encoding]::UTF8)
        $contentOk = $afterApply.Contains('自测用的文本（改过）') -and $afterApply.Contains('name: demo-skill') -and $afterApply.Contains("`r`n")
        Add-Result -Name 'apply -Apply：落盘 + 先备份 <file>.copyreview-bak（frontmatter 未被动）' -Ok ($applied.written -and $bakExists -and $bakSame -and $contentOk) -Detail "written=$($applied.written) bak=$bakExists 备份内容对=$bakSame 保CRLF+frontmatter=$contentOk"

        # ⑦ 文本变了 → 校验失效（stale）
        $state3 = Read-CopyReviewState
        $all3 = Get-AllCopyEntries -State $state3
        $stale = ($all3 | Where-Object { $_.key -eq $bodyKey }).state -eq 'stale'
        Add-Result -Name '文本变了：绑定 sha256 的"已校验"自动失效（stale）' -Ok $stale -Detail "state=$(($all3 | Where-Object { $_.key -eq $bodyKey }).state)"

        # ⑧ ui 写回（同一份源码里就地换字面量）
        $uiKey = 'ui.demo.secret_subtitle'
        $uiEntry = $all3 | Where-Object { $_.key -eq $uiKey } | Select-Object -First 1
        $uiApplied = Invoke-CopyChange -Entry $uiEntry -NewText '密钥只写不读（已复核），不会出现在界面、数据库普通字段和日志里' -DoWrite
        $all4 = Get-AllCopyEntries -State (Read-CopyReviewState)
        $uiOk = ($all4 | Where-Object { $_.key -eq $uiKey }).text -eq '密钥只写不读（已复核），不会出现在界面、数据库普通字段和日志里'
        $commentOk = [System.IO.File]::ReadAllText((Join-Path $tmp 'lib\pages\demo_page.dart'), [System.Text.Encoding]::UTF8).Contains('// 这是注释，锚点不该落到这里')
        Add-Result -Name 'ui 写回：源码字面量就地替换，可重新读回、注释未被动' -Ok ($uiApplied.written -and $uiOk -and $commentOk) -Detail "written=$($uiApplied.written) 读回一致=$uiOk 注释完好=$commentOk"

        # ⑨ batch 命中 0 处：不写任何文件
        $snapshot = @{}
        foreach ($f in (Get-ChildItem -Path $tmp -Recurse -File | Where-Object { $_.FullName -notlike '*\.run\*' })) { $snapshot[$f.FullName] = Get-Sha256Short -Text ([System.IO.File]::ReadAllText($f.FullName, [System.Text.Encoding]::UTF8)) }
        $batchTargets = @($all4 | Where-Object { $_.editable })
        $miss = 0
        foreach ($b in $batchTargets) { if (($b.text.Replace('一个故意不存在的串', 'x')) -ceq $b.text) { $miss++ } }
        $noFileTouched = $true
        foreach ($k in $snapshot.Keys) { if ((Get-Sha256Short -Text ([System.IO.File]::ReadAllText($k, [System.Text.Encoding]::UTF8))) -ne $snapshot[$k]) { $noFileTouched = $false } }
        Add-Result -Name 'batch：0 处命中 → 一条都不改（逐文件哈希未变）' -Ok ($miss -eq $batchTargets.Count -and $noFileTouched) -Detail "checked=$($batchTargets.Count) zeroHit=$miss 无文件变化=$noFileTouched"

        # ⑩ batch 命中：真的改 + 备份
        $res = @()
        foreach ($b in $batchTargets) {
            $new = $b.text.Replace('自测用的文本（改过）', '自测用的文本（批改过）')
            if ($new -ceq $b.text) { continue }
            $res += Invoke-CopyChange -Entry $b -NewText $new -DoWrite
        }
        $batchOk = ($res.Count -ge 1) -and (@($res | Where-Object { $_.written }).Count -eq $res.Count)
        Add-Result -Name 'batch -Apply：命中条目落盘成功（逐条独立）' -Ok $batchOk -Detail "命中=$(($res | Where-Object { $_.changed }).Count) 写入=$(($res | Where-Object { $_.written }).Count)"

        # ⑪ check：坏 frontmatter 能被拦下
        $badDir = Join-Path $tmp 'skills\builtin\bad-skill'
        New-Item -ItemType Directory -Force -Path $badDir | Out-Null
        Write-Utf8File -Path (Join-Path $badDir 'SKILL.md') -Text "---`ndescription: 没有 name 的坏 frontmatter`n---`n`n正文。`n"
        $all5 = Get-AllCopyEntries -State (Read-CopyReviewState)
        $fmBad = $all5 | Where-Object { $_.key -eq 'skill.bad-skill.frontmatter' } | Select-Object -First 1
        $parsedBad = Parse-SkillFrontmatter -Text $fmBad.text
        Add-Result -Name 'check：坏 frontmatter（缺 name）能被解析器拦下' -Ok ([bool]$parsedBad.Error) -Detail "error=$($parsedBad.Error)"

        $checkJson = $null
        $sw = [System.IO.StringWriter]::new()
        $oldOut = [Console]::Out
        try {
            [Console]::SetOut($sw)
            Invoke-CheckAction -State (Read-CopyReviewState) -Json | Out-Null
        } finally { [Console]::SetOut($oldOut) }
        $checkJson = $sw.ToString()
        Add-Result -Name 'check -Json：ui 锚点失配被列为 problem；后端没取到被列为 warn' -Ok ($checkJson.Contains('ui.demo.missing') -and $checkJson.Contains('backend')) -Detail '含 ui-anchor problem + backend warn'

        # ⑫ 导出 → 导入（dry-run 不落盘）
        $expPath = Join-Path $tmp '.run\overlay.json'
        $overlay = [ordered]@{
            version = 1
            entries = @($all5 | Where-Object { $_.editable } | ForEach-Object { [ordered]@{ key = $_.key; text = $_.text } })
        }
        Write-Utf8File -Path $expPath -Text ($overlay | ConvertTo-Json -Depth 8)
        $beforeImport = @{}
        foreach ($f in (Get-ChildItem -Path $tmp -Recurse -File | Where-Object { $_.FullName -notlike '*\.run\*' -and $_.Extension -ne '.copyreview-bak' })) {
            $beforeImport[$f.FullName] = Get-Sha256Short -Text ([System.IO.File]::ReadAllText($f.FullName, [System.Text.Encoding]::UTF8))
        }
        $impDry = Invoke-ApplyPayload -Payload (Read-JsonFileOrNull -Path $expPath) -Origin 'selftest' -DoWrite:$false
        $impSame = $true
        foreach ($k in $beforeImport.Keys) { if ((Get-Sha256Short -Text ([System.IO.File]::ReadAllText($k, [System.Text.Encoding]::UTF8))) -ne $beforeImport[$k]) { $impSame = $false } }
        Add-Result -Name 'import/apply：与当前一致时 0 处命中，dry-run 不落盘' -Ok $impSame -Detail "文件未变=$impSame"

        # ⑬ report / open 的 HTML 能生成且自包含
        # ⚠ 自测跑在临时根里，而 `Get-AbsPath` 是按真实的 `$Script:Root` 拼的 ——
        #    所以要**先换成绝对路径**再传进去，否则报告会写到真项目里（实测踩到）。
        $repPath = Join-Path $tmp '.run\report.html'
        Invoke-ReportAction -State (Read-CopyReviewState) -Out $repPath
        $repText = [System.IO.File]::ReadAllText($repPath, [System.Text.Encoding]::UTF8)
        $repOk = $repText.Contains('<html') -and $repText.Contains('系统提示词（只读）') -and $repText.Contains('没取到后端')
        Add-Result -Name 'report：生成单文件 HTML（含只读区 + "没取到后端"的如实说明）' -Ok $repOk -Detail "字节=$((Get-Item $repPath).Length)"

        $openPath = Join-Path $tmp '.run\copy-review.html'
        $all6 = Get-AllCopyEntries -State (Read-CopyReviewState)
        $payload = New-OpenPayload -Entries $all6 -Src (Get-BackendCopySource) -State (Read-CopyReviewState)
        $html = (Get-HtmlEditorTemplate).Replace('__PAYLOAD__', (ConvertTo-EmbeddedJson -Payload $payload))
        Write-Utf8File -Path $openPath -Text $html
        $openOk = $html.Contains('application/json') -and $html.Contains('copy-review-data') -and (-not ($html -match '<script[^>]+src=')) -and (-not ($html -match '<link[^>]+href='))
        Add-Result -Name 'open：自包含 HTML（内嵌清单、无外链、无服务器）' -Ok $openOk -Detail "字节=$((Get-Item $openPath).Length)"

        # ⑭ 只读的可改性：prompt/tool 条目在真源里没有写路径
        $promptEntry = [pscustomobject]@{ key = 'prompt.s01'; editable = $false; editableReason = '系统提示词只读'; scope = 'prompt' }
        $blocked = $false
        try { Assert-Editable -Entry $promptEntry } catch { $blocked = $true }
        Add-Result -Name '只读：Assert-Editable 对 prompt/tool 一律拦住' -Ok $blocked -Detail "拦住=$blocked"
    } catch {
        Add-Result -Name '自测自身异常' -Ok $false -Detail ($_.Exception.Message + ' @ ' + $_.ScriptStackTrace)
    } finally {
        $Script:Root = $oldRoot
        $Script:StateDir = $oldStateDir
        $Script:StatePath = $oldStatePath
        $Script:UiManifestPath = $oldUi
        $Script:BackendProbe = $null
        if (-not $KeepSelfTestTemp) { Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue } else { Write-Host ("（-KeepSelfTestTemp：临时目录留在 $tmp）") -ForegroundColor Yellow }
    }

    $pass = @($results | Where-Object { $_.Ok }).Count
    $fail = @($results | Where-Object { -not $_.Ok }).Count
    # ⚠ 自测的输出必须走 [Console]::Out：`Write-Host` 在 `pwsh -File` 下会被吞掉，
    #   自测就成了"跑完什么都没有"（实测踩到）。
    [Console]::Out.WriteLine('')
    [Console]::Out.WriteLine('== copy-review -SelfTest ==')
    foreach ($r in $results) {
        $mark = if ($r.Ok) { 'PASS' } else { 'FAIL' }
        [Console]::Out.WriteLine(("  [{0}] {1}" -f $mark, $r.Name))
        [Console]::Out.WriteLine(("         {0}" -f $r.Detail))
    }
    [Console]::Out.WriteLine('')
    [Console]::Out.WriteLine(("PASS {0} / FAIL {1}（共 {2} 项）" -f $pass, $fail, $results.Count))
    if ($Json) {
        Write-Output ([ordered]@{ version = 1; pass = $pass; fail = $fail; total = $results.Count; results = $results } | ConvertTo-Json -Depth 6)
    }
    return $fail
}

# ─────────────────────────────────────────────────────────────────────────────
# 13. 分发（动作只是薄薄一层，引擎都在上面）
# ─────────────────────────────────────────────────────────────────────────────

if ($SelfTest -or $Action -eq 'selftest') {
    $failed = Invoke-SelfTest
    exit ([int]$failed)
}

# 这几个动作**不许写任何文件**（除 report/open 自己那份产物）：list/show/check 明确只读
$state = Read-CopyReviewState

try {
    switch ($Action) {
        'list' { Invoke-ListAction -State $state }
        'show' { Invoke-ShowAction -State $state }
        'verify' { Invoke-VerifyAction -State $state -Undo $false }
        'unverify' { Invoke-VerifyAction -State $state -Undo $true }
        'set' { Invoke-SetAction -State $state -DoWrite:$Apply }
        'batch' { Invoke-BatchAction -State $state -DoWrite:$Apply }
        'export' { Invoke-ExportAction -State $state }
        'import' { Invoke-ImportAction -State $state -DoWrite:$Apply }
        'report' { Invoke-ReportAction -State $state }
        'open' { Invoke-OpenAction -State $state }
        'check' { Invoke-CheckAction -State $state }
    }
} catch {
    Write-Host ("错误：{0}" -f $_.Exception.Message) -ForegroundColor Red
    exit 2
}