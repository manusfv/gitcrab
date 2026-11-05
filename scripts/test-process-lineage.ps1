$ErrorActionPreference='Stop'
. "$PSScriptRoot/process-lineage.ps1"
$now=[DateTime]::UtcNow
$snapshot=@(
    [pscustomobject]@{ProcessId=100;ParentProcessId=1;CreationDate=$now},
    [pscustomobject]@{ProcessId=200;ParentProcessId=100;CreationDate=$now.AddSeconds(-60)},
    [pscustomobject]@{ProcessId=201;ParentProcessId=200;CreationDate=$now.AddSeconds(-30)},
    [pscustomobject]@{ProcessId=300;ParentProcessId=100;CreationDate=$now.AddSeconds(1)},
    [pscustomobject]@{ProcessId=301;ParentProcessId=300;CreationDate=$now.AddSeconds(2)}
)
$actual=@(Get-ProcessLineage -Snapshot $snapshot -RootProcessId 100|Sort-Object ProcessId|ForEach-Object ProcessId)
if(($actual -join ',') -ne '100,300,301'){throw "Invalid ancestry: $actual"}
$failed=$false
try{Get-ProcessLineage -Snapshot $snapshot -RootProcessId 999|Out-Null}catch{$failed=$true}
if(!$failed){throw 'A missing measured root must reject the sample'}
'Process ancestry regressions passed: reused PID excluded, live descendants retained, missing root rejected.'
