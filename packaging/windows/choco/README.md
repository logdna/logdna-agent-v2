# Mezmo Agent for Windows

## Build Choco package on Linux
1. build MSI file in <repo>/packaging/windows/msi
2. run
```
export BUILD_NUMBER=1
./mk
```


## Publish Choco package on Linux
1. run 
```
export CHOCO_API_KEY=xxxxx
.\publish_to_choco
```
