param([int]$DesktopPid, [string]$EvidenceDirectory)
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
  throw 'Requires disposable GitHub CI'
}
# Read only this test's app and descendants. Never stop processes by name.
$processes = @(Get-CimInstance Win32_Process)
$owned = [System.Collections.Generic.HashSet[int]]::new()
[void]$owned.Add($DesktopPid)
do {
  $added = $false
  foreach ($item in $processes) {
    if ($owned.Contains([int]$item.ParentProcessId) -and $owned.Add([int]$item.ProcessId)) { $added = $true }
  }
} while ($added)
$processes | Where-Object { $owned.Contains([int]$_.ProcessId) } |
  Select-Object Name, ProcessId, ParentProcessId, CommandLine |
  ConvertTo-Json -Depth 3 | Set-Content (Join-Path $EvidenceDirectory 'windows-processes.json')
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$bounds = [System.Windows.Forms.SystemInformation]::VirtualScreen
$bitmap = [System.Drawing.Bitmap]::new($bounds.Width, $bounds.Height)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
try {
  $graphics.CopyFromScreen($bounds.Left, $bounds.Top, 0, 0, $bounds.Size)
  $bitmap.Save((Join-Path $EvidenceDirectory 'windows-desktop.png'), [System.Drawing.Imaging.ImageFormat]::Png)
} finally {
  $graphics.Dispose()
  $bitmap.Dispose()
}
