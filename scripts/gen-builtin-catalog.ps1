<#
.SYNOPSIS
  从本机 `.dsh\settings.yaml` 生成项目内置的模型目录（冻结副本）。

.DESCRIPTION
  背景（用户报的 bug）：本机的 `%USERPROFILE%\.dsh\settings.yaml` 里登记了 69 个模型，
  但项目运行时**不能**去读那份 YAML —— 装了本项目却没装 DSH 的机器上根本没有这个文件。
  所以这里把 YAML 里的模型信息**抄**成一份带版本的 JSON，进版本库、进 jar，
  运行期只读这份冻结副本（`server/src/main/resources/ai/builtin-catalog.json`）。

  这个脚本是**开发期**工具：settings.yaml 变了就重跑一次，然后提交新的 JSON。

  只支持本文件用到的那一小撮 YAML 语法（flow-style 映射 / 序列，可跨行）：

      providers:
        <provider-id>:
          apiKeyEnv: XXX
          api: openai-completions
          baseURL: https://...
          models:
            - { id: a, name: A, contextWindow: 1000, input: [ text, image ],
                reasoningEfforts: { off: null, low: low, high: high } }
            - { id: b, name: B, contextWindow: 2000 }

  不做的事：不解析锚点 / 多行标量 / 嵌套块序列 —— 遇到解析不出来的形状会**直接报错**，
  不会悄悄产出一份残缺的目录。

.PARAMETER Path
  源 YAML。默认 `%USERPROFILE%\.dsh\settings.yaml`。

.PARAMETER OutPath
  输出 JSON。默认 `server\src\main\resources\ai\builtin-catalog.json`。

.PARAMETER Version
  写进 `version` 字段的目录版本号。

.PARAMETER MinModels
  安全闸：解析到的模型数少于这个值就**拒绝写文件**（解析器退化时不至于覆盖掉好数据）。

.EXAMPLE
  pwsh -File scripts\gen-builtin-catalog.ps1
  pwsh -File scripts\gen-builtin-catalog.ps1 -OutPath $env:TEMP\catalog.json    # 只生成、用于 diff
#>
[CmdletBinding()]
param(
    [string]$Path = (Join-Path $env:USERPROFILE '.dsh\settings.yaml'),
    [string]$OutPath = (Join-Path $PSScriptRoot '..\server\src\main\resources\ai\builtin-catalog.json'),
    [string]$Version = '2026-09b-dsh',
    [int]$MinModels = 60
)

$ErrorActionPreference = 'Stop'

# 供应商 id → 显示名。YAML 里只有 id，显示名是给人看的，所以在这里补一张小表；
# 表里没有的由 id 推出来（连字符转空格 + 首字母大写）。
$ProviderDisplayNames = [ordered]@{
    'command-code-goat' = 'Command Code GOAT'
}

# ---------------------------------------------------------------------------
#  flow-style 解析（够用就好，不含完整 YAML）
# ---------------------------------------------------------------------------

function Split-FlowItems {
    param([string]$Text)
    $items = [System.Collections.Generic.List[string]]::new()
    $buf = [System.Text.StringBuilder]::new()
    $depth = 0
    $quoted = $false
    foreach ($ch in $Text.ToCharArray()) {
        if ($ch -eq '"') { $quoted = -not $quoted }
        if (-not $quoted) {
            if ($ch -eq '{' -or $ch -eq '[') { $depth++ }
            elseif ($ch -eq '}' -or $ch -eq ']') { $depth-- }
            elseif ($ch -eq ',' -and $depth -eq 0) {
                $items.Add($buf.ToString())
                [void]$buf.Clear()
                continue
            }
        }
        [void]$buf.Append($ch)
    }
    if ($buf.Length -gt 0) { $items.Add($buf.ToString()) }
    return $items.ToArray()
}

function ConvertFrom-FlowScalar {
    param([string]$Raw)
    $v = $Raw.Trim()
    if ($v -eq '' -or $v -eq 'null' -or $v -eq '~') { return $null }
    if ($v -eq 'true') { return $true }
    if ($v -eq 'false') { return $false }
    if ($v -match '^-?\d+$') { return [long]$v }
    if ($v.Length -ge 2 -and (($v.StartsWith('"') -and $v.EndsWith('"')) -or ($v.StartsWith("'") -and $v.EndsWith("'")))) {
        return $v.Substring(1, $v.Length - 2)
    }
    return $v
}

function ConvertFrom-FlowValue {
    param([string]$Raw)
    $v = $Raw.Trim()
    if ($v.StartsWith('{') -and $v.EndsWith('}')) {
        return (ConvertFrom-FlowMapping ($v.Substring(1, $v.Length - 2)))
    }
    if ($v.StartsWith('[') -and $v.EndsWith(']')) {
        $arr = [System.Collections.Generic.List[object]]::new()
        foreach ($item in (Split-FlowItems ($v.Substring(1, $v.Length - 2)))) {
            if ($item.Trim() -eq '') { continue }
            $arr.Add((ConvertFrom-FlowValue $item))
        }
        return , $arr.ToArray()
    }
    return (ConvertFrom-FlowScalar $v)
}

