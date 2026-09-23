$ErrorActionPreference = 'Stop'

$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

Write-Host 'Starting Linux and Android container tests in parallel...'

$linux = Start-Job -Name gemacast-linux -ScriptBlock {
    param($repo)
    Set-Location $repo
    docker compose -f docker/compose.yaml run --rm test
    if ($LASTEXITCODE -ne 0) { throw "Linux container exited with $LASTEXITCODE" }
} -ArgumentList $root

$android = Start-Job -Name gemacast-android -ScriptBlock {
    param($repo)
    Set-Location $repo
    docker compose -f docker/compose.yaml --profile android run --rm android
    if ($LASTEXITCODE -ne 0) { throw "Android container exited with $LASTEXITCODE" }
} -ArgumentList $root

Wait-Job -Job $linux, $android | Out-Null
$linuxOutput = Receive-Job -Job $linux -Keep
$androidOutput = Receive-Job -Job $android -Keep

Write-Host "`n=== Linux output ==="
$linuxOutput | Write-Host
Write-Host "`n=== Android output ==="
$androidOutput | Write-Host

$failed = @(@($linux, $android) | Where-Object State -eq 'Failed')
Remove-Job -Job $linux, $android -Force

if ($failed.Count -gt 0) {
    Write-Error ("Failed container jobs: " + (($failed | ForEach-Object Name) -join ', '))
    exit 1
}

Write-Host "`nLinux and Android container tests passed."
