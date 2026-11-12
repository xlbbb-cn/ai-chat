# 把应用窗口截成 Store 提交用的 PNG。Store 要求 ≥1366×768，窗口太小时只警告不拦截。
# 用法：pwsh -File scripts/capture-store-screenshots.ps1 [-DelaySeconds 5]
[CmdletBinding()]
param(
  [string]$ProcessName = 'rabbit-chat-studio',
  [string]$OutDir = (Join-Path (Split-Path -Parent $PSScriptRoot) 'docs\store\screenshots'),
  [int]$DelaySeconds = 0
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public class StoreCapture {
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
  [DllImport("shcore.dll")] public static extern int SetProcessDpiAwareness(int value);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
'@

# 高 DPI 缩放下不声明感知会截出缩放过的模糊图；已经设置过会返回错误，忽略即可。
try { [void][StoreCapture]::SetProcessDpiAwareness(2) } catch { }

$proc = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue |
  Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
if (-not $proc) { throw "没有找到窗口：$ProcessName（先启动应用，或改 -ProcessName）" }

$handle = $proc.MainWindowHandle
[void][StoreCapture]::ShowWindow($handle, 9)
[void][StoreCapture]::SetForegroundWindow($handle)
if ($DelaySeconds -gt 0) { Start-Sleep -Seconds $DelaySeconds }

$rect = New-Object StoreCapture+RECT
[void][StoreCapture]::GetWindowRect($handle, [ref]$rect)
$width = $rect.Right - $rect.Left
$height = $rect.Bottom - $rect.Top
if ($width -lt 1366 -or $height -lt 768) {
  Write-Warning "窗口 ${width}x${height} 小于 1366x768，Store 会拒收，请先放大窗口再截。"
}

New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
$path = Join-Path $OutDir ('screenshot-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '.png')
$bitmap = New-Object System.Drawing.Bitmap $width, $height
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bitmap.Size)
$bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
$graphics.Dispose()
$bitmap.Dispose()

"saved $path (${width}x${height})"
