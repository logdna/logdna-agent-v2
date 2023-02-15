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
  (Get-Content -Path "$INSTALLFOLDER\logdna.conf.templ") -Replace "<YOUR_INGESTION_KEY>", "$INGESTION_KEY" | Set-Content -Path "$DATAFOLDER\logdna.conf"
} else {
  # backup config
  $TS = Get-Date -format "yyyy_MM_dd_hh_mm_ss"
  copy-item "$DATAFOLDER\logdna.conf" -destination "$DATAFOLDER\logdna.conf.$TS"
  # read config template
  # replace key
  # save config back
  (Get-Content -Path "$INSTALLFOLDER\logdna.conf.templ") -Replace "<YOUR_INGESTION_KEY>", "$INGESTION_KEY" | Set-Content -Path "$DATAFOLDER\logdna.conf"
}
Remove-Item  "$INSTALLFOLDER\logdna.conf.templ" -Force -Confirm:$false
# prefetch Mezmo CA cert
try { $null=iwr https://www.mezmo.com -UseBasicParsing } catch {}
Start-Sleep 1

Write-Host
