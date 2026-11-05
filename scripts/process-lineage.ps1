function Get-ProcessLineage {
    param([Parameter(Mandatory=$true)][object[]]$Snapshot,[Parameter(Mandatory=$true)][int]$RootProcessId)
    $byId=@{}
    foreach($entry in $Snapshot){$byId[[int]$entry.ProcessId]=$entry}
    if(!$byId.ContainsKey($RootProcessId)){throw 'Measured root process exited before sampling'}
    $ids=[Collections.Generic.HashSet[int]]::new()
    $null=$ids.Add($RootProcessId)
    do {
        $added=$false
        foreach($entry in $Snapshot){
            $parentId=[int]$entry.ParentProcessId
            if($ids.Contains($parentId) -and $entry.CreationDate -and $byId[$parentId].CreationDate -and
               $entry.CreationDate -ge $byId[$parentId].CreationDate -and $ids.Add([int]$entry.ProcessId)){$added=$true}
        }
    }while($added)
    @($Snapshot|Where-Object{$ids.Contains([int]$_.ProcessId)})
}
