param([string]$Repository='fixtures/showcase',[string]$Binary='target/release/gitcrab.exe',[string]$OutputDirectory='evidence/tauri')
$ErrorActionPreference='Stop'
if(-not $IsWindows){throw 'This capture driver targets WebView2.'}
& node scripts/capture-tauri.mjs $Binary $Repository $OutputDirectory
if($LASTEXITCODE -ne 0){throw 'Native WebView capture failed'}
