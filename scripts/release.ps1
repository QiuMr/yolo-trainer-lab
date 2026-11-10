# 发版：一条命令完成「校验 -> 构建 -> 签名 -> 打 tag -> 上传 -> 生成 latest.json -> 核对资产」。
#
# 为什么必须有脚本：手工发版最容易漏的两处都不会立刻报错，只会让所有用户的自动更新
# 静默失效 ——
#   1. latest.json 里的 signature 还是上一个包的；
#   2. 上传了大文件但中断，Release 建成却缺安装包。
# 这两类错误要等用户点「下载并安装」才会暴露，届时已经很难定位。
#
# 用法：
#   powershell -File scripts/release.ps1 -Notes "修了分类数据集扫描"
#   powershell -File scripts/release.ps1 -SkipBuild -Draft   # 只重传，不重编
#
# 前置：gh 已登录；工作区已提交；私钥在 %USERPROFILE%\.tauri\yolo-trainer.key
#       （或用 TAURI_SIGNING_PRIVATE_KEY 指向别处）；externalBin 的 montgomery.exe 已就位。

param(
  [string]$Notes = "",
  [string]$NotesFile = "",
  [switch]$SkipBuild,
  [switch]$Draft
)

# 刻意不用 "Stop"：npm / vite / cargo / gh 都把正常进度写进 stderr，
# PowerShell 5.1 会把它当 NativeCommandError，配 Stop 会在构建其实成功时中止。
$ErrorActionPreference = "Continue"

$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$desktop = Join-Path $root "desktop"
$bundleDir = Join-Path $root "artifacts\desktop-target\release\bundle\nsis"
$releaseDir = Join-Path $root "release"
$startedAt = Get-Date

function Fail([string]$msg) { Write-Host "[FAIL] $msg" -ForegroundColor Red; exit 1 }
function Info([string]$msg) { Write-Host "[INFO] $msg" -ForegroundColor Cyan }
function Ok([string]$msg) { Write-Host "[OK]   $msg" -ForegroundColor Green }

# 跑一个原生 exe 并返回 @{ Code; Out }。
# 必须包这一层：git / gh 的进度与提示都写 stderr，且 $ErrorActionPreference 下直接取输出
# 在某些会话会拿到空字符串、退出码也读空，容易被误判成「没有 origin 远程」或「上传失败」。
# 统一走 .NET Process 显式重定向两条流，取真实 ExitCode。
function Invoke-Native([string]$exePath, [string[]]$cmdArgs, [string]$workDir = $root) {
  $psi = New-Object System.Diagnostics.ProcessStartInfo
  $psi.FileName = $exePath
  $psi.Arguments = (($cmdArgs | ForEach-Object {
    if ($_ -match '[\s"]') { '"' + ($_ -replace '"', '\"') + '"' } else { $_ }
  }) -join ' ')
  $psi.WorkingDirectory = $workDir
  $psi.UseShellExecute = $false
  $psi.RedirectStandardOutput = $true
  $psi.RedirectStandardError = $true
  $psi.StandardOutputEncoding = [System.Text.Encoding]::UTF8
  $psi.StandardErrorEncoding = [System.Text.Encoding]::UTF8
  $psi.CreateNoWindow = $true
  $p = [System.Diagnostics.Process]::Start($psi)
  $out = $p.StandardOutput.ReadToEnd()
  $err = $p.StandardError.ReadToEnd()
  $p.WaitForExit()
  return @{ Code = $p.ExitCode; Out = (($out + "`n" + $err).Trim()) }
}

$gitExe = (Get-Command git -ErrorAction SilentlyContinue).Source
if (-not $gitExe) {
  foreach ($c in @("D:\Git\cmd\git.exe", "C:\Program Files\Git\cmd\git.exe")) {
    if (Test-Path $c) { $gitExe = $c; break }
  }
}
if (-not $gitExe) { Fail "找不到 git。装一个，或把它加进 PATH" }
function Invoke-Git([string[]]$gitArgs) { Invoke-Native $gitExe $gitArgs }

$gh = (Get-Command gh -ErrorAction SilentlyContinue).Source
if (-not $gh) { Fail "找不到 gh 命令。先装 GitHub CLI 并 gh auth login" }
function Invoke-Gh([string[]]$ghArgs) { Invoke-Native $gh $ghArgs }

# ── 1. 版本号：唯一来源 tauri.conf.json ────────────────────────────────────
# build.rs 把同一个值注入 Rust 的 YOLO_TRAINER_VERSION，Tauri CLI 用它命名安装包。
# 三处漂开会导致「程序显示 0.1.0，Release 却是 v0.2.0」，用户报版本号时两边对不上。
$confPath = Join-Path $desktop "src-tauri\tauri.conf.json"
$confRaw = [System.IO.File]::ReadAllText($confPath, [System.Text.UTF8Encoding]::new($false))
if ($confRaw -notmatch '"version"\s*:\s*"([^"]+)"') { Fail "tauri.conf.json 里找不到顶层 version" }
$ver = $Matches[1]
$tag = "v$ver"
Info "版本 $ver（tag $tag）"

