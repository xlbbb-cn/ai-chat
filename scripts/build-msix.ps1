# 把 `tauri build --no-bundle` 产出的 rabbit-chat-studio.exe 打成一个 MSIX 包。
#
#   npm run msix                        # 默认：构建 exe + 打包（无需签名，Store 会用微软证书重签）
#   npm run msix -- -SkipBuild           # 复用已有的 target\release\rabbit-chat-studio.exe，只打包
#   npm run msix -- -SkipBuild -DevCert  # 打包并用开发证书签名，用于本地侧载测试
#   npm run msix -- -SkipBuild -Sign -CertPath .\code.pfx -CertPassword ***   # 正式证书签名
#
# 产物：src-tauri\target\msix\msix\ 下的 .msix（提交到 Partner Center 的就是这个文件）。
[CmdletBinding()]
param(
  # MSIX 版本号，四段数字。默认取 src-tauri\tauri.conf.json 的版本并补成 x.y.z.0。
  [string]$Version,
  [ValidateSet('x64', 'arm64', 'x86')][string]$Arch = 'x64',
  [string]$OutDir,
  [switch]$SkipBuild,
  [switch]$Sign,
  [string]$CertPath,
  [string]$CertPassword,
  # 生成/复用一个自签开发证书（subject 必须等于 manifest 的 Publisher）并签名。
  [switch]$DevCert
)

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$msixSource = Join-Path $root 'src-tauri\msix'
$configPath = Join-Path $msixSource 'store.config.json'
$templatePath = Join-Path $msixSource 'Package.appxmanifest.template'
$iconsDir = Join-Path $root 'src-tauri\icons'
$tauriConfPath = Join-Path $root 'src-tauri\tauri.conf.json'

function Write-Step($message) {
  Write-Host "==> $message" -ForegroundColor Cyan
}

function Resolve-SdkTool([string]$name) {
  $kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
  if (-not (Test-Path $kits)) {
    throw "找不到 Windows SDK（$kits）。安装「Windows SDK」后再打包。"
  }
  $versions = Get-ChildItem $kits -Directory |
    Where-Object { $_.Name -match '^10\.\d+\.\d+\.\d+$' } |
    Sort-Object { [version]$_.Name } -Descending
  foreach ($dir in $versions) {
    $candidate = Join-Path $dir.FullName "$Arch\$name"
    if (Test-Path $candidate) { return $candidate }
  }
  throw "Windows SDK 里找不到 $name（架构 $Arch）。"
}

# 3.1.1 -> 3.1.1.0；带预发布后缀时只保留数字段，MSIX 版本必须是四段纯数字。
function ConvertTo-MsixVersion([string]$raw) {
  $clean = $raw.Trim().TrimStart('v', 'V')
  if ($clean -match '^(\d+)\.(\d+)\.(\d+)(?:\.(\d+))?') {
    $parts = @([int]$Matches[1], [int]$Matches[2], [int]$Matches[3], $(if ($Matches[4]) { [int]$Matches[4] } else { 0 }))
    foreach ($p in $parts) {
      if ($p -lt 0 -or $p -gt 65535) { throw "版本号 $raw 超出 MSIX 允许范围（每段 0-65535）。" }
    }
    return ($parts -join '.')
  }
  throw "无法把版本号 '$raw' 转成 MSIX 的四段数字版本。"
}

if (-not $Version) {
  $tauriConf = Get-Content $tauriConfPath -Raw | ConvertFrom-Json
  $Version = $tauriConf.version
}
$msixVersion = ConvertTo-MsixVersion $Version

$config = Get-Content $configPath -Raw | ConvertFrom-Json
foreach ($key in 'packageName', 'publisher', 'publisherDisplayName', 'displayName', 'description', 'executable') {
  if (-not $config.$key) { throw "src-tauri\msix\store.config.json 缺少字段 $key。" }
}
if ($config.publisher -match '^CN=0{8}-') {
  Write-Host '警告：store.config.json 里的 publisher 还是占位符。可以打包并本地侧载，' -ForegroundColor Yellow
  Write-Host '      但上传到 Partner Center 前必须换成「产品身份」里 CN=… 的真实值。' -ForegroundColor Yellow
}

$makeappx = Resolve-SdkTool 'makeappx.exe'
$signtool = if ($Sign -or $DevCert) { Resolve-SdkTool 'signtool.exe' } else { $null }

$targetRoot = Join-Path $root "src-tauri\target\msix"
if (-not $OutDir) { $OutDir = Join-Path $targetRoot 'msix' }
$stage = Join-Path $targetRoot "stage-$Arch"
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null

if (-not $SkipBuild) {
  Write-Step "构建 exe（tauri build --no-bundle，架构 $Arch）"
  Push-Location $root
  try {
    if ($Arch -eq 'x64') {
      npm run tauri -- build --no-bundle
    } else {
      npm run tauri -- build --no-bundle --target "$Arch-pc-windows-msvc"
    }
    if ($LASTEXITCODE -ne 0) { throw "tauri build 失败（退出码 $LASTEXITCODE）。" }
  } finally {
    Pop-Location
  }
}

$exeDir = if ($Arch -eq 'x64') {
  Join-Path $root 'src-tauri\target\release'
} else {
  Join-Path $root "src-tauri\target\$Arch-pc-windows-msvc\release"
}
$exePath = Join-Path $exeDir $config.executable
if (-not (Test-Path $exePath)) {
  throw "找不到 $exePath。先跑 `npm run tauri -- build --no-bundle`，或去掉 -SkipBuild。"
}

Write-Step "准备打包目录 $stage"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Path (Join-Path $stage 'Assets') -Force | Out-Null
Copy-Item $exePath (Join-Path $stage $config.executable)

