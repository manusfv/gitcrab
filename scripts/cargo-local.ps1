param([Parameter(ValueFromRemainingArguments=$true)][string[]]$CargoArgs)
$ErrorActionPreference='Stop'
$root=Split-Path $PSScriptRoot -Parent
$env:RUSTUP_HOME=Join-Path $root '.toolchain\rustup'
$env:CARGO_HOME=Join-Path $root '.toolchain\cargo'
$toolchain=Join-Path $env:RUSTUP_HOME 'toolchains\stable-x86_64-pc-windows-gnu'
$env:PATH=(Join-Path $env:CARGO_HOME 'bin')+';'+(Join-Path $root '.toolchain\binutils\mingw64\bin')+';C:\Program Files\Git\mingw64\bin;'+$env:PATH
$env:CARGO_ENCODED_RUSTFLAGS=@('-C','linker-flavor=ld.lld','-C',('linker='+(Join-Path $toolchain 'lib\rustlib\x86_64-pc-windows-gnu\bin\rust-lld.exe')),'-C',('dlltool='+(Join-Path $root '.toolchain\binutils\mingw64\bin\dlltool.exe'))) -join [char]31
Push-Location $root
try { & (Join-Path $env:CARGO_HOME 'bin\cargo.exe') @CargoArgs; exit $LASTEXITCODE } finally {Pop-Location}
