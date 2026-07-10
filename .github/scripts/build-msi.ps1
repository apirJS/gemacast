# Build MSI installer using WiX v6.
# Expects env vars: VERSION
$ErrorActionPreference = 'Stop'

$version = $env:VERSION
if (-not $version) {
    Write-Error "VERSION environment variable is required"
    exit 1
}

$sourceDir = (Resolve-Path "wix\msi\staging").Path
$iconDir = (Resolve-Path "wix\assets").Path

Write-Host "Building MSI with Version=$version"
Write-Host "  SourceDir=$sourceDir"
Write-Host "  IconDir=$iconDir"

dotnet build wix\msi\msi.wixproj `
    -c Release `
    "-p:Version=$version" `
    "-p:SourceDir=$sourceDir" `
    "-p:IconDir=$iconDir"

if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Verify MSI was produced
$msi = Get-ChildItem -Path wix\msi\bin -Filter "gemacast-pc-x86_64-pc-windows-msvc.msi" -Recurse | Select-Object -First 1
if (-not $msi) {
    Write-Error "gemacast-pc-x86_64-pc-windows-msvc.msi was not produced by the build"
    exit 1
}
Write-Host "MSI built successfully: $($msi.FullName) ($([math]::Round($msi.Length / 1MB, 2)) MB)"
echo "MSI_PATH=$($msi.FullName)" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append
