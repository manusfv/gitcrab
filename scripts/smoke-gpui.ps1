param([string]$Executable, [string]$OutputRoot)
$ErrorActionPreference='Stop'
$root=Split-Path $PSScriptRoot -Parent
if(!$Executable){$Executable=Join-Path $root $(if($IsWindows){'target/debug/gitcrab-gpui.exe'}else{'target/debug/gitcrab-gpui'})}
if(!$OutputRoot){$OutputRoot=Join-Path $root 'dist/gpui-smoke'}
if(Test-Path -LiteralPath $OutputRoot){throw 'Smoke destination exists; choose a new output root'}
New-Item -ItemType Directory -Force $OutputRoot | Out-Null
$fixture=Join-Path $OutputRoot 'fixtures'
& "$PSScriptRoot/create-fixtures.ps1" -OutputRoot $fixture | Out-Null
$results=@()
foreach($view in @('history','details','staging','diff','split','commitdiff','dialog')){
  $ready=Join-Path $OutputRoot "$view.json"
  $env:GITCRAB_NATIVE_READY=$ready
  $env:GITCRAB_CAPTURE_VIEW=$view
  $start=[Diagnostics.ProcessStartInfo]::new($Executable)
  $start.UseShellExecute=$false
  $start.ArgumentList.Add((Join-Path $fixture 'showcase'))
  $start.RedirectStandardError=$true
  $start.RedirectStandardOutput=$true
  $clock=[Diagnostics.Stopwatch]::StartNew()
  $process=[Diagnostics.Process]::Start($start)
  $stderr=$process.StandardError.ReadToEndAsync()
  $stdout=$process.StandardOutput.ReadToEndAsync()
  try{
    while(!(Test-Path -LiteralPath $ready)){
      if($process.HasExited){throw "Native $view exited $($process.ExitCode) before ready"}
      if($clock.Elapsed.TotalSeconds -gt 45){throw "Native $view did not render within 45 seconds"}
      Start-Sleep -Milliseconds 25
    }
    $readyMs=$clock.Elapsed.TotalMilliseconds
    $data=Get-Content $ready -Raw | ConvertFrom-Json
    if($data.loaded -ne 43){throw 'Unexpected fixture history'}
    if($view -in @('diff','split','commitdiff') -and !$data.diff_rows){throw 'Diff did not load'}
    # OS screenshots are separate evidence from the native ready marker.
    if($IsLinux){& scrot (Join-Path $OutputRoot "$view.png");if($LASTEXITCODE -ne 0){throw 'scrot failed'}}
    if($IsMacOS){& screencapture -x (Join-Path $OutputRoot "$view.png");if($LASTEXITCODE -ne 0){throw 'screencapture failed'}}
    $results+=@{view=$view;ready_after_first_layout_ms=$readyMs;process_id=$process.Id;working_set=$process.WorkingSet64;native=$data;screenshot=($IsLinux -or $IsMacOS)}
  }finally{
    if(!$process.HasExited){$process.Kill();$process.WaitForExit()}
    $stderr.GetAwaiter().GetResult() | Set-Content (Join-Path $OutputRoot "$view.stderr.log")
    $null=$stdout.GetAwaiter().GetResult()
  }
}
Remove-Item Env:GITCRAB_NATIVE_READY,Env:GITCRAB_CAPTURE_VIEW -ErrorAction SilentlyContinue
@{source=(& git -C $root rev-parse HEAD);os=[Environment]::OSVersion.VersionString;executable_sha256=(Get-FileHash $Executable).Hash;results=$results;conditions='Native evidence feature; default window 1280x800 logical; unflushed caches; single sample/view. Ready after layout is not physical presentation or input latency.'} | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $OutputRoot 'result.json')
