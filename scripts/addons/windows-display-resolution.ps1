# CI-only: give the Windows native UI run the same room as Linux. The hosted
# runner's display is 1024x768, which caps the app's 1280x800 window at about
# 1028x779; Linux runs under xvfb-run's 1280x1024 screen and gets the full
# window. Layout-sensitive gates (a split chat beside the right panel) should
# not depend on which OS ran them.
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
  throw 'Changing the display resolution requires a disposable GitHub-hosted runner'
}
$width = 1920
$height = 1080

if (Get-Command Set-DisplayResolution -ErrorAction SilentlyContinue) {
  Set-DisplayResolution -Width $width -Height $height -Force
} else {
  # Not every Windows Server image ships the cmdlet; the Win32 call is the same
  # change underneath.
  Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class CodemuxDisplay {
  [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Ansi)]
  public struct DEVMODE {
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmDeviceName;
    public short dmSpecVersion, dmDriverVersion, dmSize, dmDriverExtra;
    public int dmFields, dmPositionX, dmPositionY, dmDisplayOrientation, dmDisplayFixedOutput;
    public short dmColor, dmDuplex, dmYResolution, dmTTOption, dmCollate;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmFormName;
    public short dmLogPixels;
    public int dmBitsPerPel, dmPelsWidth, dmPelsHeight, dmDisplayFlags, dmDisplayFrequency;
    public int dmICMMethod, dmICMIntent, dmMediaType, dmDitherType, dmReserved1, dmReserved2;
    public int dmPanningWidth, dmPanningHeight;
  }
  [DllImport("user32.dll")] public static extern bool EnumDisplaySettings(string device, int mode, ref DEVMODE devMode);
  [DllImport("user32.dll")] public static extern int ChangeDisplaySettings(ref DEVMODE devMode, int flags);
}
'@
  $mode = New-Object CodemuxDisplay+DEVMODE
  $mode.dmSize = [Runtime.InteropServices.Marshal]::SizeOf($mode)
  if (-not [CodemuxDisplay]::EnumDisplaySettings($null, -1, [ref]$mode)) {
    throw 'Could not read the current display mode'
  }
  $mode.dmPelsWidth = $width
  $mode.dmPelsHeight = $height
  $mode.dmFields = 0x80000 -bor 0x100000 # DM_PELSWIDTH | DM_PELSHEIGHT
  $result = [CodemuxDisplay]::ChangeDisplaySettings([ref]$mode, 0)
  if ($result -ne 0) { throw "ChangeDisplaySettings failed with $result" }
}

Add-Type -AssemblyName System.Windows.Forms
$bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
Write-Host "Display: $($bounds.Width)x$($bounds.Height)"
if ($bounds.Width -lt 1280 -or $bounds.Height -lt 800) {
  throw "Display is still smaller than the app's 1280x800 window"
}
