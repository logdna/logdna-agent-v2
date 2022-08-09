# This runs in 0.9.10+ before upgrade and uninstall.
# Use this file to do things like stop services prior to upgrade or uninstall.
# NOTE: It is an anti-pattern to call chocolateyUninstall.ps1 from here. If you
#  need to uninstall an MSI prior to upgrade, put the functionality in this
#  file without calling the uninstall script. Make it idempotent in the
#  uninstall script so that it doesn't fail when it is already uninstalled.
# NOTE: For upgrades - like the uninstall script, this script always runs from
#  the currently installed version, not from the new upgraded package version.


IF(Get-Service -Name 'logdna-agent'){
    Stop-Service -Name 'logdna-agent'
}

$registryPath = "HKLM:\SYSTEM\CurrentControlSet\Services\logdna-agent"

IF(Test-Path $registryPath)
{
    Remove-Item -Path $registryPath -Force -Recurse
}



if(!(Test-Path -Path $env:Programfiles\Mezmo\logdna-agent.exe)){
  Remove-Item -Path  $env:Programfiles\Mezmo\logdna-agent.exe -Force
}

if(!(Test-Path -Path $env:Programfiles\Mezmo\logdna-agent-svc.exe)){
  Remove-Item -Path $env:Programfiles\Mezmo\logdna-agent-svc.exe -Force
}
