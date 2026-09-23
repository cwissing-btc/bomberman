# Build the Windows client natively on Windows into dist\Bomberman.exe.
#
# Needs Rust (https://rustup.rs) with the default MSVC toolchain and the
# "Desktop development with C++" workload of the Visual Studio Build Tools,
# which provides the linker and rc.exe for the icon.
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

cargo build --release -p bomber-client
New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item target\release\bomberman.exe dist\Bomberman.exe -Force
Get-Item dist\Bomberman.exe | Format-List Name, Length
