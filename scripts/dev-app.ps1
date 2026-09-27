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

# ---------------------------------------------------------------------------
#  TMP/TEMP 指到工作区内（**不是洁癖，是让 debug 能起来**）
# ---------------------------------------------------------------------------
#  受限会话（AI 助手的文件沙箱）会把"**映像在项目里**的进程"连同它的子孙一起
#  关在项目目录里：实测 `storage\ai\mcp\python\python.exe` 写 `%TEMP%` / `%APPDATA%`
#  一律 `PermissionError`，写项目内 OK —— 连任务计划程序起的都一样（与"谁起的"无关）。
#
#  而 `build\windows\...\Debug\viewer.exe` 正是这种进程，于是：
#    · App 里的 **VM service** 会在 `%TEMP%` 建 DevFS 临时目录 —— 建不出来，
#      `flutter run` 当场崩，热重载根本起不来（2026-09-27 现场）：
#        _createDevFS: PathAccessException: Creation of temporary directory failed,
#        path = 'C:\Users\...\AppData\Local\Temp' (OS Error: 拒绝访问。, errno = 5)
#    · App 自己写 `%APPDATA%\com.example\viewer\shared_preferences.json` 也失败
#      （那条只能打印警告，改这个环境变量救不了它 —— path_provider 走的是
#       SHGetKnownFolderPath，不认 `APPDATA`）。
#
#  App 继承本进程的环境变量，所以在这里把 TMP/TEMP 换掉就够了。
#  与 docs\pitfalls.md 里"跑 cargo test 前先把 TMP/TEMP 指到工作区"是同一个配方。
$TempDir = Join-Path $ProjectRoot '.run\tmp'
New-Item -ItemType Directory -Force -Path $TempDir | Out-Null
$env:TMP = $TempDir
$env:TEMP = $TempDir
Write-Host "==> TMP/TEMP -> $TempDir（受限会话里 App 写不了系统临时目录）" -ForegroundColor DarkGray

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
