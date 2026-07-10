# Stage binaries from a cargo-dist Windows archive for WiX MSI packaging.
# Expects env vars: DIST_EXTRACTED (path to extracted archive root)
$ErrorActionPreference = 'Stop'

$distExtracted = if ($env:DIST_EXTRACTED) { $env:DIST_EXTRACTED } else { "dist_extracted" }
$stagingDir = "wix\msi\staging"
New-Item -ItemType Directory -Force -Path $stagingDir | Out-Null

# Find and copy the main executable
$exe = Get-ChildItem -Path $distExtracted -Filter "gemacast-pc.exe" -Recurse | Select-Object -First 1
if (-not $exe) {
    Write-Error "gemacast-pc.exe not found in extracted archive"
    exit 1
}
Copy-Item $exe.FullName -Destination $stagingDir
Write-Host "Staged: gemacast-pc.exe"

# Copy ADB binaries (bundled by cargo-dist alongside the executable)
foreach ($fileName in @("adb.exe", "AdbWinApi.dll", "AdbWinUsbApi.dll")) {
    $found = Get-ChildItem -Path $distExtracted -Filter $fileName -Recurse | Select-Object -First 1
    if ($found) {
        Copy-Item $found.FullName -Destination $stagingDir
        Write-Host "Staged: $fileName"
    } else {
        Write-Error "$fileName not found in archive — crashing build!"
        exit 1
    }
}

Write-Host "`nStaging complete. Contents:"
Get-ChildItem $stagingDir | Format-Table Name, Length