function ConvertFrom-FlowMapping {
    param([string]$Body)
    $map = [ordered]@{}
    foreach ($item in (Split-FlowItems $Body)) {
        $t = $item.Trim()
        if ($t -eq '') { continue }
        $idx = $t.IndexOf(':')
        if ($idx -lt 1) { throw "无法解析 flow 映射条目: '$t'" }
        $key = $t.Substring(0, $idx).Trim()
        $map[$key] = ConvertFrom-FlowValue ($t.Substring($idx + 1))
    }
    return $map
}

function Get-Indent {
    param([string]$Line)
    return ($Line.Length - $Line.TrimStart().Length)
}

# ---------------------------------------------------------------------------
#  读 YAML
# ---------------------------------------------------------------------------

if (-not (Test-Path -LiteralPath $Path)) {
    throw "找不到源文件: $Path（可用 -Path 指定；本机没装 DSH 时这份冻结副本才是真源）"
}
$sourcePath = (Resolve-Path -LiteralPath $Path).Path
$lines = (Get-Content -LiteralPath $sourcePath -Raw) -split "\r?\n"

# providers: 块（缩进 > providers 的行都属于它）
$providersIdx = -1
$providersIndent = 0
for ($i = 0; $i -lt $lines.Count; $i++) {
    if ($lines[$i] -match '^(\s*)providers:\s*$') {
        $providersIdx = $i
        $providersIndent = $Matches[1].Length
        break
    }
}
if ($providersIdx -lt 0) { throw "$sourcePath 里找不到 providers: 块" }

$blockLines = [System.Collections.Generic.List[string]]::new()
for ($i = $providersIdx + 1; $i -lt $lines.Count; $i++) {
    $line = $lines[$i]
    if ($line.Trim() -eq '') { continue }
    if ((Get-Indent $line) -le $providersIndent) { break }
    $blockLines.Add($line)
}
if ($blockLines.Count -eq 0) { throw "$sourcePath 的 providers: 块是空的" }

# 按最小缩进切出每个 provider
$providerBlocks = [System.Collections.Generic.List[object]]::new()
$providerIndent = $null
$current = $null
foreach ($line in $blockLines) {
    $indent = Get-Indent $line
    if ($null -eq $providerIndent) { $providerIndent = $indent }
    if ($indent -eq $providerIndent) {
        if ($null -ne $current) { $providerBlocks.Add($current) }
        $current = [ordered]@{ id = $line.Trim().TrimEnd(':'); lines = [System.Collections.Generic.List[string]]::new() }
        continue
    }
    if ($null -eq $current) { throw "providers 块里第一行不是 provider 名: '$line'" }
    $current.lines.Add($line.Trim())
}
if ($null -ne $current) { $providerBlocks.Add($current) }

# ---------------------------------------------------------------------------
#  组装目录
# ---------------------------------------------------------------------------

$providers = [System.Collections.Generic.List[object]]::new()
$totalModels = 0
$reasoningModels = 0

