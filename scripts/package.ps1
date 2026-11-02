param(
    [Parameter(Mandatory=$true)][string]$Binary,
    [Parameter(Mandatory=$true)][string]$Name,
    [string]$Destination=(Join-Path (Split-Path $PSScriptRoot -Parent) 'dist')
)
$ErrorActionPreference='Stop'
if ($Name -notmatch '^[a-z0-9-]+$') {throw 'Package name must contain only lowercase letters, digits, and hyphens'}
$root=Split-Path $PSScriptRoot -Parent
$Binary=(Resolve-Path -LiteralPath $Binary).Path
$Destination=[IO.Path]::GetFullPath($Destination)
$stage=Join-Path $Destination $Name
if (Test-Path -LiteralPath $stage) {throw "Package staging path already exists: $stage"}
New-Item -ItemType Directory -Force -Path $Destination | Out-Null
New-Item -ItemType Directory -Path $stage | Out-Null
Copy-Item -LiteralPath (Join-Path $root 'README.md'),(Join-Path $root 'LICENSE') -Destination $stage
$version=[regex]::Match([IO.File]::ReadAllText((Join-Path $root 'Cargo.toml')),'(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
if (-not $version) {throw 'Could not read the package version'}
if ($IsMacOS) {
    $app=Join-Path $stage 'GitCrab.app'
    $contents=Join-Path $app 'Contents'
    $macos=Join-Path $contents 'MacOS'
    New-Item -ItemType Directory -Force -Path $macos | Out-Null
    $executable=Join-Path $macos 'gitcrab'
    Copy-Item -LiteralPath $Binary -Destination $executable
    & chmod 755 $executable
    if ($LASTEXITCODE -ne 0) {throw 'Could not set app executable permissions'}
    $plist=@"
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>gitcrab</string>
<key>CFBundleIdentifier</key><string>dev.gitcrab.desktop</string>
<key>CFBundleName</key><string>GitCrab</string>
<key>CFBundleDisplayName</key><string>GitCrab</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$version</string>
<key>CFBundleVersion</key><string>$version</string>
<key>LSMinimumSystemVersion</key><string>12.0</string>
<key>NSHighResolutionCapable</key><true/>
<key>NSPrincipalClass</key><string>NSApplication</string>
</dict></plist>
"@
    [IO.File]::WriteAllText((Join-Path $contents 'Info.plist'),$plist)
    & plutil -lint (Join-Path $contents 'Info.plist')
    if ($LASTEXITCODE -ne 0) {throw 'Invalid application plist'}
    # Local ad-hoc signing needs no Apple account or signing credentials.
    & codesign --force --sign - --timestamp=none $app
    if ($LASTEXITCODE -ne 0) {throw 'Ad-hoc app signing failed'}
    & codesign --verify --deep --strict $app
    if ($LASTEXITCODE -ne 0) {throw 'App signature verification failed'}
} else {
    $executable=Join-Path $stage $(if ($IsWindows) {'gitcrab.exe'} else {'gitcrab'})
    Copy-Item -LiteralPath $Binary -Destination $executable
    if (-not $IsWindows) {& chmod 755 $executable; if ($LASTEXITCODE -ne 0) {throw 'Could not set executable permissions'}}
}
if ($IsWindows) {
    $archive=Join-Path $Destination "$Name.zip"
    if (Test-Path -LiteralPath $archive) {throw "Archive exists: $archive"}
    Compress-Archive -LiteralPath $stage -DestinationPath $archive
} else {
    $archive=Join-Path $Destination "$Name.tar.gz"
    if (Test-Path -LiteralPath $archive) {throw "Archive exists: $archive"}
    & tar -czf $archive -C $Destination $Name
    if ($LASTEXITCODE -ne 0) {throw 'Archive creation failed'}
}
$hash=Get-FileHash -LiteralPath $archive -Algorithm SHA256
[IO.File]::WriteAllText("$archive.sha256",($hash.Hash.ToLowerInvariant()+'  '+[IO.Path]::GetFileName($archive)+"`n"))
Get-Item -LiteralPath $archive,"$archive.sha256" | Select-Object Name,Length