# ── 2. 仓库 ───────────────────────────────────────────────────────────────
$remoteRes = Invoke-Git @("remote", "get-url", "origin")
if ($remoteRes.Code -ne 0 -or -not $remoteRes.Out) { Fail "没有 origin 远程，先 git remote add origin <url>" }
$remote = ($remoteRes.Out -split "`n")[0].Trim()
if ($remote -match 'github\.com[:/]+([^/]+)/(.+?)(?:\.git)?$') {
  $slug = "$($Matches[1])/$($Matches[2])"
} else {
  Fail "无法从 origin 解析 GitHub 仓库：$remote"
}
Info "仓库 $slug"

# ── 3. 工作区必须干净 ─────────────────────────────────────────────────────
# 带着未提交改动发版 = 发布一个源码里查不到对应版本的二进制。
$dirty = (Invoke-Git @("status", "--porcelain")).Out
if ($dirty) { Write-Host $dirty; Fail "工作区有未提交改动，先提交再发版" }

# ── 4. tag 不能重复 ───────────────────────────────────────────────────────
if ((Invoke-Git @("tag", "--list", $tag)).Out) { Fail "$tag 已存在。发新版请先改 tauri.conf.json 的 version" }

# ── 5. 前置依赖 ───────────────────────────────────────────────────────────
$keyPath = $env:TAURI_SIGNING_PRIVATE_KEY
if (-not $keyPath) { $keyPath = Join-Path $env:USERPROFILE ".tauri\yolo-trainer.key" }
if (-not (Test-Path $keyPath)) {
  Fail "找不到签名私钥：$keyPath`n  生成：npx tauri signer generate --ci --password '' -w <路径>"
}
Info "签名私钥：$keyPath"

$external = @(Get-ChildItem (Join-Path $desktop "src-tauri\binaries") -Filter "montgomery-*.exe" -ErrorAction SilentlyContinue)
if ($external.Count -eq 0) {
  Fail "externalBin 里没有 montgomery-<triple>.exe。见 README.md 的「构建前置」"
}
$auth = Invoke-Gh @("auth", "status")
if ($auth.Code -ne 0) { Fail "gh 未登录：`n$($auth.Out)" }

# ── 6. 构建 ───────────────────────────────────────────────────────────────
if (-not $SkipBuild) {
  # vite 清 dist 时会被 safe-delete 阈值拦住（文件数 > 50），打包前用 .NET 直删。
  # 这些是构建中间产物，进回收站没有意义。
  $dist = Join-Path $desktop "dist"
  if (Test-Path $dist) { [System.IO.Directory]::Delete($dist, $true) }

  $cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
  if (Test-Path $cargoBin) { $env:PATH = "$cargoBin;$env:PATH" }
  $env:CARGO_TARGET_DIR = Join-Path $root "artifacts\desktop-target"
  $env:TAURI_SIGNING_PRIVATE_KEY = $keyPath
  $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""

  $npx = (Get-Command npx -ErrorAction SilentlyContinue).Source
  if (-not $npx) { Fail "PATH 里没有 npx，无法运行 tauri build" }
  Info "构建中（npx tauri build）..."
  Push-Location $desktop
  try { & $npx tauri build } finally { Pop-Location }
}

# 不依赖构建命令的退出码：本机会把它读成空值。直接核对三个产物是否齐全且是本次生成的。
# NSIS 目标下**没有 .nsis.zip**：Tauri 2 直接对 setup.exe 签名，产物是 <setup>.exe.sig。
# 插件从 latest.json 的 signature 字段读签名，所以 .sig 本身不需要上传成 Release 资产。
$setup = Join-Path $bundleDir "YOLO Trainer_${ver}_x64-setup.exe"
$sig = "$setup.sig"
foreach ($p in @($setup, $sig)) {
  if (-not (Test-Path $p)) { Fail "产物不存在：$p（构建失败，或 createUpdaterArtifacts 没生效）" }
  if (-not $SkipBuild -and (Get-Item $p).LastWriteTime -lt $startedAt) {
    Fail "产物是上一次构建的：$p。构建没有真正跑完，拒绝发版"
  }
}
Ok ("安装包 {0:N1} MiB" -f ((Get-Item $setup).Length / 1MB))
Ok ("签名 {0} 字符" -f ((Get-Item $sig).Length))

# ── 7. 上传副本 + latest.json ─────────────────────────────────────────────
# gh 会把资产名里的**空格替换成点**："YOLO Trainer_..." -> "YOLO.Trainer_..."。
# 实测后果有两个：latest.json 里 %20 编码的 URL 会 404；按本地文件名核对资产永远不匹配。
# 所以上传前复制成不含空格的名字，URL、资产核对、补传统一用它。文件内容不变，签名仍有效。
if (-not (Test-Path $releaseDir)) { New-Item -ItemType Directory -Path $releaseDir -Force | Out-Null }
$uploadName = "YOLO-Trainer_${ver}_x64-setup.exe"
$uploadExe = Join-Path $releaseDir $uploadName
Copy-Item $setup $uploadExe -Force
Info "上传副本：$uploadName"

