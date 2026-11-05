param([string]$Executable='target/release/gitcrab.exe',[string]$Destination='evidence/gpui/performance.json',[string]$ExecutableSource='')
$ErrorActionPreference='Stop'
$root=Split-Path $PSScriptRoot -Parent
if(!$ExecutableSource){$ExecutableSource=(& git -C $root rev-parse HEAD)}
$Executable=[IO.Path]::GetFullPath((Join-Path $root $Executable))
$out=Join-Path $root ('.references/measure-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force $out | Out-Null
$env:GITCRAB_CAPTURE_WIDTH='1172';$env:GITCRAB_CAPTURE_HEIGHT='636'
$env:GITCRAB_NATIVE_BENCH='1';$env:GITCRAB_SETTINGS=Join-Path $out 'appearance.json'
$cases=@()
foreach($case in @(@{repo='showcase';limit=500;view='history'},@{repo='large';limit=10000;view='history'},@{repo='merge-heavy';limit=10000;view='history'},@{repo='merge-heavy';limit=10000;view='diff'})){
  $samples=@()
  for($sample=0;$sample -lt 3;$sample++){
    $ready=Join-Path $out "$($case.repo)-$($case.view)-$sample.json"
    $env:GITCRAB_NATIVE_READY=$ready;$env:GITCRAB_CAPTURE_VIEW=$case.view;$env:GITCRAB_CAPTURE_LIMIT=[string]$case.limit
    $start=[Diagnostics.ProcessStartInfo]::new($Executable);$start.UseShellExecute=$false
    $start.ArgumentList.Add((Join-Path $root "fixtures/$($case.repo)"))
    $clock=[Diagnostics.Stopwatch]::StartNew();$process=[Diagnostics.Process]::Start($start)
    try{
      while(!(Test-Path $ready)){if($process.HasExited -or $clock.Elapsed.TotalSeconds -gt 45){throw 'Native ready timed out'};Start-Sleep -Milliseconds 10}
      $startup=$clock.Elapsed.TotalMilliseconds
      while(!(Test-Path "$ready.bench.json")){if($process.HasExited -or $clock.Elapsed.TotalSeconds -gt 60){throw 'Native scroll measurement timed out'};Start-Sleep -Milliseconds 25}
      $frames=Get-Content "$ready.bench.json" -Raw|ConvertFrom-Json
      $sorted=@($frames.samples_ms|Sort-Object)
      $before=& "$PSScriptRoot/process-sample.ps1" -RootProcessId $process.Id | ConvertFrom-Json
      Start-Sleep -Seconds 3
      $after=& "$PSScriptRoot/process-sample.ps1" -RootProcessId $process.Id | ConvertFrom-Json
      $samples+=@{startup_to_layout_ms=$startup;native=(Get-Content $ready -Raw|ConvertFrom-Json);frames=@{samples_ms=$frames.samples_ms;p50_ms=$sorted[[int][math]::Floor($sorted.Count*.5)];p95_ms=$sorted[[int][math]::Floor($sorted.Count*.95)];p99_ms=$sorted[[int][math]::Floor($sorted.Count*.99)];over_16_7_ms=@($sorted|Where-Object {$_ -gt 16.7}).Count};idle=@{duration_seconds=3;cpu_seconds=$after.cpu_seconds-$before.cpu_seconds;working_set=$after.working_set;private_bytes=$after.private_bytes;processes=$after.processes}}
    }finally{if(!$process.HasExited){$process.Kill();$process.WaitForExit()}}
  }
  $total=& git -C (Join-Path $root "fixtures/$($case.repo)") rev-list --all --count
  $cases+=@{fixture=$case.repo;view=$case.view;total=[int]$total;samples=$samples}
}
$result=@{date=[DateTime]::UtcNow.ToString('o');source=(&git -C $root rev-parse HEAD);binary_sha256=(Get-FileHash $Executable).Hash;hardware=@{cpu=(Get-CimInstance Win32_Processor|Select-Object -First 1 -ExpandProperty Name);logical_cpus=[Environment]::ProcessorCount;gpu=(Get-CimInstance Win32_VideoController|Select-Object -ExpandProperty Name);os=[Environment]::OSVersion.VersionString};conditions='Release native-evidence build; 1172x636 logical, scale recorded per case. Three fresh processes per workload, unflushed OS caches, concurrent desktop apps. 240 synthetic GPUI scroll/render intervals per sample, not compositor presentation or physical input latency. Startup includes process launch and 10ms ready polling; first layout marker is not physical presentation. Total native descendants included in resource samples after Git worker becomes idle.';cases=$cases}
$result.source=$ExecutableSource
$result.harness_source=(& git -C $root rev-parse HEAD)
$result.conditions+=' Process ancestry requires child creation time at or after its parent, excluding stale parent PIDs reused by Windows.'
$result|ConvertTo-Json -Depth 12|Set-Content (Join-Path $root $Destination)
Remove-Item Env:GITCRAB_NATIVE_BENCH,Env:GITCRAB_NATIVE_READY,Env:GITCRAB_CAPTURE_VIEW,Env:GITCRAB_CAPTURE_LIMIT,Env:GITCRAB_CAPTURE_WIDTH,Env:GITCRAB_CAPTURE_HEIGHT,Env:GITCRAB_SETTINGS -ErrorAction SilentlyContinue
