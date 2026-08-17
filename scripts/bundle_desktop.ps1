param(
    [string]$Architecture = $env:TAURI_ENV_ARCH
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RubyVersion = '3.4.8'
$RubyInstallerVersion = '3.4.8-1'
$RubyInstallerSha256 = 'D1C3BA83AE748C08E35E0B1D9939D45DBCA7925E0A8BF84A42860BF19847E0D6'
$RubyInstallerUrl = "https://github.com/oneclick/rubyinstaller2/releases/download/RubyInstaller-$RubyInstallerVersion/rubyinstaller-$RubyInstallerVersion-x64.7z"

$RepoDir = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$BundleDir = [IO.Path]::GetFullPath((Join-Path $RepoDir '_bundle'))
$AppDir = Join-Path $BundleDir 'app'
$RubyDir = Join-Path $BundleDir 'ruby'
$ExpectedBundleDir = [IO.Path]::GetFullPath((Join-Path $RepoDir '_bundle'))

function Assert-LastExitCode([string]$Description) {
    if ($LASTEXITCODE -ne 0) {
        throw "$Description failed with exit code $LASTEXITCODE"
    }
}

function Require-Command([string]$Name) {
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "desktop bundle failed: required command not found: $Name"
    }
}

if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
    throw 'desktop bundle failed: Windows is required'
}

if ([string]::IsNullOrWhiteSpace($Architecture)) {
    $Architecture = 'x64'
}
switch ($Architecture.ToLowerInvariant()) {
    { $_ -in 'x64', 'x86_64', 'amd64' } { $TargetArchitecture = 'x64'; break }
    default { throw "desktop bundle failed: unsupported target architecture: $Architecture" }
}

$HostArchitecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
if ($HostArchitecture -ne 'X64' -or $TargetArchitecture -ne 'x64') {
    throw "desktop bundle failed: the x64 RubyInstaller bundle requires native x64 Windows (host: $HostArchitecture, target: $TargetArchitecture)"
}

foreach ($CommandName in '7z', 'npm') {
    Require-Command $CommandName
}

$RequiredRuby = (Get-Content (Join-Path $RepoDir '.ruby-version') -Raw).Trim()
if ($RequiredRuby -ne $RubyVersion) {
    throw "desktop bundle failed: .ruby-version does not contain Ruby $RubyVersion"
}
$Gemfile = Get-Content (Join-Path $RepoDir 'Gemfile') -Raw
$RubyRequirementPattern = '(?m)^ruby [''"]{0}[''"]$' -f [regex]::Escape($RubyVersion)
if ($Gemfile -notmatch $RubyRequirementPattern) {
    throw "desktop bundle failed: Gemfile does not require Ruby $RubyVersion"
}
$Lockfile = Get-Content (Join-Path $RepoDir 'Gemfile.lock') -Raw
if ($Lockfile -notmatch "(?m)^  ruby $([regex]::Escape($RubyVersion))$") {
    throw "desktop bundle failed: Gemfile.lock does not lock Ruby $RubyVersion"
}
if ($Lockfile -notmatch '(?m)^  x64-mingw-ucrt$') {
    throw 'desktop bundle failed: Gemfile.lock does not contain the x64-mingw-ucrt platform'
}
if ($BundleDir -ne $ExpectedBundleDir) {
    throw "desktop bundle failed: refusing to replace unexpected bundle path: $BundleDir"
}

Write-Host 'Building frontend assets...'
Push-Location $RepoDir
try {
    & npm run build
    Assert-LastExitCode 'frontend build'
} finally {
    Pop-Location
}

# _bundle is generated output. Rebuild it from scratch so stale gems or assets cannot leak
# from an earlier build. Unlike macOS, Windows does not carry rbsecp256k1, so native x64
# builds are allowed without the macOS host-gem cross-compilation restriction.
if (Test-Path $BundleDir) {
    Remove-Item -Recurse -Force $BundleDir
}
New-Item -ItemType Directory -Force -Path $BundleDir | Out-Null

$TemporaryDir = Join-Path ([IO.Path]::GetTempPath()) ("deltabadger-rubyinstaller-" + [guid]::NewGuid())
$ArchivePath = Join-Path $TemporaryDir 'rubyinstaller.7z'
$ExtractDir = Join-Path $TemporaryDir 'extract'
New-Item -ItemType Directory -Force -Path $ExtractDir | Out-Null

