# CI-only: use the driver matching the installed WebView2 runtime, not Edge's
# independently updated browser version. Download only from Microsoft.
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
  throw 'Native UI acceptance requires a disposable GitHub-hosted runner'
}
$client = 'Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
$version = $null
foreach ($prefix in @('HKLM:\SOFTWARE\WOW6432Node\', 'HKLM:\SOFTWARE\', 'HKCU:\SOFTWARE\WOW6432Node\', 'HKCU:\SOFTWARE\')) {
  $entry = Get-ItemProperty -Path "$prefix$client" -ErrorAction SilentlyContinue
  if ($entry.pv -match '^\d+\.\d+\.\d+\.\d+$') { $version = $entry.pv; break }
}
if (-not $version) { throw 'No installed WebView2 runtime version found' }
$destination = Join-Path $env:RUNNER_TEMP 'codemux-webdriver'
New-Item -ItemType Directory -Path $destination -Force | Out-Null
$archive = Join-Path $destination 'driver.zip'
Invoke-WebRequest "https://msedgedriver.microsoft.com/$version/edgedriver_win64.zip" -OutFile $archive
Expand-Archive $archive -DestinationPath $destination -Force
$driver = Join-Path $destination 'msedgedriver.exe'
$signature = Get-AuthenticodeSignature $driver
if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'Microsoft Corporation') {
  throw 'Microsoft Edge driver signature verification failed'
}
& $driver --version
if ($LASTEXITCODE -ne 0) { throw 'Microsoft Edge driver did not start' }
$destination | Out-File -FilePath $env:GITHUB_PATH -Encoding utf8 -Append
