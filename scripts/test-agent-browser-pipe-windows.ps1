param([Parameter(Mandatory)][string]$ProfilePath)
$ErrorActionPreference = 'Stop'

[Console]::Error.WriteLine('Inspecting managed Chromium process')
$chrome = @(Get-CimInstance Win32_Process | Where-Object {
  $_.CommandLine -and $_.CommandLine.Contains("--user-data-dir=$ProfilePath") -and
  $_.CommandLine.Contains('--remote-debugging-pipe') -and
  $_.CommandLine.Contains('--remote-debugging-io-pipes=')
})
if ($chrome.Count -ne 1) { throw "Expected one managed Chromium main process, found $($chrome.Count)" }
$commandLine = $chrome[0].CommandLine
if ($commandLine.Contains('--remote-debugging-port')) { throw 'Managed Chromium has a TCP debugging switch' }
if ($commandLine -notmatch '--remote-debugging-io-pipes=(\d+),(\d+)') {
  throw 'Managed Chromium has no inherited pipe handle switch'
}
$handles = @([long]$Matches[1], [long]$Matches[2])

[Console]::Error.WriteLine('Loading native handle inspector')
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class BrowserPipeHandles {
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern IntPtr GetCurrentProcess();
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool DuplicateHandle(IntPtr sourceProcess, IntPtr sourceHandle,
        IntPtr targetProcess, out IntPtr targetHandle, uint access, bool inherit, uint options);
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern uint GetFileType(IntPtr handle);
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool CloseHandle(IntPtr handle);
}
'@

$source = [BrowserPipeHandles]::OpenProcess(0x40, $false, $chrome[0].ProcessId)
if ($source -eq [IntPtr]::Zero) { throw 'Cannot inspect managed Chromium handles' }
try {
  foreach ($value in $handles) {
    $duplicate = [IntPtr]::Zero
    $ok = [BrowserPipeHandles]::DuplicateHandle($source, [IntPtr]$value,
      [BrowserPipeHandles]::GetCurrentProcess(), [ref]$duplicate, 0, $false, 2)
    if (-not $ok) { throw "Chromium pipe handle $value is closed or invalid" }
    try {
      if ([BrowserPipeHandles]::GetFileType($duplicate) -ne 3) {
        throw "Chromium handle $value is not a pipe"
      }
    } finally {
      [void][BrowserPipeHandles]::CloseHandle($duplicate)
    }
  }
} finally {
  [void][BrowserPipeHandles]::CloseHandle($source)
}

[Console]::Error.WriteLine('Inspecting managed Chromium TCP listeners')
$listeners = @(Get-NetTCPConnection -State Listen -ErrorAction Stop | Where-Object {
  $_.OwningProcess -eq $chrome[0].ProcessId
})
if ($listeners.Count -gt 0) { throw "Managed Chromium owns $($listeners.Count) TCP listener(s)" }
[Console]::WriteLine($chrome[0].ProcessId)
