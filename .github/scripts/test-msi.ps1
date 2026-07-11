# Full MSI installer test: install → verify → smoke test → uninstall → verify cleanup.
# Expects env var: MSI_PATH (absolute path to the .msi file)
$ErrorActionPreference = 'Stop'

$msiPath = $env:MSI_PATH
if (-not $msiPath -or -not (Test-Path $msiPath)) {
    Write-Error "MSI_PATH is not set or file does not exist: $msiPath"
    exit 1
}

# ── Install ────────────────────────────────────────────────────────────────
Write-Host "=== Installing MSI ==="
$proc = Start-Process msiexec -ArgumentList "/i", "`"$msiPath`"", "/qn", "/norestart" -Wait -PassThru
if ($proc.ExitCode -ne 0) {
    Write-Error "MSI install failed with exit code $($proc.ExitCode)"
    exit 1
}
Write-Host "MSI installed successfully"

# ── Verify Installation ───────────────────────────────────────────────────
Write-Host "`n=== Verifying Installation ==="
$installDir = "$env:ProgramFiles\Gemacast"
$errors = @()

# Check main executable
if (-not (Test-Path "$installDir\gemacast-pc.exe")) {
    $errors += "gemacast-pc.exe not found in $installDir"
} else {
    Write-Host "PASS: gemacast-pc.exe exists"
}

# Check ADB binaries
foreach ($f in @("adb.exe", "AdbWinApi.dll", "AdbWinUsbApi.dll")) {
    if (-not (Test-Path "$installDir\$f")) {
        $errors += "$f not found in $installDir"
    } else {
        Write-Host "PASS: $f exists"
    }
}

# Check firewall rules
$tcpRule = netsh advfirewall firewall show rule name="Gemacast (TCP)" 2>&1
if ($tcpRule -match "Gemacast") {
    Write-Host "PASS: TCP firewall rule exists"
} else {
    $errors += "TCP firewall rule 'Gemacast (TCP)' not found"
}

$udpRule = netsh advfirewall firewall show rule name="Gemacast (UDP)" 2>&1
if ($udpRule -match "Gemacast") {
    Write-Host "PASS: UDP firewall rule exists"
} else {
    $errors += "UDP firewall rule 'Gemacast (UDP)' not found"
}

# Check Start Menu shortcut
$startMenu = "$env:ProgramData\Microsoft\Windows\Start Menu\Programs\Gemacast.lnk"
if (Test-Path $startMenu) {
    Write-Host "PASS: Start Menu shortcut exists"
} else {
    # Try alternative location
    $altMenu = Get-ChildItem "$env:ProgramData\Microsoft\Windows\Start Menu\Programs" -Filter "*Gemacast*" -Recurse -ErrorAction SilentlyContinue
    if ($altMenu) {
        Write-Host "PASS: Start Menu shortcut found at $($altMenu.FullName)"
    } else {
        $errors += "Start Menu shortcut not found"
    }
}

# Smoke test: start the process briefly and verify it doesn't crash
Write-Host "`nSmoke test: starting gemacast-pc.exe..."
$proc = Start-Process -FilePath "$installDir\gemacast-pc.exe" -PassThru
Start-Sleep -Seconds 5

if ($proc.HasExited) {
    $errors += "gemacast-pc.exe exited prematurely with code $($proc.ExitCode)"
} else {
    Write-Host "PASS: gemacast-pc.exe is running (PID $($proc.Id))"
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Write-Host "Process killed after smoke test"
}

if ($errors.Count -gt 0) {
    Write-Host "`n=== INSTALLATION FAILURES ==="
    $errors | ForEach-Object { Write-Host "FAIL: $_" }
    exit 1
}
Write-Host "`nAll installation checks passed"

# ── Uninstall ──────────────────────────────────────────────────────────────
Write-Host "`n=== Uninstalling MSI ==="
$proc = Start-Process msiexec -ArgumentList "/x", "`"$msiPath`"", "/qn", "/norestart" -Wait -PassThru
if ($proc.ExitCode -ne 0) {
    Write-Error "MSI uninstall failed with exit code $($proc.ExitCode)"
    exit 1
}
Write-Host "MSI uninstalled successfully"

# ── Verify Uninstall ──────────────────────────────────────────────────────
Write-Host "`n=== Verifying Uninstall Cleanup ==="
$errors = @()

# Installation directory should be removed
if (Test-Path $installDir) {
    $remaining = Get-ChildItem $installDir -Recurse
    if ($remaining.Count -gt 0) {
        $errors += "Installation directory still contains files: $($remaining.Name -join ', ')"
    } else {
        Write-Host "PASS: Installation directory is empty (will be removed)"
    }
} else {
    Write-Host "PASS: Installation directory removed"
}

# Firewall rules should be cleaned up
$tcpRule = netsh advfirewall firewall show rule name="Gemacast (TCP)" 2>&1
if ($tcpRule -match "No rules match") {
    Write-Host "PASS: TCP firewall rule removed"
} else {
    $errors += "TCP firewall rule still exists after uninstall"
}

$udpRule = netsh advfirewall firewall show rule name="Gemacast (UDP)" 2>&1
if ($udpRule -match "No rules match") {
    Write-Host "PASS: UDP firewall rule removed"
} else {
    $errors += "UDP firewall rule still exists after uninstall"
}

if ($errors.Count -gt 0) {
    Write-Host "`n=== UNINSTALL FAILURES ==="
    $errors | ForEach-Object { Write-Host "FAIL: $_" }
    exit 1
}
Write-Host "`nAll uninstall checks passed"
