# 把引擎构建产物同步到应用实际使用的两处副本。
#
# 为什么需要它：改完 research/montgomery 只构建 artifacts/target-dist 是不够的，应用跑的是
# 另外两个位置，而 tauri 不会因为源文件变了就覆盖它们（它按时间戳判断，而 Copy-Item 默认保留
# 源文件的旧时间戳）。实测为此多花了一轮排查：补丁明明生效了，界面里却还是旧行为，
# 引擎以"进程退出，代码 1"收场。
#
# 三处的关系：
#   artifacts/target-dist/release/montgomery.exe         ← cargo 构建产物（唯一真相）
#   desktop/src-tauri/binaries/montgomery-<triple>.exe   ← tauri 的 externalBin 源
#   artifacts/desktop-target/release/montgomery.exe      ← tauri build 之后应用实际启动的那个
#
# 用法（改完引擎、跑过 cargo build 之后）：
#   powershell -File scripts/sync-engine.ps1
#   powershell -File scripts/sync-engine.ps1 -SkipAppTarget    # 只更新 externalBin

param(
  [string]$Built = "artifacts\target-dist\release\montgomery.exe",
  [switch]$SkipAppTarget
)

$ErrorActionPreference = "Continue"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$src = Join-Path $root $Built

if (-not (Test-Path $src)) {
  Write-Host "[FAIL] 找不到构建产物：$src" -ForegroundColor Red
  Write-Host "       先跑：cd research/montgomery && cargo build --release --features training,onnx" -ForegroundColor Yellow
  exit 1
}

$targets = @(Join-Path $root "desktop\src-tauri\binaries\montgomery-x86_64-pc-windows-msvc.exe")
if (-not $SkipAppTarget) {
  $targets += Join-Path $root "artifacts\desktop-target\release\montgomery.exe"
}

# 统一盖上当前时间：时间戳本身就是"是否最新"的判据，保留旧时间会让下一道检查误判为过期。
$stamp = Get-Date
$size = (Get-Item $src).Length
foreach ($dst in $targets) {
  $parent = Split-Path $dst -Parent
  if (-not (Test-Path $parent)) {
    Write-Host "[SKIP] 目录不存在：$parent（还没构建过桌面端）" -ForegroundColor Yellow
    continue
  }
  Copy-Item $src $dst -Force
  (Get-Item $dst).LastWriteTime = $stamp
  Write-Host "[OK]   $dst" -ForegroundColor Green
}
(Get-Item $src).LastWriteTime = $stamp
Write-Host ("[DONE] 引擎 {0:N0} 字节，已同步到 {1} 处" -f $size, $targets.Count) -ForegroundColor Cyan
