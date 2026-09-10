# TrueMate Windows 签名构建（自动更新产物 + .sig）。
#
# 用法（仓库根 backend/cmx-agent/ 下）：
#   powershell -ExecutionPolicy Bypass -File scripts\build-signed-windows.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\build-signed-windows.ps1 -KeyPassword <密码>
#   powershell -ExecutionPolicy Bypass -File scripts\build-signed-windows.ps1 -SkipClean   # 跳过 clean（更快，仅限未改 UI 真源时）
#
# 私钥位置（gitignore 已挡）：crates/cmx-agent-shell/src-tauri/.tauri/cmx-agent.key
# 产物：crates/cmx-agent-shell/src-tauri/target/release/bundle/nsis/TrueMate_x.y.z_x64-setup.exe + 同名 .sig
#       （exe + .sig 一对，一起上传门户维护页面登记发布）

param(
    [string]$KeyPassword = $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD,
    [switch]$SkipClean
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$srcTauri = Join-Path $repoRoot "crates\cmx-agent-shell\src-tauri"
$keyPath = Join-Path $srcTauri ".tauri\cmx-agent.key"

if (-not (Test-Path $keyPath)) {
    Write-Error "找不到私钥：$keyPath —— 把 cmx-agent.key 放到该位置后重试（勿入 git，.gitignore 已挡）"
}
if (-not $KeyPassword) {
    $KeyPassword = Read-Host "输入私钥密码"
}

Set-Location $srcTauri

# release 构建默认 clean：防增量编译不重嵌 UI 真源（sync-ui.sh 改过 UI 后必须 clean）。
if (-not $SkipClean) {
    cargo clean -p cmx-agent-shell
}

# 内容型 env 全版本通吃（npx 拉的 CLI 可能不认 PATH 型）；PATH 型一并设置兼容新版。
$env:TAURI_SIGNING_PRIVATE_KEY = Get-Content $keyPath -Raw
$env:TAURI_SIGNING_PRIVATE_KEY_PATH = $keyPath
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $KeyPassword

npx -y @tauri-apps/cli build
if ($LASTEXITCODE -ne 0) {
    Write-Error "tauri build 失败（exit $LASTEXITCODE）"
}

# 防漏签自检：NSIS 产物旁必须产出同名 .sig（缺了说明签名步没跑，勿分发）。
$sigs = Get-ChildItem "target\release\bundle\nsis\*.sig" -ErrorAction SilentlyContinue
if (-not $sigs) {
    Write-Error "未找到 .sig —— 更新产物没有签名，禁止分发；检查私钥/密码后重跑"
}

Write-Host ""
Write-Host "构建完成（exe + .sig 成对，一起上传门户维护页面登记发布）：" -ForegroundColor Green
Get-ChildItem "target\release\bundle\nsis\*" | Where-Object { $_.Extension -in ".exe", ".sig" } |
    ForEach-Object { Write-Host ("  " + $_.FullName) }
