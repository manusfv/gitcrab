param([Parameter(Mandatory=$true)][ValidatePattern('^[a-z0-9-]+$')][string]$Name)
$ErrorActionPreference='Stop'
$root=Split-Path $PSScriptRoot -Parent
$out=Join-Path $root 'dist/gpui-packages'
New-Item -ItemType Directory -Force $out | Out-Null
$stage=Join-Path $out "gitcrab-$Name"
if(Test-Path -LiteralPath $stage){throw "Package staging directory exists: $stage"}
New-Item -ItemType Directory $stage | Out-Null
if($IsMacOS){
  $app=Join-Path $stage 'GitCrab.app/Contents'
  New-Item -ItemType Directory -Force "$app/MacOS" | Out-Null
  Copy-Item "$root/target/release/gitcrab-gpui" "$app/MacOS/gitcrab"
  @'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>CFBundleName</key><string>GitCrab</string><key>CFBundleIdentifier</key><string>dev.gitcrab.desktop</string><key>CFBundleExecutable</key><string>gitcrab</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleVersion</key><string>0.1.0</string><key>LSMinimumSystemVersion</key><string>12.0</string><key>NSHighResolutionCapable</key><true/></dict></plist>
'@ | Set-Content "$app/Info.plist"
  & chmod +x "$app/MacOS/gitcrab"
  & codesign --force --deep --sign - (Join-Path $stage 'GitCrab.app')
  if($LASTEXITCODE -ne 0){throw 'Ad-hoc codesign failed'}
  & codesign --verify --deep --strict (Join-Path $stage 'GitCrab.app')
  if($LASTEXITCODE -ne 0){throw 'Signature verification failed'}
}elseif($IsWindows){Copy-Item "$root/target/release/gitcrab-gpui.exe" "$stage/gitcrab.exe"}
else{Copy-Item "$root/target/release/gitcrab-gpui" "$stage/gitcrab"; & chmod +x "$stage/gitcrab"}
Copy-Item "$root/LICENSE" $stage
Copy-Item "$root/THIRD_PARTY.md" $stage
& "$PSScriptRoot/dependency-notices.ps1" -Destination (Join-Path $stage 'DEPENDENCY-LICENSES.txt')
if($IsWindows){$archive="$stage.zip";if(Test-Path $archive){throw 'Archive already exists'};Compress-Archive "$stage/*" $archive}
else{$archive="$stage.tar.gz";if(Test-Path $archive){throw 'Archive already exists'};& tar -czf $archive -C $out "gitcrab-$Name";if($LASTEXITCODE -ne 0){throw 'tar failed'}}
$hash=(Get-FileHash $archive -Algorithm SHA256).Hash.ToLowerInvariant()
"$hash  $([IO.Path]::GetFileName($archive))" | Set-Content "$archive.sha256"
Write-Output $archive
