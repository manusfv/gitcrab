param([Parameter(Mandatory=$true)][string]$Destination)
$ErrorActionPreference='Stop'
$root=Split-Path $PSScriptRoot -Parent
$hostLine=(& rustc -vV | Where-Object {$_ -like 'host: *'})
if($LASTEXITCODE -ne 0 -or !$hostLine){throw 'Cannot identify native Rust target'}
$nativeTarget=$hostLine.Substring(6).Trim()
$metadataText=& cargo metadata --locked --format-version 1 --filter-platform $nativeTarget --manifest-path "$root/Cargo.toml"
if($LASTEXITCODE -ne 0){throw 'Cannot resolve native dependency notices'}
$metadata=$metadataText | ConvertFrom-Json
$resolved=@{};foreach($node in $metadata.resolve.nodes){$resolved[$node.id]=$true}
$notice=[Text.StringBuilder]::new()
[void]$notice.AppendLine("GitCrab native dependency notices ($nativeTarget)")
[void]$notice.AppendLine('Includes resolved runtime and build dependencies. Cargo.lock pins provenance; these notices do not change upstream licenses.')
foreach($package in ($metadata.packages | Sort-Object name,version)){
  if(!$resolved.ContainsKey($package.id) -or $package.name -eq 'gitcrab'){continue}
  [void]$notice.AppendLine("`n===== $($package.name) $($package.version) =====")
  [void]$notice.AppendLine("License: $($package.license)")
  [void]$notice.AppendLine("Source: $($package.source) $($package.repository)")
  $directory=Split-Path $package.manifest_path -Parent
  $files=@(Get-ChildItem -LiteralPath $directory -File | Where-Object {$_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE|UNLICENSE|COPYRIGHT)([._-]|$)'})
  if($package.license_file){$files+=Get-Item -LiteralPath (Join-Path $directory $package.license_file)}
  foreach($file in ($files | Sort-Object FullName -Unique)){
    [void]$notice.AppendLine("--- $($file.Name) ---")
    [void]$notice.AppendLine([IO.File]::ReadAllText($file.FullName))
  }
}
[IO.File]::WriteAllText($Destination,$notice.ToString())
