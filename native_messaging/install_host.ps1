# Registers OctoHeft as a Chromium Native Messaging Host
$HostJsonPath = Join-Path $PSScriptRoot "com.antigravity.octoheft.json"
$RegistryPathChrome = "HKCU:\Software\Google\Chrome\NativeMessagingHosts\com.antigravity.octoheft"
$RegistryPathEdge = "HKCU:\Software\Microsoft\Edge\NativeMessagingHosts\com.antigravity.octoheft"

New-Item -Path $RegistryPathChrome -Force | Out-Null
Set-ItemProperty -Path $RegistryPathChrome -Name "(Default)" -Value $HostJsonPath

New-Item -Path $RegistryPathEdge -Force | Out-Null
Set-ItemProperty -Path $RegistryPathEdge -Name "(Default)" -Value $HostJsonPath

Write-Host "OctoHeft Native Messaging Host registered successfully in Chrome and Edge registry."
