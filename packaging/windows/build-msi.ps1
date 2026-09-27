# Builds the Windows installer, parterre-<version>-x86_64-pc-windows-msvc.msi, from
# parterre.wxs next to this script. Needs WiX v7 on PATH:
#
#   dotnet tool install --global wix --version 7.0.0
#
#   packaging\windows\build-msi.ps1 [-Stage DIR] [-Out FILE]
#
# -Stage is a folder holding parterre.exe, LICENSE, NOTICE and THIRD-PARTY-NOTICES.html, as the
# release workflow packages them. Without it the script gathers them in target\msi\stage, from
# target\release\parterre.exe (run `cargo build --release` first) and cargo-about.
# -Out defaults to target\msi\ with the name above.
#
# The MSI version is the version of the release tag in PARTERRE_RELEASE_TAG, as the release
# workflow sets it, or else the version parterre.exe carries (from the git tags, see
# docs/releasing.md), without its pre-release part: MSI versions are numbers only.
#
# The MSI is then checked with the ICE rules (`wix msi validate`). Two are suppressed, for the
# reasons given in parterre.wxs: ICE57 (the dual-purpose Start menu shortcut) and ICE61 (same
# version upgrades). Last, the script checks that the MSI's ProductVersion is the version above.
[CmdletBinding()]
param(
    [string]$Stage,
    [string]$Out
)
$ErrorActionPreference = 'Stop'
# Native commands that fail stop the script too (PowerShell 7.3 and newer).
$PSNativeCommandUseErrorActionPreference = $true

# Relative paths are the caller's, not the repository root's.
if ($Stage) { $Stage = (Resolve-Path $Stage).Path }
if ($Out) { $Out = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Out) }

$root = (Resolve-Path "$PSScriptRoot\..\..").Path
Push-Location $root
try {
    if (-not $Stage) {
        $Stage = "$root\target\msi\stage"
        New-Item -ItemType Directory -Force $Stage | Out-Null
        Copy-Item "$root\target\release\parterre.exe", "$root\LICENSE", "$root\NOTICE" $Stage
        cargo about generate --locked -c packaging/about.toml packaging/about.hbs `
            -o "$Stage\THIRD-PARTY-NOTICES.html"
    }

    # The release workflow's tag, like the binary's version (docs/releasing.md). Otherwise the
    # executable's own: "0.5.2-dev.3+a1b2c3d", or "0.5.1 (a1b2c3d)" from a checkout of a tag.
    if ($env:PARTERRE_RELEASE_TAG) {
        $version = $env:PARTERRE_RELEASE_TAG -replace '^v', ''
    } else {
        $version = ((Get-Item "$Stage\parterre.exe").VersionInfo.ProductVersion -split ' ')[0]
        if (-not $version) {
            throw "$Stage\parterre.exe has no version information: it was built without rc.exe (docs/building.md)"
        }
    }
    $msiVersion = ($version -split '[-+]')[0]
    if (-not $Out) {
        New-Item -ItemType Directory -Force "$root\target\msi" | Out-Null
        $Out = "$root\target\msi\parterre-$version-x86_64-pc-windows-msvc.msi"
    }

    # -acceptEula wix7: WiX v7's Open Source Maintenance Fee EULA, which asks a fee only of
    # users with revenue from it (docs/distribution.md). Accepted per run, so no file is left
    # behind.
    # No .wixpdb (debug information for patches, which parterre doesn't ship).
    wix build -acceptEula wix7 -arch x64 -pdbtype none `
        -d "Version=$msiVersion" `
        -d "Stage=$Stage" `
        -d "Icon=$root\packaging\icon\parterre.ico" `
        -o $Out `
        packaging\windows\parterre.wxs
    wix msi validate -acceptEula wix7 -sice ICE57 -sice ICE61 $Out

    # The version Windows and winget see, read back from the MSI (v0.5.0 shipped one saying
    # 0.4.0). Windows Installer's COM objects have no type information, hence InvokeMember.
    $installer = New-Object -ComObject WindowsInstaller.Installer
    $invoke = { param($object, $member, $kind, $arguments) $object.GetType().InvokeMember($member, $kind, $null, $object, $arguments) }
    $database = & $invoke $installer OpenDatabase InvokeMethod @($Out, 0)
    $view = & $invoke $database OpenView InvokeMethod @("SELECT Value FROM Property WHERE Property = 'ProductVersion'")
    & $invoke $view Execute InvokeMethod $null
    $productVersion = & $invoke (& $invoke $view Fetch InvokeMethod $null) StringData GetProperty @(1)
    & $invoke $view Close InvokeMethod $null
    [void][Runtime.InteropServices.Marshal]::ReleaseComObject($database)
    if ($productVersion -ne $msiVersion) {
        throw "The MSI's ProductVersion is '$productVersion', expected '$msiVersion'"
    }
    Write-Host "Built $Out, version $productVersion"
}
finally {
    Pop-Location
}
