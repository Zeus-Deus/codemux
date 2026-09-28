param([int]$DesktopPid)
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
  throw 'Requires disposable GitHub CI'
}
# Read only: which top-level window is in front, and whether it belongs to this
# test's own app. Nothing is focused, moved or stopped.
Add-Type -Namespace CodeMuxCi -Name Foreground -MemberDefinition @'
[DllImport("user32.dll")]
public static extern IntPtr GetForegroundWindow();
[DllImport("user32.dll")]
public static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
[DllImport("user32.dll", CharSet = CharSet.Unicode)]
public static extern int GetWindowText(IntPtr window, System.Text.StringBuilder text, int count);
'@
$window = [CodeMuxCi.Foreground]::GetForegroundWindow()
$processId = [uint32]0
$title = [System.Text.StringBuilder]::new(256)
if ($window -ne [IntPtr]::Zero) {
  [void][CodeMuxCi.Foreground]::GetWindowThreadProcessId($window, [ref]$processId)
  [void][CodeMuxCi.Foreground]::GetWindowText($window, $title, $title.Capacity)
}
$process = if ($processId) { Get-Process -Id $processId -ErrorAction SilentlyContinue }
[pscustomobject]@{
  window = $window -ne [IntPtr]::Zero
  processName = if ($process) { $process.ProcessName } else { $null }
  title = $title.ToString()
  ownApp = $processId -eq $DesktopPid
} | ConvertTo-Json -Compress
