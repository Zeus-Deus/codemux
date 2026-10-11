param([ValidateSet('powershell', 'cmd')][string]$Method)
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or $env:RUNNER_OS -ne 'Windows') {
  throw 'Requires a disposable Windows GitHub runner'
}
$evidence = Join-Path $PWD 'windows-claude-evidence'
New-Item -ItemType Directory -Path $evidence -Force | Out-Null
$beforePath = $env:PATH
$beforeUserPath = [Environment]::GetEnvironmentVariable('PATH', 'User')
if ($Method -eq 'powershell') {
  # Use Windows PowerShell 5.1, which a fresh Windows installation ships.
  & powershell.exe -NoProfile -ExecutionPolicy Bypass -Command 'irm https://claude.ai/install.ps1 | iex' 2>&1 |
    Tee-Object -FilePath (Join-Path $evidence 'installer.log')
} else {
  $installer = Join-Path $env:RUNNER_TEMP 'claude-install.cmd'
  & curl.exe -fsSL https://claude.ai/install.cmd -o $installer
  if ($LASTEXITCODE -ne 0) { throw 'Could not download the official CMD installer' }
  & cmd.exe /d /c "`"$installer`"" 2>&1 | Tee-Object -FilePath (Join-Path $evidence 'installer.log')
}
if ($LASTEXITCODE -ne 0) { throw "Claude $Method installer failed: $LASTEXITCODE" }
$cli = Join-Path $env:USERPROFILE '.local\bin\claude.exe'
if (-not (Test-Path -LiteralPath $cli -PathType Leaf)) { throw "Installer did not create $cli" }
$version = & $cli --version
if ($LASTEXITCODE -ne 0) { throw 'Installed Claude CLI cannot report its version' }
@{
  method = $Method
  cli = $cli
  version = "$version"
  sha256 = (Get-FileHash -LiteralPath $cli -Algorithm SHA256).Hash
  signature = "$( (Get-AuthenticodeSignature -LiteralPath $cli).Status )"
  processPathChanged = $beforePath -ne $env:PATH
  userPathChanged = $beforeUserPath -ne [Environment]::GetEnvironmentVariable('PATH', 'User')
  userPathContainsInstall = ([Environment]::GetEnvironmentVariable('PATH', 'User') -split ';') -contains (Split-Path $cli)
} | ConvertTo-Json | Set-Content -Encoding utf8 (Join-Path $evidence 'install.json')
Write-Output "Installed real Claude via $Method`: $version"
