$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
if ($env:OS -ne "Windows_NT") { throw "Run this build on Windows with Visual Studio C++ Build Tools." }
$repo = $PSScriptRoot
$artifacts = Join-Path $repo "artifacts"
$logs = Join-Path $artifacts "windows-package"
$release = Join-Path $artifacts "releases"
New-Item -ItemType Directory -Force $logs,$release,(Join-Path $artifacts "tmp"),(Join-Path $artifacts "cargo-home") | Out-Null
$env:CARGO_HOME = Join-Path $artifacts "cargo-home"
$env:CARGO_TARGET_DIR = Join-Path $repo "rust\target"
$env:TEMP = Join-Path $artifacts "tmp"
$env:TMP = $env:TEMP
$env:CARGO_BUILD_JOBS = "8"
$env:RUSTFLAGS = "-C target-feature=+crt-static"
$cargo = (Get-Command cargo.exe).Source
Write-Output "Building Windows x64 release (20-minute limit)."
$build = Start-Process -FilePath $cargo -ArgumentList @("build","--release","--locked","--target","x86_64-pc-windows-msvc","--manifest-path","rust/Cargo.toml","--bin","vfx-launcher") -WorkingDirectory $repo -RedirectStandardOutput (Join-Path $logs "build.out") -RedirectStandardError (Join-Path $logs "build.err") -NoNewWindow -PassThru
$buildHandle = $build.Handle
if (-not $build.WaitForExit(1200000)) {
    & taskkill /PID $build.Id /T /F | Out-Null
    throw "Windows build exceeded 20 minutes."
}
Get-Content (Join-Path $logs "build.err") -Tail 40
if ($null -eq $build.ExitCode -or $build.ExitCode -ne 0) { throw "Windows build failed; see artifacts/windows-package/build.err." }
$manifest = Get-Content (Join-Path $repo "rust/Cargo.toml") -Raw
$versionMatch = [regex]::Match($manifest, '(?m)^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"\s*$')
if (-not $versionMatch.Success) { throw "Cannot read the Cargo package version." }
$version = $versionMatch.Groups[1].Value
$binary = Join-Path $env:CARGO_TARGET_DIR "x86_64-pc-windows-msvc\release\vfx-launcher.exe"
$check = Start-Process -FilePath $binary -ArgumentList "--version" -RedirectStandardOutput (Join-Path $logs "version.out") -RedirectStandardError (Join-Path $logs "version.err") -NoNewWindow -PassThru
$checkHandle = $check.Handle
if (-not $check.WaitForExit(20000)) { $check.Kill(); throw "Packaged executable did not return its version." }
if ($check.ExitCode -ne 0 -or (Get-Content (Join-Path $logs "version.out") -Raw).Trim() -ne "VFX Launcher $version") { throw "Executable version check failed." }
$staging = Join-Path $artifacts ("windows-appdir-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory $staging | Out-Null
Copy-Item $binary (Join-Path $staging "vfx-launcher.exe")
Copy-Item (Join-Path $repo "docs/WINDOWS-PREVIEW.md") (Join-Path $staging "README-Windows.md")
Copy-Item (Join-Path $repo "docs/RELEASE-0.2.0.md") (Join-Path $staging "RELEASE.md")
$source = & git -C $repo rev-parse HEAD
if ($LASTEXITCODE -ne 0) { throw "Cannot record the source commit." }
$dirty = @(& git -C $repo status --porcelain).Count -gt 0
@("VFX Launcher $version Windows x64 preview", "Source commit: $source", "Uncommitted source changes: $dirty", "CRT: statically linked", "File operation execution: unsupported on Windows") | Set-Content (Join-Path $staging "BUILD-INFO.txt") -Encoding UTF8
$archive = Join-Path $release "VFX-Launcher-$version-Windows-x86_64-preview.zip"
Compress-Archive -Path (Join-Path $staging "*") -DestinationPath $archive -Force
$hash = (Get-FileHash $archive -Algorithm SHA256).Hash.ToLowerInvariant()
"$hash  $(Split-Path $archive -Leaf)" | Set-Content "$archive.sha256" -Encoding ASCII
Write-Output ("Windows preview: " + $archive)
