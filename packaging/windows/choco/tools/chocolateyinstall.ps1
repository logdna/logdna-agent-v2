$ErrorActionPreference = 'Stop';

$packageName= 'mezmo-agent'
$toolsDir   = "$(Split-Path -parent $MyInvocation.MyCommand.Definition)"
$fileLocation = Join-Path $toolsDir 'mezmo-agent.msi'

$packageArgs = @{
  packageName   = $packageName
  unzipLocation = $toolsDir
  fileType      = 'MSI'
  url64bit      = '${MSI_URL}'
  #file         = $fileLocation

  softwareName  = 'Mezmo Agent'

  checksum      = '${MSI_SHA256}'
  checksumType  = 'sha256'

  silentArgs    = "/quiet" # ALLUSERS=1 DISABLEDESKTOPSHORTCUT=1 ADDDESKTOPICON=0 ADDSTARTMENU=0
  validExitCodes= @(0, 3010, 1641)
}

Install-ChocolateyPackage @packageArgs
