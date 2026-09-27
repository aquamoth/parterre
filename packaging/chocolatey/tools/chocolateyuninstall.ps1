$ErrorActionPreference = 'Stop'

# Uninstalls the machine-wide MSI this package installed. A per-user install of parterre (from
# the MSI or winget) has its own entry under HKCU, which is left alone.
[array]$keys = Get-UninstallRegistryKey -SoftwareName 'parterre' |
  Where-Object { $_.PSPath -like '*HKEY_LOCAL_MACHINE*' }

if ($keys.Count -eq 1) {
  Uninstall-ChocolateyPackage -PackageName $env:ChocolateyPackageName -FileType 'msi' `
    -SilentArgs "$($keys[0].PSChildName) /qn /norestart" `
    -ValidExitCodes @(0, 3010, 1605, 1614, 1641)
} elseif ($keys.Count -eq 0) {
  Write-Warning 'parterre is not installed for all users; nothing to uninstall.'
} else {
  Write-Warning "Found $($keys.Count) machine-wide installs of parterre; uninstall them in Settings > Apps."
}
