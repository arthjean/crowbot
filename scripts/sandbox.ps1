<#
.SYNOPSIS
Builds Crow Bot and starts the client on a throwaway daemon, apart from your own data.

.DESCRIPTION
The sandbox has its own endpoint, database, bot folders, runtimes, and provider
homes, under %LOCALAPPDATA%\crowbot-sandbox\<name>. Sign-ins made there stay
there until you clean the sandbox. The client starts the sandbox daemon, which
keeps running after the window closes: stop it with -Action stop.

.EXAMPLE
./scripts/sandbox.ps1                 # build, then open the client on the sandbox
./scripts/sandbox.ps1 -Action stop    # stop the sandbox daemon and client
./scripts/sandbox.ps1 -Action reset   # wipe the sandbox, then start it fresh
./scripts/sandbox.ps1 -Action clean   # stop and delete the sandbox
#>
[CmdletBinding()]
param(
    [ValidateSet('start', 'stop', 'reset', 'clean')]
    [string]$Action = 'start',
    # Several sandboxes can live side by side under different names.
    [ValidatePattern('^[A-Za-z0-9_-]+$')]
    [string]$Name = 'default',
    # Also stops other daemons run from this build, which lock its executable.
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$sandbox = Join-Path $env:LOCALAPPDATA "crowbot-sandbox\$Name"
$pipe = "crowbotd-sandbox-$Name"
$endpoint = "\\.\pipe\$pipe"

# Both workspaces build into one folder, so the client and the daemon sit side by side.
if (-not $env:CARGO_TARGET_DIR) {
    $env:CARGO_TARGET_DIR = Join-Path $repo 'target'
}
$bin = Join-Path $env:CARGO_TARGET_DIR 'debug'
$daemon = Join-Path $bin 'crowbotd.exe'
$client = Join-Path $bin 'crowbot.exe'

function Get-SandboxProcesses {
    Get-CimInstance Win32_Process -Filter "Name='crowbotd.exe' OR Name='crowbot.exe'" |
        Where-Object { $_.CommandLine -like "*$pipe*" }
}

# The agents the daemon started from the runtimes it installed in the sandbox.
# A daemon stopped at once leaves them running until they see it is gone.
function Get-SandboxAgents {
    Get-CimInstance Win32_Process |
        Where-Object { $_.ExecutablePath -like "$sandbox\*" }
}

function Stop-Processes($processes) {
    foreach ($process in $processes) {
        Stop-Process -Id $process.ProcessId -Force -ErrorAction SilentlyContinue
    }
    foreach ($process in $processes) {
        Wait-Process -Id $process.ProcessId -Timeout 10 -ErrorAction SilentlyContinue
    }
}

function Stop-Sandbox {
    $running = @(Get-SandboxProcesses)
    Stop-Processes $running
    $agents = @(Get-SandboxAgents)
    Stop-Processes $agents
    $count = $running.Count + $agents.Count
    if ($count -gt 0) {
        Write-Host "Stopped the sandbox '$Name' ($count process(es))."
    }
}

# A file can stay locked a moment after its process ends (an antivirus scan,
# for one): the removal tries again before it gives up.
function Remove-Sandbox {
    for ($try = 1; Test-Path $sandbox; $try++) {
        try {
            Remove-Item -Recurse -Force $sandbox -ErrorAction Stop
        }
        catch {
            if ($try -ge 10) {
                throw "Cannot remove $sandbox`: $($_.Exception.Message)"
            }
            Start-Sleep -Milliseconds 500
        }
    }
}

# A daemon of this build that is not the sandbox's (your own, or another
# sandbox) keeps the executable locked, and the build would fail.
function Assert-ExecutableFree {
    $others = @(Get-CimInstance Win32_Process -Filter "Name='crowbotd.exe' OR Name='crowbot.exe'" |
            Where-Object { $_.ExecutablePath -and ((Split-Path -Parent $_.ExecutablePath) -eq $bin) })
    if ($others.Count -eq 0) {
        return
    }
    if (-not $Force) {
        $list = ($others | ForEach-Object { "  $($_.ProcessId)  $($_.CommandLine)" }) -join "`n"
        throw "These processes run from $bin and lock its executables:`n$list`nClose them, or run again with -Force to stop them (a running turn ends)."
    }
    foreach ($process in $others) {
        Stop-Process -Id $process.ProcessId -Force -ErrorAction SilentlyContinue
        Wait-Process -Id $process.ProcessId -Timeout 10 -ErrorAction SilentlyContinue
    }
}

switch ($Action) {
    'stop' {
        Stop-Sandbox
        return
    }
    'clean' {
        Stop-Sandbox
        Remove-Sandbox
        Write-Host "Removed $sandbox."
        return
    }
    'reset' {
        Stop-Sandbox
        Remove-Sandbox
    }
}

Stop-Sandbox
Assert-ExecutableFree

Push-Location $repo
try {
    cargo build -p crowbotd
    if ($LASTEXITCODE -ne 0) { throw 'The daemon did not build.' }
    cargo build --manifest-path crates/crowbot-desktop/Cargo.toml
    if ($LASTEXITCODE -ne 0) { throw 'The client did not build.' }
}
finally {
    Pop-Location
}

New-Item -ItemType Directory -Force $sandbox | Out-Null
Write-Host @"
Sandbox '$Name'
  endpoint  $endpoint
  data      $sandbox
The daemon keeps running after the window closes: ./scripts/sandbox.ps1 -Action stop -Name $Name
"@

& $client --endpoint $endpoint --daemon $daemon -- `
    --db (Join-Path $sandbox 'crowbot.db') `
    --bots-dir (Join-Path $sandbox 'bots') `
    --data-dir (Join-Path $sandbox 'data')
