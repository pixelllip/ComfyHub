<#
.SYNOPSIS
    抽视频封面（预览图）—— 用 Windows 自带能力，不依赖 ffmpeg。

.DESCRIPTION
    播放器在解码出第一帧之前需要一张封面，否则会先黑一片。ffmpeg 不是每台机器都有，
    但 Windows 资源管理器的缩略图管线（Media Foundation）对所有能播的视频都能出图，
    所以这里用 Shell 的 IShellItemImageFactory 取缩略图，再存成 PNG 缓存。

    编码走 WPF 的 PngBitmapEncoder，**不用 System.Drawing**：
    pwsh 7 跑在 .NET 10 上，`System.Drawing.Common` 在非 Windows 桌面框架里已经
    不能直接 Add-Type 引用了（GdiPlus 被拆到 System.Private.Windows.GdiPlus）。

    · 抽不出图时输出 `FAIL <原因>` 并以退出码 1 结束，让调用方走"没有封面"的
      降级路径（播放器转圈），不阻塞播放；
    · 已经存在的缓存直接复用（除非 -Force）。

.EXAMPLE
    pwsh -File scripts\video-poster.ps1 -Source D:\a.mp4 -Dest .run\a.png
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Source,
    [Parameter(Mandatory = $true)][string]$Dest,
    [int]$Size = 640,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $Source -PathType Leaf)) {
    Write-Output "FAIL 源文件不存在: $Source"
    exit 1
}
if ((Test-Path -LiteralPath $Dest) -and -not $Force) {
    Write-Output "OK 已存在: $Dest"
    exit 0
}

$destDir = Split-Path -Parent $Dest
if ($destDir -and -not (Test-Path -LiteralPath $destDir)) {
    New-Item -ItemType Directory -Path $destDir -Force | Out-Null
}

# Dest 可能是相对路径，也可能是绝对的（后端总是传绝对路径）：
# 直接 Join-Path 会把绝对路径再拼到当前目录后面，变成
# "D:\proj\D:\proj\storage\..." 这种非法路径。
$DestFull = if ([System.IO.Path]::IsPathRooted($Dest)) {
    [System.IO.Path]::GetFullPath($Dest)
} else {
    [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $Dest))
}
$Dest = $DestFull

Add-Type -AssemblyName PresentationCore, WindowsBase

if (-not ('ComfyHubPoster' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

[ComImport, Guid("bcc18b79-ba16-442f-80c4-8a59c30c463b"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IShellItemImageFactory
{
    void GetImage(SIZE size, SIIGBF flags, out IntPtr phbm);
}

[StructLayout(LayoutKind.Sequential)]
public struct SIZE { public int cx; public int cy; }

[Flags]
public enum SIIGBF : uint
{
    RESIZETOFIT = 0x00,
    BIGGERSIZEOK = 0x01,
    MEMORYONLY = 0x02,
    ICONONLY = 0x04,
    THUMBNAILONLY = 0x08,
    INCACHEONLY = 0x10
}

public static class ComfyHubPoster
{
    [DllImport("shell32.dll", CharSet = CharSet.Unicode, PreserveSig = false)]
    static extern void SHCreateItemFromParsingName(string path, IntPtr pbc, ref Guid riid,
        [MarshalAs(UnmanagedType.Interface)] out object ppv);

    /// 只取缩略图（THUMBNAILONLY 保证不会退化成文件图标）。
    public static IntPtr ExtractThumbnail(string src, int size)
    {
        var iid = new Guid("bcc18b79-ba16-442f-80c4-8a59c30c463b");
        object obj;
        SHCreateItemFromParsingName(src, IntPtr.Zero, ref iid, out obj);
        var factory = (IShellItemImageFactory)obj;
        IntPtr hbm;
        factory.GetImage(new SIZE { cx = size, cy = size }, SIIGBF.THUMBNAILONLY, out hbm);
        return hbm;
    }
}
'@
}

try {
    $full = (Resolve-Path -LiteralPath $Source).Path
    $hbm = [ComfyHubPoster]::ExtractThumbnail($full, $Size)
} catch {
    Write-Output "FAIL 抽帧失败: $($_.Exception.Message)"
    exit 1
}

if ($hbm -eq [IntPtr]::Zero) {
    Write-Output "FAIL 系统缩略图管线没有给出画面（可能缺少解码器）"
    exit 1
}

try {
    $bmp = [System.Windows.Interop.Imaging]::CreateBitmapSourceFromHBitmap(
        $hbm, [IntPtr]::Zero, [System.Windows.Int32Rect]::Empty,
        [System.Windows.Media.Imaging.BitmapSizeOptions]::FromEmptyOptions())
    $encoder = New-Object System.Windows.Media.Imaging.PngBitmapEncoder
    $encoder.Frames.Add([System.Windows.Media.Imaging.BitmapFrame]::Create($bmp))
    $stream = [System.IO.File]::Create($Dest)
    try { $encoder.Save($stream) } finally { $stream.Dispose() }
} catch {
    Write-Output "FAIL 写封面失败: $($_.Exception.Message)"
    exit 1
} finally {
    [void][ComfyHubPoster]  # 保持类型引用
    if ($hbm -ne [IntPtr]::Zero) {
        Add-Type -Namespace ComfyHubGdi -Name Native -MemberDefinition @'
[DllImport("gdi32.dll")] public static extern bool DeleteObject(IntPtr hObject);
'@ -ErrorAction SilentlyContinue
        [ComfyHubGdi.Native]::DeleteObject($hbm) | Out-Null
    }
}

if (-not (Test-Path -LiteralPath $Dest)) {
    Write-Output "FAIL 没有生成文件"
    exit 1
}
Write-Output "OK $Dest"
exit 0
