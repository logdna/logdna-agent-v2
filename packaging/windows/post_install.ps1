param (
[string] $INGESTION_KEY,
[string] $DATAFOLDER
)
$ErrorActionPreference = 'Stop'
sc.exe failure MezmoAgentService reset= 60 actions= restart/15000/restart/15000/restart/15000
echo $LASTEXITCODE

echo $INGESTION_KEY
echo $DATAFOLDER

Write-Host

Start-Sleep 20



