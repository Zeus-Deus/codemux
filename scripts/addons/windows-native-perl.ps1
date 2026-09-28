$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') {
  throw 'Requires a disposable Windows GitHub runner'
}
# Git Bash's bundled Perl lacks modules required by vendored OpenSSL. The
# runner's native Strawberry Perl supports the app's MSVC dependency build;
# select it explicitly without changing the separate GNU host compiler.
$perlPath = Join-Path $env:SystemDrive 'Strawberry\perl\bin\perl.exe'
if (-not (Test-Path -LiteralPath $perlPath -PathType Leaf)) {
  throw "Native Perl is missing: $perlPath"
}
& $perlPath -MConfig -MLocale::Maketext::Simple -MIPC::Cmd -e 'die "Expected native Windows Perl" unless $^O eq "MSWin32"; print "$^V\n";'
if ($LASTEXITCODE -ne 0) { throw 'Native Perl dependency preflight failed' }
"OPENSSL_SRC_PERL=$perlPath" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append
