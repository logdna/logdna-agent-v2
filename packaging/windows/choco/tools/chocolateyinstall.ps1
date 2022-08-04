

$ErrorActionPreference = 'Stop'; # stop on all errors
$toolsDir   = "$(Split-Path -parent $MyInvocation.MyCommand.Definition)"

$logdnaCLIPath = Join-Path $toolsDir 'logdna-agent.exe'
$serviceExe = Join-Path $toolsDir 'logdna-agent-svc.exe'
$confFile = Join-Path $toolsDir 'logdna.conf'


if(!(Test-Path -Path $env:ALLUSERSPROFILE\logs)){
  New-Item -ItemType directory -Path $env:ALLUSERSPROFILE\logs -Force
}

if(!(Test-Path -Path $env:ALLUSERSPROFILE\logdna)){
  New-Item -ItemType directory -Path $env:ALLUSERSPROFILE\logdna -Force
}

if(!(Test-Path -Path $env:ALLUSERSPROFILE\logdna\logdna.conf)){
  Copy-Item -Path $confFile $env:ALLUSERSPROFILE\logdna\logdna.conf -Force
}

if(!(Test-Path -Path $env:Programfiles\Mezmo)){
  New-Item -ItemType directory -Path $env:Programfiles\Mezmo -Force
}

if(!(Test-Path -Path $env:Programfiles\Mezmo\logdna-agent.exe)){
  Copy-Item -Path $logdnaCLIPath $env:Programfiles\Mezmo\logdna-agent.exe -Force
}

if(!(Test-Path -Path $env:Programfiles\Mezmo\logdna-agent-svc.exe)){
  Copy-Item -Path $logdnaCLIPath $env:Programfiles\Mezmo\logdna-agent-svc.exe -Force
}

# Will use for MSI Install Later
# $packageArgs = @{
#   packageName   ='test-logdna'
#   fileType      = 'msi'
#   file= $logdnaCLIPath
#   softwareName  = 'test-logdna-agent*' #part or all of the Display Name as you see it in Programs and Features. It should be enough to be unique

#   checksum      = ''
#   checksumType  = 'sha256' #default is md5, can also be sha1, sha256 or sha512

#   # Uncomment matching EXE type (sorted by most to least common)
#   #silentArgs   = '/S'           # NSIS
#   #silentArgs   = '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP-' # Inno Setup
#   #silentArgs   = '/s'           # InstallShield
#   #silentArgs   = '/s /v"/qn"'   # InstallShield with MSI
#   #silentArgs   = '/s'           # Wise InstallMaster
#   #silentArgs   = '-s'           # Squirrel
#   #silentArgs   = '-q'           # Install4j
#   #silentArgs   = '-s'           # Ghost
#   # Note that some installers, in addition to the silentArgs above, may also need assistance of AHK to achieve silence.
#   #silentArgs   = ''             # none; make silent with input macro script like AutoHotKey (AHK)
#                                  #       https://community.chocolatey.org/packages/autohotkey.portable
#   silentArgs = "/qn /norestart MSIPROPERTY=`"true`""
#   validExitCodes= @(0, 1602, 3010, 1641) #please insert other valid exit codes here
# }

# $packageParams = Get-PackageParameters


#  For msi install
# Install-ChocolateyInstallPackage @packageArgs # https://docs.chocolatey.org/en-us/create/functions/install-chocolateyinstallpackage