# MSIX 要求的图标尺寸。Tauri 的 icon 生成器已经产出大部分，宽磁贴（310x150）没有，用 310x310 居中生成。
$assets = @{
  'StoreLogo.png'        = 'StoreLogo.png'
  'Square44x44Logo.png'  = 'Square44x44Logo.png'
  'Square71x71Logo.png'  = 'Square71x71Logo.png'
  'Square150x150Logo.png' = 'Square150x150Logo.png'
  'Square310x310Logo.png' = 'Square310x310Logo.png'
}
foreach ($name in $assets.Keys) {
  $source = Join-Path $iconsDir $assets[$name]
  if (-not (Test-Path $source)) { throw "缺少图标 $source（用 `npm run tauri icon <png>` 生成一整套）。" }
  Copy-Item $source (Join-Path $stage "Assets\$name")
}

Add-Type -AssemblyName System.Drawing
$tileSource = [System.Drawing.Image]::FromFile((Join-Path $iconsDir 'Square310x310Logo.png'))
try {
  $wide = New-Object System.Drawing.Bitmap 310, 150
  try {
    $graphics = [System.Drawing.Graphics]::FromImage($wide)
    try {
      $graphics.Clear([System.Drawing.Color]::Transparent)
      $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
      $graphics.DrawImage($tileSource, [int]((310 - 150) / 2), 0, 150, 150)
    } finally {
      $graphics.Dispose()
    }
    $wide.Save((Join-Path $stage 'Assets\Wide310x150Logo.png'), [System.Drawing.Imaging.ImageFormat]::Png)
  } finally {
    $wide.Dispose()
  }
} finally {
  $tileSource.Dispose()
}

Write-Step "生成 Package.appxmanifest（$msixVersion / $Arch）"
$manifest = Get-Content $templatePath -Raw
$tokens = @{
  '{{PACKAGE_NAME}}'              = $config.packageName
  '{{PUBLISHER}}'                 = $config.publisher
  '{{PUBLISHER_DISPLAY_NAME}}'    = $config.publisherDisplayName
  '{{DISPLAY_NAME}}'              = $config.displayName
  '{{DESCRIPTION}}'               = $config.description
  '{{VERSION}}'                   = $msixVersion
  '{{ARCH}}'                      = $Arch
  '{{EXECUTABLE}}'                = $config.executable
  '{{MIN_WINDOWS_VERSION}}'       = $config.minWindowsVersion
  '{{MAX_WINDOWS_VERSION_TESTED}}' = $config.maxWindowsVersionTested
}
foreach ($token in $tokens.Keys) {
  $manifest = $manifest.Replace($token, $tokens[$token])
}
if ($manifest -match '\{\{[A-Z_]+\}\}') {
  throw "manifest 里还有没替换的占位符：$($Matches[0])"
}
# UTF-8 无 BOM 写盘：带 BOM 的 manifest 会被 makeappx 拒收。
[System.IO.File]::WriteAllText((Join-Path $stage 'AppxManifest.xml'), $manifest, (New-Object System.Text.UTF8Encoding $false))

Write-Step 'makeappx pack'
$packageName = "{0}_{1}_{2}.msix" -f $config.packageName, $msixVersion, $Arch
$msixPath = Join-Path $OutDir $packageName
& $makeappx pack /d $stage /p $msixPath /o
if ($LASTEXITCODE -ne 0) { throw "makeappx 失败（退出码 $LASTEXITCODE）。" }

if ($DevCert) {
  $pfxPath = Join-Path $msixSource 'devcert.pfx'
  $pfxPassword = 'rabbit-chat-dev'
  if (-not (Test-Path $pfxPath)) {
    Write-Step "生成自签开发证书（subject = $($config.publisher)）"
    $cert = New-SelfSignedCertificate -Type Custom -Subject $config.publisher `
      -KeyUsage DigitalSignature -FriendlyName 'Rabbit Chat Studio MSIX dev' `
      -CertStoreLocation 'Cert:\CurrentUser\My' `
      -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={text}')
    $secure = ConvertTo-SecureString -String $pfxPassword -Force -AsPlainText
    Export-PfxCertificate -Cert $cert -FilePath $pfxPath -Password $secure | Out-Null
  }
  $CertPath = $pfxPath
  $CertPassword = $pfxPassword
  $Sign = $true
}

if ($Sign) {
  if (-not $CertPath) { throw '-Sign 需要 -CertPath（或改用 -DevCert 走自签开发证书）。' }
  Write-Step "signtool 签名（$CertPath）"
  $signArgs = @('sign', '/fd', 'SHA256', '/f', $CertPath)
  if ($CertPassword) { $signArgs += @('/p', $CertPassword) }
  $signArgs += $msixPath
  & $signtool @signArgs
  if ($LASTEXITCODE -ne 0) { throw "signtool 失败（退出码 $LASTEXITCODE）。" }
  Write-Host "提示：用自签证书本地侧载前，需要把证书导入「受信任的根证书颁发机构」和「受信任的发布者」：" -ForegroundColor Yellow
  Write-Host "      Import-PfxCertificate -FilePath '$CertPath' -CertStoreLocation Cert:\LocalMachine\Root -Password (ConvertTo-SecureString '$CertPassword' -AsPlainText -Force)"
}

$sizeMb = [math]::Round((Get-Item $msixPath).Length / 1MB, 1)
Write-Host ""
Write-Host "MSIX 已生成：$msixPath（$sizeMb MB）" -ForegroundColor Green
if (-not $Sign) {
  Write-Host '未签名 —— 提交 Partner Center 就用这个文件（Store 会用微软证书重签）。' -ForegroundColor Gray
  Write-Host '本地想装来试的话，加 -DevCert 重新打包并签名。' -ForegroundColor Gray
}
