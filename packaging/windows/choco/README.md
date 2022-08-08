# **Publishing To Chocolatey**

## Update files
**IMPORTANT:** Before publishing anything make sure to update these files
1. Make sure that all information in `logdna-agenn.nuspec` (manifest of package) is still correct. 
2. If new installation process is needed update `tools/chocolateyinstall.ps1` 
3. If process of uninstall has changed update `tools/chocolateybeforemodify.ps1`
4. Update `tools/LICENSE.txt` if the license of the app has changed
5. **REQUIRED**: Every time there is a change to the app, we need to update the checksum in `tools/VERIFICATION.txt`

## Publish Using Script
1. Launch Powershell
2. Navigate to `C:\{PROJECT LOCATION}\windows\choco`
3. Set the api key to the $CHOCO_API_KEY environment variable
4. Run 
```
.\publish_to_choco.ps1
```

## Publish Manual Process
1. Launch Powershell
2. Navigate to `C:\{PROJECT LOCATION}\windows\choco`
3. Pack package with the name in your `.nuspec` file : 
```
choco pack
``` 
4. Login into Choco: 
```
choco apikey --key {API KEY IN 1PASSWORD} --push https://push.chocolatey.org
```
5. Push package to Choco: 
```
choco push {Package Name}.nupkg --source https://push.chocolatey.org
```

## Folder Structure
```
choco
│--- logdna-agent.nuspec [Manifest for Package]
│--- tools
    |--- chocolateybeforemodefy.ps1 [Script that is run before an update and uninstall]
    |--- chocolateyinstall.ps1 [Script that is run for installing application]
    |--- LICENSE.txt [Basic license]
    |--- VERIFICATIOn.txt [Location of Checksum that choco uses to verify the application installed is the one that is on choco]
```
