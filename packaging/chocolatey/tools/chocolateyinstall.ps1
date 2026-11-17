$ErrorActionPreference = 'Stop'

# The per-user NSIS setup, served from R2 through api.rmgn.ca. Replace checksum64 with the setup's
# line in the release's SHA256SUMS.txt before packing (docs/releasing.md).
$packageArgs = @{
  packageName    = $env:ChocolateyPackageName
  fileType       = 'exe'
  url64bit       = 'https://api.rmgn.ca/unflub/v1/files/0.1.0/unflub_0.1.0_x64-setup.exe'
  checksum64     = '0000000000000000000000000000000000000000000000000000000000000000'
  checksumType64 = 'sha256'
  # /SOURCE tells unflub how it was installed (install.rs); Chocolatey counts as the setup.
  silentArgs     = '/S /SOURCE=chocolatey'
  validExitCodes = @(0)
  softwareName   = 'unflub'
}

Install-ChocolateyPackage @packageArgs
