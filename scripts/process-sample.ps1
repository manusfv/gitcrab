param([Parameter(Mandatory=$true)][int]$RootProcessId)
$ErrorActionPreference='Stop'
$processes=@(Get-CimInstance Win32_Process)
$ids=[Collections.Generic.HashSet[int]]::new()
$null=$ids.Add($RootProcessId)
do {
    $added=$false
    foreach($process in $processes) {
        if($ids.Contains([int]$process.ParentProcessId) -and $ids.Add([int]$process.ProcessId)) {$added=$true}
    }
} while($added)
$rows=@(foreach($processId in $ids){
    $process=Get-Process -Id $processId -ErrorAction SilentlyContinue
    if($process){[pscustomobject]@{pid=$process.Id;name=$process.ProcessName;working_set=$process.WorkingSet64;private_bytes=$process.PrivateMemorySize64;cpu_seconds=$process.TotalProcessorTime.TotalSeconds}}
})
[pscustomobject]@{processes=$rows;working_set=($rows|Measure-Object working_set -Sum).Sum;private_bytes=($rows|Measure-Object private_bytes -Sum).Sum;cpu_seconds=($rows|Measure-Object cpu_seconds -Sum).Sum} | ConvertTo-Json -Depth 4 -Compress
