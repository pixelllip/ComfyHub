<#
.SYNOPSIS
    生成一组演示用的产物文件（图片 / 音频），放到 samples/ 目录。

.DESCRIPTION
    用来快速体验「上传产物 -> 关联提示词 -> 按标签搜索」的完整流程。
    图片用 System.Drawing 生成渐变图 + 文字，音频用纯字节写出一个正弦波 WAV。
    视频无法凭空生成，请自行用 ComfyUI 的输出文件测试。

.EXAMPLE
    pwsh -File scripts\make-sample-media.ps1
#>
[CmdletBinding()]
param(
    [string]$OutDir
)

$ErrorActionPreference = 'Stop'
$ProjectRoot = Split-Path -Parent $PSScriptRoot
if (-not $OutDir) { $OutDir = Join-Path $ProjectRoot 'samples' }

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
Add-Type -AssemblyName System.Drawing

function New-GradientImage {
    param(
        [string]$Path,
        [string]$Title,
        [string]$Subtitle,
        [System.Drawing.Color]$From,
        [System.Drawing.Color]$To,
        [int]$Width = 1216,
        [int]$Height = 832
    )

    $bmp = New-Object System.Drawing.Bitmap($Width, $Height)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    try {
        $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $rect = New-Object System.Drawing.Rectangle(0, 0, $Width, $Height)
        $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush($rect, $From, $To, 35.0)
        $g.FillRectangle($brush, $rect)
        $brush.Dispose()

        # 装饰圆
        for ($i = 0; $i -lt 7; $i++) {
            $r = 60 + $i * 34
            $a = 22 - $i * 2
            $pen = New-Object System.Drawing.Pen(
                [System.Drawing.Color]::FromArgb([Math]::Max($a, 4), 255, 255, 255), 2.0)
            $g.DrawEllipse($pen, ($Width / 2 - $r), ($Height / 2 - $r + 40), ($r * 2), ($r * 2))
            $pen.Dispose()
        }

        $fontBig = New-Object System.Drawing.Font('Segoe UI', 46, [System.Drawing.FontStyle]::Bold)
        $fontSmall = New-Object System.Drawing.Font('Segoe UI', 20)
        $white = [System.Drawing.Brushes]::White
        $soft = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(215, 255, 255, 255))

        $fmt = New-Object System.Drawing.StringFormat
        $fmt.Alignment = [System.Drawing.StringAlignment]::Center

        $g.DrawString($Title, $fontBig, $white, ($Width / 2), ($Height / 2 - 90), $fmt)
        $g.DrawString($Subtitle, $fontSmall, $soft, ($Width / 2), ($Height / 2 - 20), $fmt)
        $g.DrawString('ComfyHub sample', $fontSmall, $soft, ($Width / 2), ($Height - 70), $fmt)

        $fontBig.Dispose(); $fontSmall.Dispose(); $soft.Dispose(); $fmt.Dispose()
    } finally {
        $g.Dispose()
    }

    $bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    Write-Host "  + $Path" -ForegroundColor Green
}

function New-WavFile {
    param(
        [string]$Path,
        [double]$Seconds = 4.0,
        [int]$SampleRate = 44100
    )

    $samples = [int]($Seconds * $SampleRate)
    $dataSize = $samples * 2
    $ms = New-Object System.IO.MemoryStream
    $bw = New-Object System.IO.BinaryWriter($ms)

    # RIFF header
    $bw.Write([char[]]'RIFF')
    $bw.Write([int](36 + $dataSize))
    $bw.Write([char[]]'WAVE')
    $bw.Write([char[]]'fmt ')
    $bw.Write([int]16)
    $bw.Write([int16]1)        # PCM
    $bw.Write([int16]1)        # mono
    $bw.Write([int]$SampleRate)
    $bw.Write([int]($SampleRate * 2))
    $bw.Write([int16]2)
    $bw.Write([int16]16)
    $bw.Write([char[]]'data')
    $bw.Write([int]$dataSize)

    # 简单的和弦 + 淡入淡出，听感上像个 lo-fi pad
    $freqs = @(220.0, 277.18, 329.63, 440.0)
    for ($i = 0; $i -lt $samples; $i++) {
        $t = $i / $SampleRate
        $env = [Math]::Min(1.0, $t / 0.35) * [Math]::Min(1.0, ($Seconds - $t) / 0.6)
        $v = 0.0
        foreach ($f in $freqs) { $v += [Math]::Sin(2 * [Math]::PI * $f * $t) }
        $v = ($v / $freqs.Count) * 0.55 * $env
        $s = [int16]([Math]::Max(-32767, [Math]::Min(32767, $v * 32767)))
        $bw.Write($s)
    }

    $bw.Flush()
    [System.IO.File]::WriteAllBytes($Path, $ms.ToArray())
    $bw.Dispose(); $ms.Dispose()
    Write-Host "  + $Path" -ForegroundColor Green
}

Write-Host "==> 生成演示图片" -ForegroundColor Cyan
New-GradientImage -Path (Join-Path $OutDir 'neon-street.png') `
    -Title '雨夜霓虹街道' -Subtitle 'cyberpunk city street at night' `
    -From ([System.Drawing.Color]::FromArgb(18, 12, 48)) `
    -To   ([System.Drawing.Color]::FromArgb(180, 30, 120))

New-GradientImage -Path (Join-Path $OutDir 'ink-lotus.png') `
    -Title '古风少女 · 水墨' -Subtitle 'traditional chinese ink painting' `
    -From ([System.Drawing.Color]::FromArgb(238, 234, 224)) `
    -To   ([System.Drawing.Color]::FromArgb(120, 130, 120))

New-GradientImage -Path (Join-Path $OutDir 'product-earbud.png') `
    -Title '产品广告静帧' -Subtitle 'matte black earbud, studio lighting' `
    -From ([System.Drawing.Color]::FromArgb(20, 20, 24)) `
    -To   ([System.Drawing.Color]::FromArgb(70, 70, 90)) `
    -Width 1024 -Height 1024

Write-Host "==> 生成演示音频" -ForegroundColor Cyan
New-WavFile -Path (Join-Path $OutDir 'lofi-rain.wav') -Seconds 4.0

Write-Host ""
Write-Host "完成，文件位于: $OutDir" -ForegroundColor Green
Write-Host "可以在 App 的「画廊 -> 上传」里选中这些文件试试。" -ForegroundColor Green
