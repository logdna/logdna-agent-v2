param (
[string] $INGESTION_KEY,
[string] $INSTALLFOLDER,
[string] $DATAFOLDER
)
$ErrorActionPreference = 'Stop'

# set service recovery params
sc.exe failure MezmoAgentService reset= 60 actions= restart/15000/restart/15000/restart/15000


if (-not(Test-Path -Path "$DATAFOLDER\logdna.conf" -PathType Leaf)) {
  # read config template
  # replace key
  # save config to ProgramData folder
  New-Item -Force -Type Directory -Path "$DATAFOLDER"
  (Get-Content -Path "$INSTALLFOLDER\logdna.conf.sample") -Replace "<YOUR_INGESTION_KEY>", "$INGESTION_KEY" | Set-Content -Path "$DATAFOLDER\logdna.conf"
} else {
  # read config
  # replace key
  # save config back
  (Get-Content -Path "$DATAFOLDER\logdna.conf") -Replace "<YOUR_INGESTION_KEY>", "$INGESTION_KEY" | Set-Content -Path "$DATAFOLDER\logdna.conf"
}

Write-Host
