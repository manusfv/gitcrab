param(
    [Parameter(Mandatory=$true)][string]$Name,
    [Parameter(Mandatory=$true)][ValidateSet('nsis','deb','app')][string]$Bundles
)
$ErrorActionPreference='Stop'
if($Name -notmatch '^[a-z0-9-]+$'){throw 'Invalid package name'}
& npm run tauri -- build --ci --bundles $Bundles -- --locked
if($LASTEXITCODE -ne 0){throw 'Tauri build failed'}
$destination=Join-Path (Split-Path $PSScriptRoot -Parent) 'dist/packages'
New-Item -ItemType Directory -Force -Path $destination | Out-Null
if($Bundles -eq 'app'){
    $archive=Join-Path $destination "$Name.tar.gz"
    if(Test-Path -LiteralPath $archive){throw "Package already exists: $archive"}
    & tar -czf $archive -C target/release/bundle/macos GitCrab.app
    if($LASTEXITCODE -ne 0){throw 'Could not archive app bundle'}
    $files=@(Get-Item -LiteralPath $archive)
}else{
    $pattern=if($Bundles -eq 'nsis'){'target/release/bundle/nsis/*.exe'}else{'target/release/bundle/deb/*.deb'}
    $files=@(Get-ChildItem -Path $pattern)
    if(-not $files.Count){throw 'No native Tauri package was produced'}
    foreach($file in $files){
        $target=Join-Path $destination $file.Name
        if(Test-Path -LiteralPath $target){throw "Package already exists: $target"}
        Copy-Item -LiteralPath $file.FullName -Destination $target
    }
    $files=@(Get-ChildItem -LiteralPath $destination -File | Where-Object Extension -In @('.exe','.deb'))
}
foreach($file in $files){
    $hash=Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256
    [IO.File]::WriteAllText(($file.FullName+'.sha256'),($hash.Hash.ToLowerInvariant()+'  '+$file.Name+"`n"))
}
$files | Select-Object Name,Length
