cmd.exe /c choco pack
cmd.exe /c choco apikey --key $env:CHOCO_API_KEY --push https://push.chocolatey.org
cmd.exe /c choco push *.nupkg --source https://push.chocolatey.org
