param([string]$Binary='target/release/gitcrab.exe',[string]$OutputDirectory='dist/native-e2e')
$ErrorActionPreference='Stop'
if(-not $IsWindows){throw 'Use scripts/webdriver-smoke.mjs with the native-smoke debug build on Linux/macOS (under Xvfb for headless Linux).'}
& node scripts/native-e2e.mjs $Binary $OutputDirectory
if($LASTEXITCODE -ne 0){throw 'Native workflow checks failed'}
