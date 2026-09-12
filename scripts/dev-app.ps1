<#
.SYNOPSIS
    前端开发模式：确保 MySQL + 后端在跑，然后用 **debug 版**把 App 跑起来（支持热重载）。

.DESCRIPTION
    只改 lib/ 下的 Dart 前端代码时走这条路，别每次都 flutter build windows：
      · debug 版编译比 Release 完整构建快得多；
      · 跑起来之后在终端里按 r 热重载（保留状态）、R 热重启、q 退出；
      · 后端 / MySQL 由脚本和 App 自己保证，不用手动开。

    什么时候不要用它：改了 windows/ 原生代码或 pubspec 依赖（要重新编译），
    以及要出正式产物交给别人用时 —— 那些走 scripts\autorun-app.ps1（Release）。

.EXAMPLE
    pwsh -File scripts\dev-app.ps1
    pwsh -File scripts\dev-app.ps1 -SkipServices   # 服务已经起好了，直接跑 App
#>
[CmdletBinding()]
param(
    [switch]$SkipServices
)

$ErrorActionPreference = 'Stop'
$ProjectRoot = Split-Path -Parent $PSScriptRoot

# 与 autorun-app.ps1 保持一致：国内镜像对个别包返回 424，直连 pub.dev 更稳
$env:PUB_HOSTED_URL = 'https://pub.dev'
$env:FLUTTER_SUPPRESS_ANALYTICS = 'true'
$env:PATH = "C:\Users\$env:USERNAME\flutter\bin;$env:PATH"

if (-not (Get-Command flutter -ErrorAction SilentlyContinue)) {
    throw 'PATH 里找不到 flutter，请检查 Flutter SDK 安装位置。'
}

if (-not $SkipServices) {
    Write-Host '==> 确保 MySQL + 后端在跑（-SkipBuild：直接用上次的产物）' -ForegroundColor Cyan
    & pwsh -NoProfile -File (Join-Path $PSScriptRoot 'comfyhub.ps1') up -SkipBuild
}

Push-Location $ProjectRoot
try {
    Write-Host '==> flutter run -d windows --debug' -ForegroundColor Cyan
    Write-Host '    跑起来之后：r = 热重载（保留状态）/ R = 热重启 / q = 退出' -ForegroundColor DarkGray
    Write-Host '    改的只是 lib/ 下的 Dart 代码时，热重载 1~2 秒就能看到效果。' -ForegroundColor DarkGray
    & flutter run -d windows --debug
    exit $LASTEXITCODE
} finally {
    Pop-Location
}
