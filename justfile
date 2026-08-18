# QueenUI's single command surface for local development and CI.

# Avoid requiring Git Bash merely to use recipes on Windows.
set windows-shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-Command"]

default:
    @just --list

# Install exactly the JavaScript dependencies recorded in package-lock.json.
install:
    npm ci

# Run the browser UI with hot reload.
dev-web:
    npm run dev

# Run QueenUI as a native desktop application.
dev:
    npm run tauri -- dev

# Run frontend interaction tests.
test:
    npm test

# Format the frontend, scripts, recipes, and documentation.
format:
    npm run format

# Check formatting without modifying files.
format-check:
    npm run format:check

# Lint the TypeScript frontend.
lint:
    npm run lint

# Verify the real layout at QueenUI's supported desktop viewport classes.
[windows]
test-responsive:
    npm run test:responsive

# Stage the UI on C: and run the responsive suite in Microsoft Edge from WSL2.
[linux]
test-responsive:
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "$(wslpath -w scripts/wsl-windows-build.ps1)" -SourcePath "$(wslpath -w .)" -ResponsiveOnly

# Type-check and create the production frontend bundle.
frontend-build:
    npm run build

# Verify Rust formatting without modifying files.
rust-fmt:
    cargo fmt --manifest-path src-tauri/Cargo.toml --check
    cargo fmt --manifest-path crates/queen-runner/Cargo.toml --check
    cargo fmt --manifest-path crates/queen-client/Cargo.toml --check
    cargo fmt --manifest-path crates/queen-core/Cargo.toml --check
    cargo fmt --manifest-path crates/queen-protocol/Cargo.toml --check

# Type-check the native application.
rust-check:
    cargo check --manifest-path src-tauri/Cargo.toml

# Lint the native application, treating warnings as errors.
rust-clippy:
    cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
    cargo clippy --manifest-path crates/queen-runner/Cargo.toml --all-targets -- -D warnings
    cargo clippy --manifest-path crates/queen-client/Cargo.toml --all-targets -- -D warnings
    cargo clippy --manifest-path crates/queen-core/Cargo.toml --all-targets -- -D warnings
    cargo clippy --manifest-path crates/queen-protocol/Cargo.toml --all-targets -- -D warnings

# Run the native application's unit tests.
rust-test:
    cargo test --manifest-path src-tauri/Cargo.toml
    cargo test --manifest-path crates/queen-runner/Cargo.toml
    cargo test --manifest-path crates/queen-client/Cargo.toml
    cargo test --manifest-path crates/queen-core/Cargo.toml
    cargo test --manifest-path crates/queen-protocol/Cargo.toml

# Build the portable Linux/Windows runner without any Tauri desktop dependencies.
runner-build:
    cargo build --release --locked --manifest-path crates/queen-runner/Cargo.toml

# Run every fast verification step. Used locally and in CI.
check: format-check lint test frontend-build rust-fmt rust-clippy rust-test

# Reproduce the continuous-integration verification from a clean dependency install.
ci: install check

# Build both native Windows installer formats on a Windows host.
[windows]
package-windows:
    npm run tauri -- build --bundles nsis,msi --ci

# Full Windows CI: install dependencies, verify, and create the installers.
[windows]
ci-windows: ci test-responsive package-windows

# Build and open the NSIS installer on the current Windows machine.
[windows]
install-windows:
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/windows-install.ps1

# Silently install and launch-check an already built NSIS package. Used by CI.
[windows]
smoke-install-windows:
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/windows-install.ps1 -SkipBuild -Silent -SmokeTest

# Install/repair the native Windows build prerequisites from WSL2.
[linux]
wsl-windows-bootstrap:
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "$(wslpath -w scripts/wsl-windows-build.ps1)" -SourcePath "$(wslpath -w .)" -BootstrapOnly

# Stage the source on C: and build native Windows NSIS and MSI installers from WSL2.
[linux]
wsl-windows-build:
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "$(wslpath -w scripts/wsl-windows-build.ps1)" -SourcePath "$(wslpath -w .)"

# Run clippy on the Windows host toolchain from WSL2 (catches windows-only lint).
wsl-windows-clippy:
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "$(wslpath -w scripts/wsl-windows-build.ps1)" -SourcePath "$(wslpath -w .)" -ClippyOnly

# Build on Windows from WSL2 and open the native Windows installer.
[linux]
wsl-windows-install:
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "$(wslpath -w scripts/wsl-windows-build.ps1)" -SourcePath "$(wslpath -w .)" -Install

# Build, silently install, and smoke-launch QueenUI on Windows from WSL2.
[linux]
wsl-windows-smoke:
    powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "$(wslpath -w scripts/wsl-windows-build.ps1)" -SourcePath "$(wslpath -w .)" -Install -Silent -SmokeTest
