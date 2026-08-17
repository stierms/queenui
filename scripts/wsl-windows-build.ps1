[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$SourcePath,
    [switch]$BootstrapOnly,
    [switch]$ResponsiveOnly,
    [switch]$Install,
    [switch]$Silent,
    [switch]$SmokeTest
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")

# Single-source the toolchain versions from the repository's pin files.
$rustVersion = (Select-String -Path (Join-Path $repoRoot "rust-toolchain.toml") -Pattern 'channel\s*=\s*"(.+)"' |
    Select-Object -First 1).Matches.Groups[1].Value
$nodeVersion = (Get-Content (Join-Path $repoRoot ".node-version") -First 1).Trim()
$windowsTarget = "x86_64-pc-windows-msvc"
$stagePath = Join-Path $env:LOCALAPPDATA "QueenUI\wsl-build"

# SHA256 of nodejs.org's node-v<version>-win-x64.zip; extend when .node-version moves.
$nodeArchiveHashes = @{
    "24.18.0" = "0ae68406b42d7725661da979b1403ec9926da205c6770827f33aac9d8f26e821"
}
$nodeArchiveHash = $nodeArchiveHashes[$nodeVersion]
if (-not $nodeArchiveHash) {
    throw "No pinned SHA256 for Node.js $nodeVersion. Add it to `$nodeArchiveHashes in $PSCommandPath."
}
$toolchainPath = Join-Path $env:LOCALAPPDATA "QueenUI\toolchains"
$portableNodePath = Join-Path $toolchainPath "node-v$nodeVersion-win-x64"

function Refresh-ProcessPath {
    $machinePath = [Environment]::GetEnvironmentVariable("Path", "Machine")
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $env:Path = "$machinePath;$userPath"
    # A refresh rebuilds Path from the registry, which would silently drop the
    # portable Node.js entry added during this bootstrap run.
    if (Test-Path (Join-Path $portableNodePath "node.exe")) {
        $env:Path = "$portableNodePath;$env:Path"
    }
}

function Invoke-NativeCommand {
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

function Install-WindowsPrerequisites {
    Refresh-ProcessPath

    if (-not (Get-Command winget.exe -ErrorAction SilentlyContinue)) {
        throw "Windows Package Manager (winget) is required to bootstrap the native Windows toolchain."
    }

    if (-not (Get-Command node.exe -ErrorAction SilentlyContinue) -and -not (Test-Path (Join-Path $portableNodePath "node.exe"))) {
        Write-Host "Installing verified portable Windows Node.js $nodeVersion..." -ForegroundColor Cyan
        New-Item -ItemType Directory -Force -Path $toolchainPath | Out-Null
        $nodeArchive = Join-Path $env:TEMP "node-v$nodeVersion-win-x64.zip"
        $nodeUri = "https://nodejs.org/dist/v$nodeVersion/node-v$nodeVersion-win-x64.zip"
        Invoke-WebRequest -Uri $nodeUri -OutFile $nodeArchive

        $actualHash = (Get-FileHash -Path $nodeArchive -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actualHash -ne $nodeArchiveHash) {
            Remove-Item -Path $nodeArchive -Force
            throw "The downloaded Node.js archive failed SHA256 verification."
        }

        Expand-Archive -Path $nodeArchive -DestinationPath $toolchainPath -Force
        Remove-Item -Path $nodeArchive -Force
    }

    if (Test-Path (Join-Path $portableNodePath "node.exe")) {
        $env:Path = "$portableNodePath;$env:Path"
    }

    if (-not (Get-Command npm.cmd -ErrorAction SilentlyContinue)) {
        throw "Native Windows npm.cmd is not available after bootstrapping Node.js."
    }

    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    $buildTools = $null
    if (Test-Path $vswhere) {
        $buildTools = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    }

    if (-not $buildTools) {
        Write-Host "Installing Visual Studio 2022 C++ Build Tools..." -ForegroundColor Cyan
        Invoke-NativeCommand winget.exe install --id Microsoft.VisualStudio.2022.BuildTools --exact --silent --accept-source-agreements --accept-package-agreements --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
    }

    if (-not (Get-Command rustup.exe -ErrorAction SilentlyContinue)) {
        Write-Host "Installing rustup for Windows..." -ForegroundColor Cyan
        Invoke-NativeCommand winget.exe install --id Rustlang.Rustup --exact --silent --accept-source-agreements --accept-package-agreements --disable-interactivity
        Refresh-ProcessPath
    }

    if (-not (Get-Command rustup.exe -ErrorAction SilentlyContinue)) {
        throw "rustup.exe is not available after installation. Open a new terminal and retry."
    }

    Write-Host "Installing native Rust $rustVersion for $windowsTarget..." -ForegroundColor Cyan
    Invoke-NativeCommand rustup.exe toolchain install "$rustVersion-$windowsTarget" --profile minimal --component rustfmt

    Write-Host "Windows toolchain ready:" -ForegroundColor Green
    & node.exe --version
    & npm.cmd --version
    & rustup.exe run "$rustVersion-$windowsTarget" rustc.exe --version
    Write-Host "MSVC Build Tools: $buildTools"
}

function Stage-SourceTree {
    Write-Host "Staging WSL source on the Windows filesystem at $stagePath..." -ForegroundColor Cyan
    New-Item -ItemType Directory -Force -Path $stagePath | Out-Null

    $excludedDirectories = @(
        ".git",
        ".idea",
        ".vscode",
        "artifacts",
        "dist",
        "node_modules",
        "target"
    )
    $robocopyArguments = @(
        $SourcePath,
        $stagePath,
        "/MIR",
        "/R:2",
        "/W:1",
        "/NFL",
        "/NDL",
        "/NJH",
        "/NJS",
        "/NP",
        "/XD"
    ) + $excludedDirectories

    & robocopy.exe @robocopyArguments
    if ($LASTEXITCODE -gt 7) {
        throw "robocopy failed with exit code $LASTEXITCODE."
    }
}

function Copy-WindowsArtifacts {
    $artifactPath = Join-Path $SourcePath "artifacts\windows"
    New-Item -ItemType Directory -Force -Path $artifactPath | Out-Null
    Get-ChildItem -Path $artifactPath -File -ErrorAction SilentlyContinue | Remove-Item -Force

    $bundleRoot = Join-Path $stagePath "src-tauri\target\release\bundle"
    $installers = @(
        Get-ChildItem -Path (Join-Path $bundleRoot "nsis") -Filter "*-setup.exe" -File -ErrorAction SilentlyContinue
        Get-ChildItem -Path (Join-Path $bundleRoot "msi") -Filter "*.msi" -File -ErrorAction SilentlyContinue
    )

    if (-not $installers) {
        throw "The native build finished without producing Windows installers."
    }

    foreach ($installer in $installers) {
        Copy-Item -Path $installer.FullName -Destination $artifactPath -Force
        Write-Host "Created $(Join-Path $artifactPath $installer.Name)" -ForegroundColor Green
    }
}

function Find-InstalledExecutable {
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
            Where-Object { $_.FullName -notlike "*\Temp\*" -and $_.FullName -notlike "$stagePath*" } |
            Select-Object -ExpandProperty FullName -First 1
    }

    return $installedExecutable
}

Install-WindowsPrerequisites
if ($BootstrapOnly) {
    exit 0
}

Stage-SourceTree
Set-Location $stagePath

Write-Host "Installing dependencies with native Windows npm..." -ForegroundColor Cyan
Invoke-NativeCommand -Command "npm.cmd" -CommandArguments @("ci")

if ($ResponsiveOnly) {
    Write-Host "Running responsive layout tests in Microsoft Edge..." -ForegroundColor Cyan
    Invoke-NativeCommand -Command "npm.cmd" -CommandArguments @("run", "test:responsive")
    exit 0
}

Write-Host "Building QueenUI with native Windows Node, Rust, and MSVC..." -ForegroundColor Cyan
Invoke-NativeCommand -Command "npm.cmd" -CommandArguments @("run", "tauri", "--", "build", "--bundles", "nsis,msi", "--ci")
Copy-WindowsArtifacts

if (-not $Install) {
    exit 0
}

$nsisInstaller = Get-ChildItem -Path (Join-Path $stagePath "src-tauri\target\release\bundle\nsis") -Filter "*-setup.exe" -File |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1

if (-not $nsisInstaller) {
    throw "No NSIS installer is available to install."
}

Write-Host "Installing native Windows package $($nsisInstaller.Name)..." -ForegroundColor Cyan
if ($Silent) {
    $installerProcess = Start-Process -FilePath $nsisInstaller.FullName -ArgumentList "/S" -Wait -PassThru
}
else {
    $installerProcess = Start-Process -FilePath $nsisInstaller.FullName -Wait -PassThru
}

if ($installerProcess.ExitCode -ne 0) {
    throw "The Windows installer exited with code $($installerProcess.ExitCode)."
}

if (-not $SmokeTest) {
    Write-Host "QueenUI is installed. Launch it from the Windows Start menu." -ForegroundColor Green
    exit 0
}

$installedExecutable = Find-InstalledExecutable
if (-not $installedExecutable) {
    throw "QueenUI installed successfully, but its executable could not be located."
}

Write-Host "Smoke-launching installed QueenUI at $installedExecutable..." -ForegroundColor Cyan
$applicationProcess = Start-Process -FilePath $installedExecutable -PassThru
try {
    Start-Sleep -Seconds 5
    if ($applicationProcess.HasExited) {
        throw "The installed application exited unexpectedly with code $($applicationProcess.ExitCode)."
    }
    Write-Host "Native Windows install smoke test passed." -ForegroundColor Green
}
finally {
    if (-not $applicationProcess.HasExited) {
        Stop-Process -Id $applicationProcess.Id -Force
    }
}
