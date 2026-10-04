# Installs the MSI as a user would, in one scope, checks that parterre runs and is on that
# scope's PATH, and removes it again. The release workflow runs it in both scopes before it
# publishes. The machine scope needs an elevated prompt.
#
#   packaging\windows\test-msi.ps1 -Msi parterre-0.5.1-x86_64-pc-windows-msvc.msi -Scope user|machine
#
# user is the MSI's default (`msiexec /i parterre.msi`), as winget installs it for a user;
# machine is `ALLUSERS=1`, as Chocolatey installs it.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Msi,
    [Parameter(Mandatory)][ValidateSet('user', 'machine')][string]$Scope
)
$ErrorActionPreference = 'Stop'

$Msi = (Resolve-Path $Msi).Path
$extra, $folder, $pathScope = if ($Scope -eq 'machine') {
    @('ALLUSERS=1'), "$env:ProgramFiles\parterre", 'Machine'
} else {
    @(), "$env:LOCALAPPDATA\Programs\parterre", 'User'
}
$exe = Join-Path $folder 'parterre.exe'

# msiexec returns at once unless waited for; 3010 is success with a restart pending.
function Invoke-Msiexec([string]$Action) {
    $log = Join-Path $env:TEMP "parterre-msi-$Scope-$Action.log"
    $arguments = @("/$Action", "`"$Msi`"") + $extra + @('/qn', '/norestart', '/l*v', "`"$log`"")
    $process = Start-Process msiexec.exe -ArgumentList $arguments -Wait -PassThru
    if ($process.ExitCode -notin 0, 3010) {
        Get-Content $log -Tail 40
        throw "msiexec /$Action failed: $($process.ExitCode) (log: $log)"
    }
}

function Test-OnPath {
    $path = [Environment]::GetEnvironmentVariable('Path', $pathScope)
    # The installer writes the folder with a trailing backslash.
    ($path -split ';' | ForEach-Object { $_.TrimEnd('\') }) -contains $folder
}

Invoke-Msiexec i
if (-not (Test-Path $exe)) { throw "$exe is not there after installing" }
# PowerShell doesn't capture the output of a GUI-subsystem program like parterre.exe
# (`& $exe` returns nothing), so redirect it to a file.
$out = Join-Path $env:TEMP "parterre-version-$Scope.txt"
Start-Process $exe --version -Wait -NoNewWindow -RedirectStandardOutput $out
$version = "$(Get-Content -Raw $out)".Trim()
Write-Host $version
if ($version -notmatch '^parterre \d+\.\d+\.\d+\S*( \([0-9a-f]{7,}\))?$') { throw "--version printed '$version'" }
if (-not (Test-OnPath)) { throw "$folder is not on the $pathScope PATH" }

Invoke-Msiexec x
if (Test-Path $exe) { throw "$exe is still there after uninstalling" }
if (Test-OnPath) { throw "$folder is still on the $pathScope PATH after uninstalling" }
Write-Host "Installed, ran and removed parterre ($Scope)"
