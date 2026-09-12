<#
.SYNOPSIS
    ComfyHub 运行时一键就绪：检测不到的运行时自动下载安装。

.DESCRIPTION
    发布包只带走了 Java/MySQL 之类**能打包**的东西；真正难搞的是目标机器上
    "少了某个运行时"，而且报错往往没头没尾：

      · 没有 Java        → 后端起不来（"等待后端超时"）
      · 没有 VC++ 运行时  → mysqld.exe 起不来，原因只写进 mysql-error.log
      · 没有 PowerShell 7 → 脚本之间 15 处 `& pwsh` 互调全失败

    这个脚本把这三件事一次办完：

      Java        → **自动下载**一个便携版 JRE 21（Temurin）解压到 <根>\jre，
                    免安装、免管理员；带 sha256 校验，多个下载源依次重试。
      PowerShell 7 / VC++ 运行时
                  → 有 winget 就自动 `winget install`（VC++ 需要管理员，会弹 UAC）；
                    没有 winget 就打印官方下载地址。

    具体实现都在 scripts\runtime-deps.ps1（comfyhub.ps1 也 dot-source 它）。

.EXAMPLE
    pwsh -File scripts\ensure-runtime.ps1              # 检测 + 缺什么装什么
    pwsh -File scripts\ensure-runtime.ps1 -CheckOnly   # 只看，不装（doctor 用的就是这个）
    pwsh -File scripts\ensure-runtime.ps1 -JavaOnly    # 只保证 Java（comfyhub.ps1 up 走的这个）
    pwsh -File scripts\ensure-runtime.ps1 -Force       # 已存在也重下一份
    pwsh -File scripts\ensure-runtime.ps1 -NoSystem    # Java 自动装，系统级的只提示
    pwsh -File scripts\ensure-runtime.ps1 -Proxy http://127.0.0.1:7890   # 走代理下载

.NOTES
    退出码：0 = 所有运行时都就绪；1 = 还有没搞定的（原因见输出）。
#>
[CmdletBinding()]
param(
    # 已存在也重新下载安装
    [switch]$Force,

    # 只检测、不安装（doctor / CI 用）
    [switch]$CheckOnly,

    # 少输出
    [switch]$Quiet,

    # 只保证 Java（其余不碰）—— comfyhub.ps1 up 走的就是这个，免得启动时弹 UAC
    [switch]$JavaOnly,

    # 系统级运行时（pwsh / VC++）只报告、不自动装
    [switch]$NoSystem,

    # 显式代理（不传则沿用系统/环境里的代理设置）
    [string]$Proxy
)

$ErrorActionPreference = 'Stop'
$ProjectRoot = Split-Path -Parent $PSScriptRoot

# 与其它脚本一致：被重定向时把输出钉成 UTF-8（App 按 UTF-8 解）
if ([Console]::IsOutputRedirected) {
    try { [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false) } catch { }
}

$helper = Join-Path $PSScriptRoot 'runtime-deps.ps1'
if (-not (Test-Path $helper)) {
    Write-Host "找不到 $helper，无法检测运行时。" -ForegroundColor Red
    exit 1
}
. $helper

if (-not $Quiet) {
    Write-Host ''
    Write-Host '  ComfyHub 运行时检查' 'White'
    Write-Host '  ────────────────────────────────────────────────────────────'
    Write-Host "  项目根目录: $ProjectRoot" 'DarkGray'
    if ($CheckOnly) { Write-Host '  模式: 只检测（-CheckOnly）' 'DarkGray' }
}

try {
    $ok = Invoke-RuntimeEnsure -ProjectRoot $ProjectRoot `
        -Force:$Force -CheckOnly:$CheckOnly -Quiet:$Quiet -JavaOnly:$JavaOnly -NoSystem:$NoSystem -Proxy $Proxy
} catch {
    Write-Host "运行时处理出错: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}

if ($ok) { exit 0 }
exit 1