foreach ($pb in $providerBlocks) {
    $providerId = [string]$pb.id
    $header = [ordered]@{}
    $modelItems = [System.Collections.Generic.List[string]]::new()
    $inModels = $false
    $buf = $null

    foreach ($raw in $pb.lines) {
        if ($inModels) {
            if ($raw.StartsWith('-')) {
                if ($null -ne $buf) { $modelItems.Add($buf) }
                $buf = $raw.Substring(1).Trim()
            }
            else {
                if ($null -eq $buf) { throw "provider $providerId 的 models 列表里出现意外续行: '$raw'" }
                $buf = "$buf $raw"
            }
            continue
        }
        if ($raw -match '^([A-Za-z0-9_.\-]+):\s*(.*)$') {
            $key = $Matches[1]
            $value = $Matches[2]
            if ($key -eq 'models') { $inModels = $true; continue }
            $header[$key] = ConvertFrom-FlowScalar $value
        }
    }
    if ($null -ne $buf) { $modelItems.Add($buf) }

    $models = [System.Collections.Generic.List[object]]::new()
    foreach ($item in $modelItems) {
        $entry = ConvertFrom-FlowValue $item
        if ($entry -isnot [System.Collections.IDictionary]) { throw "provider $providerId 的模型条目不是映射: '$item'" }
        foreach ($required in @('id', 'name')) {
            if (-not $entry.Contains($required)) { throw "provider $providerId 的模型条目缺 $required : '$item'" }
        }

        $modalities = [System.Collections.Generic.List[string]]::new()
        if ($entry.Contains('input') -and $null -ne $entry['input']) {
            foreach ($m in $entry['input']) { $modalities.Add([string]$m) }
        }
        else {
            $modalities.Add('text')   # 没写 input ⇒ 仅文本
        }

        $efforts = [ordered]@{}
        if ($entry.Contains('reasoningEfforts') -and $null -ne $entry['reasoningEfforts']) {
            foreach ($level in $entry['reasoningEfforts'].Keys) {
                if ($level -eq 'off') { continue }                       # off 是隐含的，不进目录
                $wire = $entry['reasoningEfforts'][$level]
                if ($null -eq $wire) { continue }
                $efforts[$level] = [string]$wire
            }
        }
        $reasoning = ($efforts.Count -gt 0)
        if ($reasoning) { $reasoningModels++ }

        $context = $null
        if ($entry.Contains('contextWindow') -and $null -ne $entry['contextWindow']) {
            $context = [long]$entry['contextWindow']
        }

        $models.Add([ordered]@{
                id              = [string]$entry['id']
                displayName     = [string]$entry['name']
                contextWindow   = $context
                inputModalities = $modalities.ToArray()
                tools           = $true    # 项目约定：工具能力默认给上
                parallelTools   = $false
                reasoning       = $reasoning
                thinkingFormat  = 'openai' # 该 provider 是 openai-completions，方言就是默认的 openai
                thinkingEfforts = $efforts
            })
    }

    $displayName = if ($ProviderDisplayNames.Contains($providerId)) {
        $ProviderDisplayNames[$providerId]
    }
    else {
        (($providerId -split '[-_]') | ForEach-Object { if ($_.Length -gt 0) { $_.Substring(0, 1).ToUpper() + $_.Substring(1) } }) -join ' '
    }

    $totalModels += $models.Count
    $providers.Add([ordered]@{
            id            = $providerId
            displayName   = $displayName
            api           = [string]$header['api']
            baseURL       = [string]$header['baseURL']
            credentialRef = [string]$header['apiKeyEnv']
            endpointTrust = 'public'
            models        = $models.ToArray()
        })
}

# agent-default-model（顶层块，给"新建会话用哪个模型"当默认值）
$agentDefault = $null
for ($i = 0; $i -lt $lines.Count; $i++) {
    if ($lines[$i] -match '^agent-default-model:\s*$') {
        $block = [ordered]@{}
        for ($j = $i + 1; $j -lt $lines.Count; $j++) {
            if ($lines[$j].Trim() -eq '') { continue }
            if ((Get-Indent $lines[$j]) -eq 0) { break }
            if ($lines[$j] -match '^\s*([A-Za-z0-9_.\-]+):\s*(.+)$') {
                $block[$Matches[1]] = [string](ConvertFrom-FlowScalar $Matches[2])
            }
        }
        if ($block.Contains('provider') -and $block.Contains('model')) {
            $agentDefault = [ordered]@{ provider = $block['provider']; model = $block['model'] }
        }
        break
    }
}

$catalog = [ordered]@{
    version           = $Version
    source            = '.dsh/settings.yaml'
    note              = '由 scripts/gen-builtin-catalog.ps1 从本机 .dsh/settings.yaml 生成；运行时只读这份冻结副本，不读 YAML。'
    agentDefaultModel = $agentDefault
    providers         = $providers.ToArray()
}

# ---------------------------------------------------------------------------
#  安全闸 + 写盘
# ---------------------------------------------------------------------------

if ($totalModels -lt $MinModels) {
    throw "只解析到 $totalModels 个模型（少于 $MinModels），拒绝写入：$sourcePath 的格式可能变了，请检查解析器。"
}

$resolvedOut = [System.IO.Path]::GetFullPath($OutPath)
$outDir = Split-Path -Parent $resolvedOut
if (-not (Test-Path -LiteralPath $outDir)) { New-Item -ItemType Directory -Path $outDir -Force | Out-Null }

$json = $catalog | ConvertTo-Json -Depth 20
# 统一写成 LF、UTF-8 无 BOM：仓库里其他文本文件都是 LF，
# 这样 `git diff` 和"生成结果 == 冻结副本"的比对都不会被行尾符干扰。
$json = ($json -replace "`r`n", "`n")
if (-not $json.EndsWith("`n")) { $json += "`n" }
[System.IO.File]::WriteAllText($resolvedOut, $json, [System.Text.UTF8Encoding]::new($false))

$sha = (Get-FileHash -LiteralPath $resolvedOut -Algorithm SHA256).Hash.ToLower()

Write-Host "[gen-builtin-catalog] 源        : $sourcePath"
Write-Host "[gen-builtin-catalog] 输出      : $resolvedOut"
Write-Host "[gen-builtin-catalog] 版本      : $Version"
Write-Host "[gen-builtin-catalog] provider  : $($providers.Count)"
Write-Host "[gen-builtin-catalog] 模型      : $totalModels（其中 $reasoningModels 个声明了思考档位）"
Write-Host "[gen-builtin-catalog] 默认模型  : $(if ($agentDefault) { "$($agentDefault['provider']) / $($agentDefault['model'])" } else { '（无）' })"
Write-Host "[gen-builtin-catalog] sha256    : $sha"
