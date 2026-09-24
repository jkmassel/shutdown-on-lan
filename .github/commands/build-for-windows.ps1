# Builds the release binary and packages it as an installer in build\windows.
#
# The installer's version comes from Cargo.toml unless -Version is given. Requires the WiX Toolset v3.
param(
    [string] $Version,
    [string] $Output = 'Product.msi'
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

Set-Location (Resolve-Path "$PSScriptRoot\..\..")

cargo build --release
Copy-Item target\release\shutdown-on-lan.exe build\windows\shutdown-on-lan.exe

if (-not $Version) {
    $Version = (cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).packages[0].version
}

Write-Host "Build Complete – packaging version $Version as $Output"

Push-Location build\windows
try {
    & "$env:WIX\bin\candle.exe" -arch x64 "-dProductVersion=$Version" -ext WixFirewallExtension -ext WixUtilExtension Product.wxs
    & "$env:WIX\bin\light.exe" -ext WixFirewallExtension -ext WixUtilExtension -out $Output Product.wixobj
} finally {
    Pop-Location
}
