[CmdletBinding()]
param(
    [switch]$SkipBuild,
    [switch]$Silent,
    [switch]$SmokeTest
)

$ErrorActionPreference = "Stop"
$projectRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $projectRoot

function Invoke-Checked {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Command,
        [Parameter(ValueFromRemainingArguments = $true)]
        [string[]]$CommandArguments
    )

    & $Command @CommandArguments
    if ($LASTEXITCODE -ne 0) {
        throw "Command failed with exit code ${LASTEXITCODE}: $Command $CommandArguments"
    }
}

if (-not $SkipBuild) {
    Write-Host "Building the QueenUI NSIS installer on Windows..."
    # The "--" must be passed inside an explicit argument array: PowerShell's
    # parameter binder otherwise consumes a bare "--", so npm would never see it
    # and would swallow "--bundles nsis --ci" as npm-config flags.
    Invoke-Checked npm.cmd @("run", "tauri", "--", "build", "--bundles", "nsis", "--ci")
}

$bundleDirectory = Join-Path $projectRoot "src-tauri\target\release\bundle\nsis"
$installer = Get-ChildItem -Path $bundleDirectory -Filter "*-setup.exe" -File |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1

if (-not $installer) {
    throw "No NSIS installer was found in $bundleDirectory. Run 'just package-windows' first."
}

Write-Host "Installing from $($installer.FullName)"
if ($Silent) {
    $installerProcess = Start-Process -FilePath $installer.FullName -ArgumentList "/S" -Wait -PassThru
}
else {
    $installerProcess = Start-Process -FilePath $installer.FullName -Wait -PassThru
}

if ($installerProcess.ExitCode -ne 0) {
    throw "The QueenUI installer exited with code $($installerProcess.ExitCode)."
}

if (-not $SmokeTest) {
    Write-Host "Installer completed. Launch QueenUI from the Start menu to begin testing."
    exit 0
}

$candidateExecutables = @(
    (Join-Path $env:LOCALAPPDATA "QueenUI\queenui.exe"),
    (Join-Path $env:LOCALAPPDATA "Programs\QueenUI\queenui.exe"),
    (Join-Path $env:ProgramFiles "QueenUI\queenui.exe")
)

$installedExecutable = $candidateExecutables |
    Where-Object { Test-Path $_ } |
    Select-Object -First 1

if (-not $installedExecutable) {
    $installedExecutable = Get-ChildItem -Path $env:LOCALAPPDATA -Filter "queenui.exe" -File -Recurse -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -notlike "*\Temp\*" } |
        Select-Object -ExpandProperty FullName -First 1
}

if (-not $installedExecutable) {
    throw "The installer succeeded, but the installed QueenUI executable could not be located."
}

Write-Host "Smoke-launching installed application: $installedExecutable"
$applicationProcess = Start-Process -FilePath $installedExecutable -PassThru

try {
    # Wait for a visible main window instead of a fixed sleep: it both catches
    # early crashes and confirms the UI actually came up on slow runners.
    $deadline = (Get-Date).AddSeconds(30)
    $windowSeen = $false
    while ((Get-Date) -lt $deadline) {
        if ($applicationProcess.HasExited) {
            throw "The installed application exited unexpectedly with code $($applicationProcess.ExitCode)."
        }
        $applicationProcess.Refresh()
        if ($applicationProcess.MainWindowHandle -ne 0) {
            $windowSeen = $true
            break
        }
        Start-Sleep -Milliseconds 500
    }
    if (-not $windowSeen) {
        throw "The installed application never presented a main window within 30 seconds."
    }
    Write-Host "Installed QueenUI process presented its main window during the smoke test."
}
finally {
    if (-not $applicationProcess.HasExited) {
        Stop-Process -Id $applicationProcess.Id -Force
    }
}
