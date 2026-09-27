$ErrorActionPreference = 'Stop'

# Installs the MSI from the GitHub Release machine-wide (ALLUSERS=1). build.ps1 fills in the
# URL and checksum.
$packageArgs = @{
  packageName    = $env:ChocolateyPackageName
  fileType       = 'msi'
  url64bit       = '__URL__'
  checksum64     = '__CHECKSUM__'
  checksumType64 = 'sha256'
  softwareName   = 'parterre'
  silentArgs     = "/qn /norestart ALLUSERS=1 /l*v `"$($env:TEMP)\$($env:ChocolateyPackageName).$($env:ChocolateyPackageVersion).MsiInstall.log`""
  validExitCodes = @(0, 3010, 1641)
}

Install-ChocolateyPackage @packageArgs
