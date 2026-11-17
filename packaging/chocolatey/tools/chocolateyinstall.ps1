$ErrorActionPreference = 'Stop'

# The per-user NSIS setup from the GitHub release. Replace checksum64 with the setup's line in the
# release's SHA256SUMS.txt before packing (docs/releasing.md).
$packageArgs = @{
  packageName    = $env:ChocolateyPackageName
  fileType       = 'exe'
  url64bit       = 'https://github.com/ReimagineMedias/unflub/releases/download/v0.1.0/unflub_0.1.0_x64-setup.exe'
  checksum64     = '0000000000000000000000000000000000000000000000000000000000000000'
  checksumType64 = 'sha256'
  silentArgs     = '/S'
  validExitCodes = @(0)
  softwareName   = 'unflub'
}

Install-ChocolateyPackage @packageArgs
