param([Parameter(Mandatory=$true)][int]$RootProcessId)
$ErrorActionPreference='Stop'
$processes=@(Get-CimInstance Win32_Process)
. "$PSScriptRoot/process-lineage.ps1"
$lineage=@(Get-ProcessLineage -Snapshot $processes -RootProcessId $RootProcessId)
$rows=@(foreach($metadata in $lineage){
    $processId=[int]$metadata.ProcessId
    $process=Get-Process -Id $processId -ErrorAction SilentlyContinue
    if($process -and [Math]::Abs(($process.StartTime.ToUniversalTime()-$metadata.CreationDate.ToUniversalTime()).TotalSeconds) -lt 0.01){[pscustomobject]@{pid=$process.Id;parent_pid=[int]$metadata.ParentProcessId;created_utc=$process.StartTime.ToUniversalTime().ToString('o');name=$process.ProcessName;working_set=$process.WorkingSet64;private_bytes=$process.PrivateMemorySize64;cpu_seconds=$process.TotalProcessorTime.TotalSeconds}}
})
[pscustomobject]@{processes=$rows;working_set=($rows|Measure-Object working_set -Sum).Sum;private_bytes=($rows|Measure-Object private_bytes -Sum).Sum;cpu_seconds=($rows|Measure-Object cpu_seconds -Sum).Sum} | ConvertTo-Json -Depth 4 -Compress
