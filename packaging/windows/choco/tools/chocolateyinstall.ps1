$ErrorActionPreference = 'Stop';

$packageName= 'Mezmo Agent'
$toolsDir   = "$(Split-Path -parent $MyInvocation.MyCommand.Definition)"
$fileLocation = Join-Path $toolsDir 'mezmo-agent.msi'

$packageArgs = @{
  packageName   = $packageName
  unzipLocation = $toolsDir
  fileType      = 'MSI'
  url64bit      = 'https://logdna-agent-build-bin.s3.amazonaws.com/3.6.0-beta.1/x86_64-pc-windows-msvc/signed/mezmo-agent.msi'
  #file         = $fileLocation

  softwareName  = 'Mezmo Agent' #part or all of the Display Name as you see it in Programs and Features. It should be enough to be unique

  checksum      = ''
  checksumType  = 'sha256' #default is md5, can also be sha1, sha256 or sha512
  checksum64    = ''
  checksumType64= 'sha256'

  silentArgs    = "/quiet" # ALLUSERS=1 DISABLEDESKTOPSHORTCUT=1 ADDDESKTOPICON=0 ADDSTARTMENU=0
  validExitCodes= @(0, 3010, 1641)
}

#https://chocolatey.org/docs/helpers-install-chocolatey-package
Install-ChocolateyPackage @packageArgs
## If you are making your own internal packages (organizations), you can embed the installer or
## put on internal file share and use the following instead (you'll need to add $file to the above)
# https://chocolatey.org/docs/helpers-install-chocolatey-install-package
#Install-ChocolateyInstallPackage @packageArgs