try {
    Write-Host "Downloading RubyInstaller $RubyInstallerVersion..."
    Invoke-WebRequest -UseBasicParsing -Uri $RubyInstallerUrl -OutFile $ArchivePath
    $ActualHash = (Get-FileHash -Algorithm SHA256 $ArchivePath).Hash
    if ($ActualHash -ne $RubyInstallerSha256) {
        throw "desktop bundle failed: RubyInstaller checksum mismatch (expected $RubyInstallerSha256, got $ActualHash)"
    }

    & 7z x $ArchivePath "-o$ExtractDir" -y | Out-Host
    Assert-LastExitCode 'RubyInstaller extraction'

    $RubyBinaries = @(Get-ChildItem -Path $ExtractDir -Filter ruby.exe -File -Recurse |
        Where-Object { $_.Directory.Name -eq 'bin' })
    if ($RubyBinaries.Count -ne 1) {
        throw "desktop bundle failed: expected one Ruby runtime in the archive, found $($RubyBinaries.Count)"
    }
    $RuntimeRoot = $RubyBinaries[0].Directory.Parent.FullName
    New-Item -ItemType Directory -Force -Path $RubyDir | Out-Null
    Get-ChildItem -Force $RuntimeRoot | Move-Item -Destination $RubyDir
} finally {
    if (Test-Path $TemporaryDir) {
        Remove-Item -Recurse -Force $TemporaryDir
    }
}

$RubyExe = Join-Path $RubyDir 'bin\ruby.exe'
$RubywExe = Join-Path $RubyDir 'bin\rubyw.exe'
$GemScript = Join-Path $RubyDir 'bin\gem'
$BundleScript = Join-Path $RubyDir 'bin\bundle'
foreach ($RuntimeFile in $RubyExe, $RubywExe, $GemScript) {
    if (-not (Test-Path -PathType Leaf $RuntimeFile)) {
        throw "desktop bundle failed: bundled runtime file is missing: $RuntimeFile"
    }
}

$RuntimeIdentity = & $RubyExe -e 'require "rbconfig"; print [RUBY_VERSION, RUBY_PLATFORM, RbConfig::CONFIG["host_cpu"]].join("|")'
Assert-LastExitCode 'Ruby platform check'
if ($RuntimeIdentity -notmatch "^$([regex]::Escape($RubyVersion))\|x64-mingw-ucrt\|(x64|x86_64)$") {
    throw "desktop bundle failed: unexpected Ruby runtime: $RuntimeIdentity"
}

Write-Host 'Copying the Rails application...'
& $RubyExe (Join-Path $RepoDir 'scripts\copy_desktop_app.rb') $RepoDir $AppDir
Assert-LastExitCode 'Rails application copy'

$BundlerMatch = [regex]::Match($Lockfile, '(?ms)^BUNDLED WITH\r?\n\s+([^\s]+)')
if (-not $BundlerMatch.Success) {
    throw 'desktop bundle failed: could not read the Bundler version from Gemfile.lock'
}
$BundlerVersion = $BundlerMatch.Groups[1].Value
& $RubyExe $GemScript install bundler --version $BundlerVersion --no-document
Assert-LastExitCode 'Bundler installation'
if (-not (Test-Path -PathType Leaf $BundleScript)) {
    throw "desktop bundle failed: Bundler executable is missing: $BundleScript"
}

Write-Host 'Installing production gems...'
$env:BUNDLE_FORCE_RUBY_PLATFORM = 'false'
Push-Location $AppDir
try {
    & $RubyExe $BundleScript config set --local deployment true
    Assert-LastExitCode 'Bundler deployment configuration'
    & $RubyExe $BundleScript config set --local path vendor/bundle
    Assert-LastExitCode 'Bundler path configuration'
    & $RubyExe $BundleScript config set --local without 'development test'
    Assert-LastExitCode 'Bundler group configuration'
    & $RubyExe $BundleScript config set --local force_ruby_platform false
    Assert-LastExitCode 'Bundler platform configuration'
    & $RubyExe $BundleScript install
    Assert-LastExitCode 'production gem installation'

    Write-Host 'Precompiling production assets...'
    $env:SECRET_KEY_BASE_DUMMY = '1'
    $env:RAILS_ENV = 'production'
    $env:APP_ROOT_URL = 'http://localhost:3000'
    $env:HOME_PAGE_URL = 'http://localhost:3000'
    $env:SMTP_ADDRESS = 'localhost'
    $env:SMTP_DOMAIN = 'localhost'
    $env:SMTP_PORT = '25'
    $env:SMTP_USER_NAME = 'placeholder'
    $env:SMTP_PASSWORD = 'placeholder'
    $env:NOTIFICATIONS_SENDER = 'placeholder@example.com'
    $env:COINGECKO_API_KEY = 'placeholder'
    $env:ORDERS_FREQUENCY_LIMIT = '60'
    & $RubyExe $BundleScript exec rails assets:precompile
    Assert-LastExitCode 'production asset precompilation'
} finally {
    Pop-Location
}

if (Get-ChildItem -Path $AppDir -Filter '.env*' -File -Recurse | Select-Object -First 1) {
    throw 'desktop bundle failed: an .env file was copied into the app bundle'
}
foreach ($RequiredFile in $RubywExe, (Join-Path $AppDir 'bin\rails'), (Join-Path $AppDir 'src-tauri\Cargo.toml')) {
    if (-not (Test-Path -PathType Leaf $RequiredFile)) {
        throw "desktop bundle failed: required bundle file is missing: $RequiredFile"
    }
}

Write-Host "Desktop resources are ready in $BundleDir"
