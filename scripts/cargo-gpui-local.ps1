<#
.SYNOPSIS
Runs Cargo using an already provisioned local Windows GNU toolchain.
.DESCRIPTION
Optional helper retained for the historical native validation workflow. This is
not a toolchain installer or the standard cross-platform development entry point.
It expects .toolchain under the Git common directory's parent, containing:
  rustup/toolchains/stable-x86_64-pc-windows-gnu
  cargo/bin/cargo.exe
  binutils/mingw64/bin
  w64devkit/bin and w64devkit/lib/libktmw32.a
Git is expected at C:/Program Files/Git/mingw64/bin. Release builds also require
the Windows SDK shader compiler (GPUI_FXC_PATH when not discovered automatically).
The helper sets process-local variables and copies libktmw32.a into the checkout's
ignored .toolchain/native-lib. It does not provision any of these prerequisites.
For the standard Rust/MSVC workflow, use cargo directly.
#>
param([Parameter(ValueFromRemainingArguments=$true)][string[]]$CargoArgs)
$ErrorActionPreference='Stop'
$root=Split-Path $PSScriptRoot -Parent
$common = (& git -C $root rev-parse --path-format=absolute --git-common-dir).Trim()
$shared = Join-Path (Split-Path $common -Parent) '.toolchain'
$env:RUSTUP_HOME=Join-Path $shared 'rustup'
$env:CARGO_HOME=Join-Path $shared 'cargo'
$toolchain=Join-Path $env:RUSTUP_HOME 'toolchains/stable-x86_64-pc-windows-gnu'
$env:PATH=(Join-Path $env:CARGO_HOME 'bin')+';'+(Join-Path $shared 'binutils/mingw64/bin')+';'+(Join-Path $shared 'w64devkit/bin')+';C:/Program Files/Git/mingw64/bin;'+$env:PATH
$nativeLib = Join-Path $root '.toolchain/native-lib'
New-Item -ItemType Directory -Force $nativeLib | Out-Null
Copy-Item -LiteralPath (Join-Path $shared 'w64devkit/lib/libktmw32.a') -Destination (Join-Path $nativeLib 'libktmw32.a') -Force
$env:GITCRAB_NATIVE_LIB_DIR=$nativeLib
$env:CARGO_ENCODED_RUSTFLAGS=@('-C','linker-flavor=ld.lld','-C',('linker='+(Join-Path $toolchain 'lib/rustlib/x86_64-pc-windows-gnu/bin/rust-lld.exe')),'-C',('dlltool='+(Join-Path $shared 'binutils/mingw64/bin/dlltool.exe'))) -join [char]31
Set-Location $root
& (Join-Path $env:CARGO_HOME 'bin/cargo.exe') @CargoArgs
exit $LASTEXITCODE


