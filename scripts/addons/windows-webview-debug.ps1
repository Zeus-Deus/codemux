param([ValidateSet('enable', 'disable')][string]$Mode)
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
  throw 'Requires disposable GitHub CI'
}
# GitHub's elevated runner ignores WEBVIEW2_* environment overrides. Use the
# documented machine override for this executable only; never change a value
# already present on the runner. No product setting or binary is modified.
# The acceptance run opens an add-on link, which starts the runner's own
# browser in front of the app. Chromium's switch for tests keeps a covered
# window visible to its page, so frames and screenshots still come from the
# app. It is a plain switch: it cannot replace the app's --disable-features.
$key = 'HKLM:\SOFTWARE\Policies\Microsoft\Edge\WebView2\AdditionalBrowserArguments'
$name = 'codemux.exe'
$value = '--remote-debugging-port=9231 --disable-backgrounding-occluded-windows'
if ($Mode -eq 'enable') {
  if (Get-ItemProperty -Path $key -Name $name -ErrorAction SilentlyContinue) {
    throw 'Refusing to replace an existing CodeMux WebView2 policy'
  }
  New-Item -Path $key -Force | Out-Null
  New-ItemProperty -Path $key -Name $name -Value $value -PropertyType String | Out-Null
} else {
  $current = Get-ItemProperty -Path $key -Name $name -ErrorAction SilentlyContinue
  if ($current -and $current.$name -eq $value) {
    Remove-ItemProperty -Path $key -Name $name
  }
}
