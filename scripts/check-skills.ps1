<#
.SYNOPSIS
    打包前体检：核对并修正 skills 的转义，再校验每份 SKILL.md 的 frontmatter。

.DESCRIPTION
    症状：SKILL.md 首行的 frontmatter 定界符变成 `\---`，正文里的 `[` / `_` / `~` / `.` / `-`
    前面被插入反斜杠（`\\\[CHARACTER CARD]`、`300\~400`、`## 1\.`）。这种形态来自文本被字符串
    序列化过一次，不是编辑器保存文件的行为。

    后端的 frontmatter 解析只认首行恰好是 `---`（server-rs\src\ai\skills.rs 的
    parse_front_matter），于是整份文件按"没有 frontmatter"处理：description 取到空串，
    判成 `validationError: 缺少 description`。结果是名单里仍有这一项，但 available 为 false、
    load_skill 取不到、系统提示不注入 —— 模型读不到这份 skill 的任何规则。

    本脚本按顺序做两件事：
      1. 认转义签名并修正：首行是 `\---`，或正文里有连续两个以上反斜杠紧跟 `[` / `_`。
         只改带签名的文件；正常 skill 里的 Windows 路径（`models\rembg\`、`user\<用户>\`）
         与 `\n` 这类字面量不动。
      2. 逐份核对 SKILL.md：首行 `---`、frontmatter 闭合、name 与目录名一致、description 非空。
    修不好就抛错，调用方据此中止打包 —— 带着坏 skill 出包，比打包失败更难发现。

.PARAMETER Root
    仓库根目录（该目录下要有 skills\）。默认取本脚本所在目录的上一级。

.EXAMPLE
    pwsh -File scripts\check-skills.ps1
    pwsh -File scripts\check-skills.ps1 -Root D:\myProject\FlutterProject\viewer
#>
[CmdletBinding()]
param([string]$Root)

$ErrorActionPreference = 'Stop'

# 输出编码**不**在这里钉死：`flutter build windows` 经 MSBuild 调本脚本时，MSBuild 按
# 控制台代码页解码子进程输出，这里改成 UTF-8 会让中文在构建日志里变成乱码。
# 需要 UTF-8 的调用方（pack-release.ps1 被重定向时）在它自己的进程里钉。

if (-not $Root) { $Root = Split-Path -Parent $PSScriptRoot }
$Root = (Resolve-Path -LiteralPath $Root).Path.TrimEnd('\')
$SkillsDir = Join-Path $Root 'skills'
if (-not (Test-Path -LiteralPath $SkillsDir)) {
    throw "找不到 skills 目录：$SkillsDir"
}

function Say([string]$msg, [string]$color = 'Gray') { Write-Host $msg -ForegroundColor $color }

function Get-FirstLine([string]$Text) {
    <# 首行（去掉 BOM 与首尾空白）—— frontmatter 定界符只认这一行 #>
    $line = ($Text -split "`r?`n", 2)[0]
    if ($null -eq $line) { return '' }
    return $line.TrimStart([char]0xFEFF).Trim()
}

function Test-EscapedText([string]$Text) {
    <# 两种签名：首行 `\---`；正文里连续两个以上反斜杠紧跟 `[` 或 `_`。
       `\\\[` 这种三反斜杠形态不可能是合法 Markdown，只可能来自转义。 #>
    if ((Get-FirstLine $Text) -eq '\---') { return $true }
    return ($Text -match '\\{2,}[\[_]')
}

function Repair-EscapedText([string]$Text) {
    <# 去掉紧贴在 Markdown 标点前的反斜杠（含连续多个）。
       只看 `[ ] _ . ~ -` 这六类：`\.`、`\~`、`\-` 在 Markdown 里与不转义渲染结果相同，
       而 Windows 路径（反斜杠后面是字母 / 汉字）不在替换范围内。 #>
    $before = ([regex]::Matches($Text, '\\')).Count
    $fixed = [regex]::Replace($Text, '\\+(?=[\[\]_.~\-])', '')
    $after = ([regex]::Matches($fixed, '\\')).Count
    return [pscustomobject]@{ Text = $fixed; Removed = ($before - $after); Remaining = $after }
}

function Get-FrontMatterError([string]$Text, [string]$DirName) {
    <# 与后端 parse_one 的判据对齐：首行 `---`、有闭合、name 是 kebab-case 且与目录名一致、
       description 非空、正文非空。返回人类可读的错误；合法返回 $null。 #>
    $lines = $Text -split "`r?`n"
    $head = if ($lines.Count -gt 0) { $lines[0].TrimStart([char]0xFEFF).Trim() } else { '' }
    if ($head -ne '---') { return "第一行是「$head」，不是 frontmatter 起始的 ---" }

    $meta = @{}
    $closed = $false
    $bodyStart = -1
    $i = 1
    while ($i -lt $lines.Count) {
        $t = $lines[$i].Trim()
        if ($t -eq '---') { $closed = $true; $bodyStart = $i + 1; break }
        if ($t -eq '' -or $t.StartsWith('#')) { $i++; continue }
        $idx = $t.IndexOf(':')
        if ($idx -le 0) { $i++; continue }
        $key = $t.Substring(0, $idx).Trim().ToLower().Replace('_', '-')
        $val = $t.Substring($idx + 1).Trim()
        if ($val -match '^[|>][+-]?$') {
            # 块标量：收集后续缩进行（`description: |` 这种写法后端也认）
            $buf = @()
            $j = $i + 1
            while ($j -lt $lines.Count -and ($lines[$j] -match '^[ \t]' -or $lines[$j].Trim() -eq '')) {
                $buf += $lines[$j].Trim()
                $j++
            }
            $val = ($buf -join ' ').Trim()
            $i = $j
        } else {
            $i++
        }
        $meta[$key] = $val
    }

    if (-not $closed) { return 'frontmatter 没有闭合（缺少第二个 ---）' }
    if (-not $meta.ContainsKey('name') -or -not $meta['name']) { return '缺少 name' }
    $name = $meta['name']
    if ($name -notmatch '^[a-z0-9]+(?:-[a-z0-9]+)*$' -or $name.Length -gt 64) {
        return "name「$name」不是 kebab-case（小写字母 / 数字 / 连字符，≤64）"
    }
    if ($name -ne $DirName) {
        return "frontmatter 的 name「$name」与目录名「$DirName」不一致"
    }
    if (-not $meta.ContainsKey('description') -or -not $meta['description']) {
        return '缺少 description'
    }
    $body = if ($bodyStart -ge 0 -and $bodyStart -lt $lines.Count) {
        (($lines[$bodyStart..($lines.Count - 1)]) -join "`n").Trim()
    } else { '' }
    if (-not $body) { return '正文为空' }
    return $null
}

