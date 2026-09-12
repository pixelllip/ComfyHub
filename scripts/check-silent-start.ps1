<#
.SYNOPSIS
    验证「MySQL + 后端是静默启动的」：启动服务的同时盯着屏幕，报告有没有弹出窗口。

.DESCRIPTION
    为什么需要它：`Get-Process` 的 MainWindowHandle 判断不了这件事 ——
    Windows 的控制台窗口属于 **conhost.exe**，不属于那个控制台程序本身，
    所以 mysqld / java / cmd 的 MainWindowHandle 永远是 0，会给你一个假绿灯。
    正确做法是枚举**可见的顶层窗口**（EnumWindows + IsWindowVisible），
    并且以 ~50ms 的间隔轮询 —— 否则"闪一下就没"的窗口根本抓不到。

    启动方式完全走统一入口 comfyhub.ps1，和 App 里点「启动 / 修复」是同一条路。
    默认加 -SkipBuild（直接用上次的后端产物，几秒就能出结果）；加 -Build 连构建一起测。

.EXAMPLE
    pwsh -File scripts\check-silent-start.ps1
    pwsh -File scripts\check-silent-start.ps1 -Restart -Build
#>
[CmdletBinding()]
param(
    # 先 down 再 up，完整走一遍「从零启动」的路径（会短暂中断服务）
    [switch]$Restart,
    # 允许跑 gradle 构建（默认 -SkipBuild，快）
    [switch]$Build,
    [int]$Seconds = 300
)

$ErrorActionPreference = 'Continue'
$ComfyHub = Join-Path $PSScriptRoot 'comfyhub.ps1'
$allowBuild = [bool]$Build.IsPresent

Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public class SilentStartProbe {
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc lpEnumFunc, IntPtr lParam);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr hWnd);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowTextW(IntPtr hWnd, StringBuilder s, int n);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
  delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
  public static List<string> Visible() {
    var list = new List<string>();
    EnumWindows((h, l) => {
      if (IsWindowVisible(h)) {
        var sb = new StringBuilder(512);
        GetWindowTextW(h, sb, 512);
        uint pid; GetWindowThreadProcessId(h, out pid);
        list.Add(pid + "|" + sb.ToString());
      }
      return true;
    }, IntPtr.Zero);
    return list;
  }
}
'@

function Get-VisibleWindows { [SilentStartProbe]::Visible() }

# 发现窗口的**当时**就把进程名解析出来（窗口一闪而过的话，事后再查就查不到了）
function Describe([string]$Entry) {
    $parts = $Entry -split '\|', 2
    $procId = [int]$parts[0]
    $title = if ($parts.Count -gt 1) { $parts[1] } else { '' }
    $p = Get-Process -Id $procId -ErrorAction SilentlyContinue
    $name = if ($p) { $p.ProcessName } else { '（已退出）' }
    return "PID=$procId  进程=$name  标题=`"$title`""
}

if ($Restart) {
    Write-Host '==> 先停止现有服务（-Restart）' -ForegroundColor Cyan
    & pwsh -NoProfile -File $ComfyHub down 2>&1 | Out-Null
    Start-Sleep -Seconds 2
}

$seen = @{}
foreach ($w in Get-VisibleWindows) { $seen[$w] = $true }
$baseline = $seen.Count
Write-Host "==> 基线：$baseline 个可见窗口；开始以 50ms 间隔盯屏，同时启动服务…" -ForegroundColor Cyan

$started = Get-Date
$job = Start-Job -ScriptBlock {
    $a = @('up')
    if (-not $using:allowBuild) { $a += '-SkipBuild' }
    & pwsh -NoProfile -ExecutionPolicy Bypass -File $using:ComfyHub @a 2>&1
}

$hits = New-Object System.Collections.Generic.List[string]
$deadline = (Get-Date).AddSeconds($Seconds)
while ($job.State -eq 'Running' -and (Get-Date) -lt $deadline) {
    foreach ($w in Get-VisibleWindows) {
        if (-not $seen.ContainsKey($w)) {
            $seen[$w] = $true
            $line = "[{0}] 新窗口: {1}" -f (Get-Date -Format 'HH:mm:ss.fff'), (Describe $w)
            $hits.Add($line)
            Write-Host "  $line" -ForegroundColor Yellow
        }
    }
    Start-Sleep -Milliseconds 50
}

$timedOut = $job.State -eq 'Running'
$out = Receive-Job $job
Remove-Job $job -Force
$out | ForEach-Object { Write-Host "    $_" -ForegroundColor DarkGray }
if ($timedOut) { Write-Host "  !! 等待超时（$Seconds 秒），启动过程可能还没结束" -ForegroundColor Yellow }

Write-Host ''
Write-Host "基线 $baseline 个窗口；盯屏 $([int]((Get-Date) - $started).TotalSeconds) 秒，发现 $($hits.Count) 个新窗口" -ForegroundColor Cyan

$noisy = @($hits | Where-Object { $_ -match '进程=(cmd|conhost|java|javaw|mysqld|wscript|pwsh|powershell)\s' })
if ($noisy.Count -gt 0) {
    Write-Host '结果: 出现了命令行 / 服务相关的窗口，静默启动被破坏了 ——' -ForegroundColor Red
    $noisy | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
    exit 1
}
if ($hits.Count -eq 0) {
    Write-Host '结果: 全程没有出现任何新窗口 —— 静默启动 OK' -ForegroundColor Green
} else {
    Write-Host '结果: 新窗口都与服务无关（多半是浏览器等本来就在用的程序）—— 静默启动 OK' -ForegroundColor Green
}
exit 0
