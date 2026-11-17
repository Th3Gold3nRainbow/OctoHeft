# Registers OctoHeft as a Chromium Native Messaging Host
$HostJsonPath = Join-Path $PSScriptRoot "com.octoheft.downloader.json"
$RegistryPathChrome = "HKCU:\Software\Google\Chrome\NativeMessagingHosts\com.octoheft.downloader"
$RegistryPathEdge = "HKCU:\Software\Microsoft\Edge\NativeMessagingHosts\com.octoheft.downloader"

New-Item -Path $RegistryPathChrome -Force | Out-Null
Set-ItemProperty -Path $RegistryPathChrome -Name "(Default)" -Value $HostJsonPath

New-Item -Path $RegistryPathEdge -Force | Out-Null
Set-ItemProperty -Path $RegistryPathEdge -Name "(Default)" -Value $HostJsonPath

Write-Host "OctoHeft Native Messaging Host registered successfully in Chrome and Edge registry."
