# Builds the Chocolatey package, parterre.<version>.nupkg, for a release tag from the files
# next to this script. Needs choco (Windows):
#
#   packaging\chocolatey\build.ps1 -Tag v0.5.1 -Msi parterre-0.5.1-x86_64-pc-windows-msvc.msi [-Out DIR]
#
# -Msi is that release's MSI, as downloaded from its GitHub Release; the package downloads the
# same file when installed, and checks it against this one's SHA-256. -Out defaults to the
# current directory. See docs/releasing.md#chocolatey.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Tag,
    [Parameter(Mandatory)][string]$Msi,
    [string]$Out = '.'
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

if ($Tag -notmatch '^v(\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?)$') {
    throw "Tag must be vX.Y.Z with an optional pre-release part, not '$Tag'"
}
$version = $Matches[1]
$name = "parterre-$version-x86_64-pc-windows-msvc.msi"
if ((Split-Path -Leaf $Msi) -ne $name) {
    throw "Expected $name for tag $Tag, got $Msi"
}
$tokens = @{
    __VERSION__  = $version
    __TAG__      = $Tag
    __URL__      = "https://github.com/aquamoth/parterre/releases/download/$Tag/$name"
    __CHECKSUM__ = (Get-FileHash -Algorithm SHA256 $Msi).Hash
}

$stage = Join-Path ([IO.Path]::GetTempPath()) "parterre-chocolatey-$version"
Remove-Item -Recurse -Force $stage -ErrorAction Ignore
Copy-Item -Recurse $PSScriptRoot $stage
Remove-Item (Join-Path $stage 'build.ps1')
foreach ($file in Get-ChildItem -Recurse -File $stage) {
    $text = [IO.File]::ReadAllText($file.FullName)
    foreach ($token in $tokens.Keys) { $text = $text.Replace($token, $tokens[$token]) }
    if ($text -match '__[A-Z]+__') { throw "Unfilled $($Matches[0]) in $($file.Name)" }
    # UTF-8 without a byte order mark, as choco expects.
    [IO.File]::WriteAllText($file.FullName, $text)
}

New-Item -ItemType Directory -Force $Out | Out-Null
choco pack (Join-Path $stage 'parterre.nuspec') --outputdirectory $Out
Write-Host "Built $(Join-Path $Out "parterre.$version.nupkg")"
