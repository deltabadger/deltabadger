# B13: Tauri's bundle.windows.signCommand on release.yml's signing Windows leg. Authenticode-signs one file through
# Azure Artifact Signing, timestamped. Logs in with a fresh GitHub OIDC token on every call: Tauri signs half an hour
# into the build, after the token azure/login exchanged at the start has expired.
# Each result goes to $RUNNER_TEMP/signed.txt, which release.yml checks: NSIS ignores the signer's exit code for the
# uninstaller, so a failure there would otherwise ship an unsigned uninstaller inside a signed installer.
param([Parameter(Mandatory)][string]$File)
$ErrorActionPreference = 'Stop'
$ledger = Join-Path $env:RUNNER_TEMP signed.txt
"FAIL $File" | Add-Content $ledger

$oidc = Invoke-RestMethod -Headers @{ Authorization = "Bearer $env:ACTIONS_ID_TOKEN_REQUEST_TOKEN" } `
  -Uri "$env:ACTIONS_ID_TOKEN_REQUEST_URL&audience=api://AzureADTokenExchange"
az login --service-principal --username $env:AZURE_CLIENT_ID --tenant $env:AZURE_TENANT_ID `
  --federated-token $oidc.value --allow-no-subscriptions --output none
if ($LASTEXITCODE) { exit $LASTEXITCODE }

& $env:SIGNTOOL sign /v /fd SHA256 /tr http://timestamp.acs.microsoft.com /td SHA256 `
  /dlib $env:ARTIFACT_SIGNING_DLIB /dmdf $env:ARTIFACT_SIGNING_METADATA /d Deltabadger $File
if ($LASTEXITCODE) { exit $LASTEXITCODE }
(Get-Content $ledger) -replace "^FAIL $([regex]::Escape($File))$", "ok $File" | Set-Content $ledger
