<#
.SYNOPSIS
Install the Ingot toolchain from a release archive.

.DESCRIPTION
The Windows half of scripts/install.sh, and it does the same and no more: work
out which archive fits this machine, download it, verify it against the
release's SHA256SUMS, unpack three binaries into one directory, and say what to
do next. It needs no administrator, writes nothing outside that directory, and
does not touch PATH unless asked.

It exists because the alternative was `cargo install`, which needs a Rust
toolchain and then compiles twenty-one crates on your machine — which looks
exactly like installing twenty-one things.

.PARAMETER Version
Install this version rather than the newest, e.g. 0.9.0.

.PARAMETER BinDir
Install here rather than into %LOCALAPPDATA%\Ingot\bin.

.PARAMETER AddToPath
Append the install directory to your user PATH. Off by default: changing PATH
outlives this script, so it is something to ask for rather than something to
discover afterwards.

.EXAMPLE
irm https://raw.githubusercontent.com/mathissdupont/ingot/main/scripts/install.ps1 | iex

.EXAMPLE
# Or, better, read it first:
irm https://raw.githubusercontent.com/mathissdupont/ingot/main/scripts/install.ps1 -OutFile install.ps1
notepad install.ps1
.\install.ps1 -AddToPath
#>

[CmdletBinding()]
param(
    [string] $Version,
    [string] $BinDir,
    [switch] $AddToPath
)

$ErrorActionPreference = 'Stop'
# Invoke-WebRequest's progress bar makes a download several times slower on
# Windows PowerShell, and there is nothing here worth watching tick.
$ProgressPreference = 'SilentlyContinue'

$repo = 'mathissdupont/ingot'
$binaries = @('ingot.exe', 'ingot-mcp-fs.exe', 'ingot-lsp.exe')

function Fail([string] $message, [int] $code = 2) {
    Write-Host "install: $message" -ForegroundColor Red
    exit $code
}

# Older Windows PowerShell defaults to TLS 1.0, which GitHub refuses.
try {
    [Net.ServicePointManager]::SecurityProtocol =
        [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
} catch {
    # Already on a runtime that negotiates properly.
}

# --- what this machine is --------------------------------------------------

$arch = $env:PROCESSOR_ARCHITECTURE
if ($arch -ne 'AMD64') {
    Fail "there is no release archive for $arch on Windows yet; install with ``cargo install ingot-cli`` (needs Rust 1.85 or newer)" 1
}
$target = 'x86_64-pc-windows-msvc'

# --- which version ---------------------------------------------------------

# Every release here is marked pre-release, because pre-1.0 the language, the IR
# and the artifact format can still move. GitHub's releases/latest EXCLUDES
# pre-releases and answers 404, so the newest tag has to come from the list.
if ([string]::IsNullOrWhiteSpace($Version)) {
    try {
        $releases = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases?per_page=1" `
            -Headers @{ 'User-Agent' = 'ingot-install' }
    } catch {
        Fail "could not ask GitHub which version is newest ($($_.Exception.Message)); pass -Version x.y.z and run again"
    }
    if (-not $releases -or -not $releases[0].tag_name) {
        Fail 'GitHub listed no releases; pass -Version x.y.z and run again'
    }
    $Version = $releases[0].tag_name -replace '^v', ''
}

$name = "ingot-$Version-$target"
$archive = "$name.zip"
$base = "https://github.com/$repo/releases/download/v$Version"

if ([string]::IsNullOrWhiteSpace($BinDir)) {
    $BinDir = Join-Path $env:LOCALAPPDATA 'Ingot\bin'
}

Write-Host "Ingot $Version for $target"

$work = Join-Path ([System.IO.Path]::GetTempPath()) ("ingot-install-" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null

try {
    $zip = Join-Path $work $archive
    $sums = Join-Path $work 'SHA256SUMS'

    Write-Host "  fetching  $archive"
    try {
        Invoke-WebRequest -Uri "$base/$archive" -OutFile $zip -UseBasicParsing
    } catch {
        Fail "could not download $base/$archive ($($_.Exception.Message))"
    }
    try {
        Invoke-WebRequest -Uri "$base/SHA256SUMS" -OutFile $sums -UseBasicParsing
    } catch {
        Fail "could not download the checksums, so nothing was installed"
    }

    # An unverified download is not installed, and there is no flag to skip
    # this: the whole reason to prefer an archive over `cargo install` is that
    # somebody else built it, which is exactly why it has to be checked.
    $expected = $null
    foreach ($line in Get-Content $sums) {
        if ($line -match "^([0-9a-fA-F]{64})\s+$([regex]::Escape($archive))$") {
            $expected = $Matches[1].ToLowerInvariant()
            break
        }
    }
    if (-not $expected) {
        Fail "SHA256SUMS does not mention $archive, so it cannot be verified"
    }
    $actual = (Get-FileHash -Path $zip -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($expected -ne $actual) {
        Fail "$archive does not match its checksum`n  expected $expected`n  got      $actual`nnothing was installed"
    }
    Write-Host "  verified  sha256 $actual"

    Expand-Archive -Path $zip -DestinationPath $work -Force

    # The two archive kinds do not agree about layout: a tarball carries a
    # ingot-<version>-<target>\ directory and this zip is flat. Both are already
    # published that way, so look for either rather than trusting one.
    $unpacked = Join-Path $work $name
    if (-not (Test-Path $unpacked)) {
        $unpacked = $work
    }

    if (-not (Test-Path $BinDir)) {
        New-Item -ItemType Directory -Path $BinDir -Force | Out-Null
    }
    foreach ($binary in $binaries) {
        $from = Join-Path $unpacked $binary
        if (-not (Test-Path $from)) {
            Fail "$archive does not contain $binary"
        }
        # Copied to a temporary name and then moved, so replacing a binary that
        # is currently running cannot leave a half-written one behind.
        $staging = Join-Path $BinDir ".$binary.new"
        Copy-Item -Path $from -Destination $staging -Force
        Move-Item -Path $staging -Destination (Join-Path $BinDir $binary) -Force
        Write-Host "  installed $(Join-Path $BinDir $binary)"
    }
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

Write-Host ''

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$already = $false
if ($userPath) {
    foreach ($entry in $userPath.Split(';')) {
        if ($entry.TrimEnd('\') -eq $BinDir.TrimEnd('\')) { $already = $true }
    }
}

if ($AddToPath -and -not $already) {
    if ([string]::IsNullOrEmpty($userPath)) {
        [Environment]::SetEnvironmentVariable('Path', $BinDir, 'User')
    } else {
        [Environment]::SetEnvironmentVariable('Path', "$userPath;$BinDir", 'User')
    }
    $env:Path = "$env:Path;$BinDir"
    Write-Host "Added $BinDir to your user PATH. Open a new terminal for it to take everywhere."
} elseif (-not $already) {
    Write-Host "$BinDir is not on your PATH yet. Either run this script again with"
    Write-Host '-AddToPath, or add it yourself:'
    Write-Host ''
    Write-Host "    [Environment]::SetEnvironmentVariable('Path', `"`$env:Path;$BinDir`", 'User')"
}

Write-Host ''
Write-Host 'Try it:'
Write-Host ''
Write-Host '    ingot init hello; cd hello'
Write-Host '    ingot check'
Write-Host '    ingot run --provider replay --input topic="compiler design"'
Write-Host ''
Write-Host 'That last one produces a real artifact without contacting anything: a new'
Write-Host 'project ships with a recorded fixture. `ingot doctor` says what a live run'
Write-Host 'would still need.'
