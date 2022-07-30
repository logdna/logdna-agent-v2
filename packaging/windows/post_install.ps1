$ErrorActionPreference = 'Inquire'
sc.exe failure MezmoAgentService reset= 60 actions= restart/15000/restart/15000/restart/15000
echo $LASTEXITCODE
Write-Host
