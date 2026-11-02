param([string]$Repository='fixtures\showcase')
$ErrorActionPreference='Stop'
$root=Split-Path $PSScriptRoot -Parent
$repo=(Resolve-Path -LiteralPath (Join-Path $root $Repository)).Path
$exe=Join-Path $root 'target\debug\gitcrab.exe'
$env:GITCRAB_CAPTURE_EXIT='1'
foreach($view in @('history','staging','diff','split','commitdiff')) {
    $env:GITCRAB_SCREENSHOT=Join-Path $root "evidence\gitcrab-$view.png"
    $env:GITCRAB_CAPTURE_VIEW=$view
    $app=Start-Process -FilePath $exe -ArgumentList ('"'+$repo+'"') -WorkingDirectory $root -WindowStyle Hidden -PassThru
    if(-not $app.WaitForExit(30000)){throw "Capture did not complete: $view (PID $($app.Id))"}
    if($app.ExitCode -ne 0){throw "Capture app exited $($app.ExitCode): $view"}
    Get-Item -LiteralPath $env:GITCRAB_SCREENSHOT | Select-Object Name,Length
}