# URL 指向**本版本**的 Release 资产，而不是 latest/download ——
# latest.json 自己由 latest/download 指过来，再指回去会绕圈。
$assetUrl = "https://github.com/$slug/releases/download/$tag/$uploadName"
$signature = ([System.IO.File]::ReadAllText($sig, [System.Text.UTF8Encoding]::new($false))).Trim()
if (-not $signature) { Fail "$sig 是空的，签名没有生成" }

if ($NotesFile) {
  if (-not (Test-Path $NotesFile)) { Fail "说明文件不存在：$NotesFile" }
  $Notes = [System.IO.File]::ReadAllText($NotesFile, [System.Text.UTF8Encoding]::new($false))
}
if (-not $Notes) { $Notes = (Invoke-Git @("log", "-1", "--pretty=format:%s")).Out }
if (-not $Notes) { $Notes = "发布 $tag" }

$latestPath = Join-Path $releaseDir "latest.json"
$latest = [ordered]@{
  version  = $ver
  notes    = $Notes
  pub_date = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
  # 插件依次尝试 windows-x86_64-nsis 与 windows-x86_64；NSIS 是唯一目标，用通用键即可。
  platforms = @{
    "windows-x86_64" = @{ url = $assetUrl; signature = $signature }
  }
}
$jsonText = ($latest | ConvertTo-Json -Depth 6)
# 无 BOM：serde_json 遇到 BOM 直接解析失败，而每次启动都要读这个文件。
[System.IO.File]::WriteAllText($latestPath, $jsonText, [System.Text.UTF8Encoding]::new($false))
Ok "已生成 release/latest.json"

# ── 8. push + 建 Release ─────────────────────────────────────────────────
Info "推送 main..."
$push = Invoke-Git @("push", "origin", "HEAD")
if ($push.Code -ne 0) { Fail "推送失败：$($push.Out)" }

Info "打 tag $tag..."
$tagged = Invoke-Git @("tag", "-a", $tag, "-m", "release $tag")
if ($tagged.Code -ne 0) { Fail "打 tag 失败：$($tagged.Out)" }
$tagPush = Invoke-Git @("push", "origin", $tag)
if ($tagPush.Code -ne 0) { Fail "推送 tag 失败：$($tagPush.Out)" }

$ghArgs = @(
  "release", "create", $tag,
  $uploadExe, $latestPath,
  "--repo", $slug,
  "--title", "YOLO Trainer $tag",
  "--notes", $Notes
)
if ($Draft) { $ghArgs += "--draft" }
Info "创建 Release..."
& $gh @ghArgs | Out-Host

# ── 9. 核对资产（判据，不用退出码）───────────────────────────────────────
# gh 上传大文件中断时会留下「只有 latest.json、没有安装包」的空壳 Release。
# 直接问 GitHub 资产全不全，比读 $LASTEXITCODE 可靠。
function Get-AssetNames {
  $v = Invoke-Gh @("release", "view", $tag, "--repo", $slug, "--json", "assets")
  if ($v.Code -ne 0) { return $null }
  # Trim: `Invoke-Native` merges stdout+stderr, and a trailing CR on a name would make the
  # `-notcontains` comparison below silently miss an asset that is actually there.
  try { return @(($v.Out | ConvertFrom-Json).assets | ForEach-Object { ($_.name).Trim() }) } catch { return $null }
}
$expected = @($uploadName, "latest.json")
$names = Get-AssetNames
if ($null -eq $names) { Fail "读不到 Release 资产列表，请手动检查 https://github.com/$slug/releases/tag/$tag" }
$missing = @($expected | Where-Object { $names -notcontains $_ })
if ($missing.Count -gt 0) {
  Write-Host "[WARN] 缺资产: $($missing -join ', ') —— 多半是上传中断，补传中" -ForegroundColor Yellow
  $retry = @("release", "upload", $tag)
  foreach ($m in $missing) {
    if ($m -eq "latest.json") { $retry += $latestPath }
    else { $retry += $uploadExe }
  }
  $retry += @("--clobber", "--repo", $slug)
  Invoke-Gh $retry | Out-Null
  $names = Get-AssetNames
  $missing = @($expected | Where-Object { $names -notcontains $_ })
  if ($missing.Count -gt 0) { Fail "补传后仍缺: $($missing -join ', ')。到 Releases 页面手动上传" }
  Ok "补传完成"
}
Ok "资产齐全：$($names -join ', ')"

Write-Host ""
Ok "发布完成：https://github.com/$slug/releases/tag/$tag"
if ($Draft) {
  Info "这是草稿：到 Releases 页面点 Publish，latest 指针才会切过去"
} else {
  Info "更新源：https://github.com/$slug/releases/latest/download/latest.json"
  Write-Host ""
  Write-Host "  自检（建议）:" -ForegroundColor Yellow
  Write-Host "    1. 浏览器打开上面的 latest.json，确认 version / signature 是本次的"
  Write-Host "    2. 用旧版本打开设置页，点「检查更新」，确认能看到 $tag"
}
