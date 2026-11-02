param([string]$Binary='target/release/gitcrab.exe',[string]$OutputDirectory='dist/native-e2e')
$ErrorActionPreference='Stop'
if(-not $IsWindows){throw 'Use scripts/webdriver-smoke.mjs under Xvfb on Linux. Native macOS WebDriver is unavailable.'}
& node scripts/native-e2e.mjs $Binary $OutputDirectory
if($LASTEXITCODE -ne 0){throw 'Native workflow checks failed'}
