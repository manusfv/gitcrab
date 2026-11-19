# Installs the NSIS installer silently into a temporary folder, checks what it
# installed, then uninstalls silently and checks it cleaned up.
#
# Usage: scripts/verify-windows-installer.ps1 <setup.exe> <gitcrab.exe it should contain>
#
# It writes the per-user uninstall registry key and Start menu and desktop
# shortcuts while it runs, and removes them again. Run it on a machine where
# GitCrab is not installed with the installer (CI runners, a VM).
param(
    [Parameter(Mandatory)] [string] $Installer,
    [Parameter(Mandatory)] [string] $ExpectedExe
)
$ErrorActionPreference = 'Stop'

$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\GitCrab'
if (Test-Path $uninstallKey) {
    throw "GitCrab is already installed for this user; refusing to touch that install."
}
$dir = Join-Path ([IO.Path]::GetTempPath()) "gitcrab-install-$PID"
$startMenu = [Environment]::GetFolderPath('Programs')
$desktop = [Environment]::GetFolderPath('Desktop')
function Find-Shortcuts {
    @(Get-ChildItem -Path $startMenu -Recurse -Filter 'GitCrab.lnk' -ErrorAction SilentlyContinue) +
    @(Get-ChildItem -Path $desktop -Filter 'GitCrab.lnk' -ErrorAction SilentlyContinue)
}
function Invoke-Silent([string] $exe, [string[]] $arguments) {
    # NSIS requires /D= (and _?=) to be the last argument, unquoted.
    $p = Start-Process -FilePath $exe -ArgumentList $arguments -Wait -PassThru
    if ($p.ExitCode -ne 0) { throw "$exe exited with code $($p.ExitCode)" }
}

try {
    Invoke-Silent $Installer @('/S', "/D=$dir")

    $exe = Join-Path $dir 'gitcrab.exe'
    foreach ($f in $exe, (Join-Path $dir 'uninstall.exe')) {
        if (!(Test-Path $f)) { throw "Installer did not create $f" }
    }
    if ((Get-FileHash $exe).Hash -ne (Get-FileHash $ExpectedExe).Hash) {
        throw "Installed gitcrab.exe differs from $ExpectedExe"
    }
    $entry = Get-ItemProperty $uninstallKey
    if ($entry.DisplayName -ne 'GitCrab') { throw "Uninstall entry DisplayName is '$($entry.DisplayName)'" }
    Write-Host "Installed GitCrab $($entry.DisplayVersion) to $dir"
    $shortcuts = Find-Shortcuts
    if ($shortcuts.Count -lt 2) { throw "Expected Start menu and desktop shortcuts, found: $($shortcuts.FullName -join ', ')" }
    $shell = New-Object -ComObject WScript.Shell
    foreach ($s in $shortcuts) {
        $target = $shell.CreateShortcut($s.FullName).TargetPath
        if ($target -ne $exe) { throw "$($s.FullName) points to '$target', not $exe" }
    }

    Invoke-Silent (Join-Path $dir 'uninstall.exe') @('/S', "_?=$dir")

    if (Test-Path $exe) { throw 'Uninstaller left gitcrab.exe behind' }
    if (Test-Path $uninstallKey) { throw 'Uninstaller left the uninstall registry key behind' }
    $left = Find-Shortcuts
    if ($left.Count) { throw "Uninstaller left shortcuts behind: $($left.FullName -join ', ')" }
    Write-Host 'Uninstalled cleanly'
}
finally {
    # The in-place uninstaller cannot delete itself.
    Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
}