$files = @(Get-ChildItem -LiteralPath $SkillsDir -Recurse -File -Filter '*.md' -ErrorAction SilentlyContinue |
           Sort-Object FullName)
$BuiltinDir = (Join-Path $SkillsDir 'builtin').TrimEnd('\')

$fixedCount = 0
$problems = New-Object System.Collections.Generic.List[string]
$warnings = New-Object System.Collections.Generic.List[string]

foreach ($f in $files) {
    $rel = $f.FullName.Substring($Root.Length + 1)
    $text = [System.IO.File]::ReadAllText($f.FullName)

    if (Test-EscapedText $text) {
        $r = Repair-EscapedText $text
        [System.IO.File]::WriteAllText($f.FullName, $r.Text, [System.Text.UTF8Encoding]::new($false))
        $text = $r.Text
        $fixedCount++
        Say ("  已修正转义：{0}（去掉 {1} 个反斜杠，保留 {2} 个）" -f $rel, $r.Removed, $r.Remaining) 'Yellow'
        if ($r.Remaining -gt 0) {
            $warnings.Add("$rel 修正后仍有 $($r.Remaining) 个反斜杠，请确认不是漏掉的转义")
        }
    }

    # 只校验 skill 条目本身：`<name>\SKILL.md` 或投放根下的 `<name>.md`；
    # `references\*.md` 这类正文附件不校验 frontmatter。
    $dirName = $null
    if ($f.Name -ieq 'SKILL.md') {
        $dirName = $f.Directory.Name
    } elseif ($f.Directory.FullName.TrimEnd('\') -ieq $BuiltinDir) {
        $dirName = [System.IO.Path]::GetFileNameWithoutExtension($f.Name)
    }
    if ($dirName) {
        $err = Get-FrontMatterError -Text $text -DirName $dirName
        if ($err) { $problems.Add("$rel：$err") }
    }
}

Say ''
Say ("  skills 体检：核对 {0} 个 .md，修正 {1} 个" -f $files.Count, $fixedCount) 'Gray'
foreach ($w in $warnings) { Say "  !! $w" 'Yellow' }

if ($problems.Count -gt 0) {
    foreach ($p in $problems) { Say "  !! $p" 'Red' }
    throw ("skills 体检未通过（{0} 份 SKILL.md 不合法）。修好再打包：列表里 available 为 false 的 skill，出包后模型也读不到。" -f $problems.Count)
}

Say '  skills 体检通过' 'Green'
