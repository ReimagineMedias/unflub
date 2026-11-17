$ErrorActionPreference = 'Stop'

# unflub's uninstaller removes start at login. A silent uninstall keeps the settings in
# %APPDATA%\ca.rmgn.unflub, the same as leaving "Delete the application data" unticked.
[array]$keys = Get-UninstallRegistryKey -SoftwareName 'unflub'

if ($keys.Count -eq 0) {
  Write-Warning "unflub is not in Programs and Features; it may have been uninstalled already."
  return
}

foreach ($key in $keys) {
  Uninstall-ChocolateyPackage `
    -PackageName $env:ChocolateyPackageName `
    -FileType 'exe' `
    -SilentArgs '/S' `
    -File ($key.UninstallString -replace '"', '')
}
