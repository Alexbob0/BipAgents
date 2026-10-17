# BipAgents on Windows: the Linux install, in WSL2 (Ubuntu) — the same as on a Linux server, sandboxes included.
# In PowerShell opened as administrator (right-click › Run as administrator):
#   irm https://raw.githubusercontent.com/Alexbob0/BipAgents/main/install.ps1 | iex
# Run it again at any time: what is done already is skipped. Options, through the environment:
#   $env:BIPAGENTS_DISTRO = 'Ubuntu-24.04'   the WSL distribution (created if missing)
#   $env:BIPAGENTS_ARGS = '--dry-run'        passed on to the installer
#
# What it does, each step only when needed:
#   1. WSL2 and Ubuntu (Ubuntu asks for a Linux user name and password the first time; Windows may need a restart).
#   2. WSL's network mirrors Windows' (Windows 11 22H2 or later): the local network and Windows' Tailscale reach the
#      agents, and a model server running on Windows (LM Studio, Ollama…) is found at 127.0.0.1.
#   3. In Ubuntu: systemd, git, curl, podman (the agents' sandboxes), and « linger » (services without a session).
#   4. Windows' Tailscale, signed in: it publishes the agents on your tailnet.
#   5. A task at logon keeps WSL awake (otherwise Windows stops it once no window uses it).
#   6. The BipAgents installer, in Ubuntu, under your Linux user: it ends with the QR code to scan in the app.

function Install-BipAgents {
    # Not « Stop »: Windows PowerShell would turn what wsl.exe or tailscale.exe write on stderr into errors.
    $ErrorActionPreference = 'Continue'
    $env:WSL_UTF8 = '1'   # wsl.exe answers in UTF-8, not UTF-16
    $distro = if ($env:BIPAGENTS_DISTRO) { $env:BIPAGENTS_DISTRO } else { 'Ubuntu-24.04' }
    $installer = 'https://raw.githubusercontent.com/Alexbob0/BipAgents/main/install.sh'
    $tailscale = Join-Path $env:ProgramFiles 'Tailscale\tailscale.exe'

    function Say([string]$text) { Write-Host $text }
    function Stop-Install([string]$text) { Write-Host $text -ForegroundColor Yellow }
    function Get-Distros { @(& wsl.exe --list --quiet 2>$null | ForEach-Object { $_.Trim() } | Where-Object { $_ }) }
    # A bash script run in the distribution, passed as base64: no quoting goes wrong between PowerShell, wsl.exe and bash.
    # Called as a statement (its output straight to the console, the installer's questions included); the result is
    # in $LASTEXITCODE.
    function Invoke-Linux([string]$script, [string]$user = 'root') {
        $bytes = [Text.Encoding]::UTF8.GetBytes(($script -replace "`r", ''))
        $b64 = [Convert]::ToBase64String($bytes)
        & wsl.exe -d $distro -u $user -- bash -c "echo $b64 | base64 -d | bash"
    }

    $identity = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    if (-not $identity.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        return Stop-Install 'Open PowerShell as administrator (right-click › Run as administrator), then run this again.'
    }
    if ([Environment]::OSVersion.Version.Build -lt 22621) {
        return Stop-Install 'BipAgents needs Windows 11 22H2 or later (WSL''s mirrored network).'
    }

    # 1. WSL2 and Ubuntu.
    if ($distro -notin (Get-Distros)) {
        Say "Installing WSL2 and $distro. Ubuntu will ask for a user name and a password (for Linux): choose them,"
        Say 'then type « exit » to come back here.'
        & wsl.exe --install -d $distro
        if ($distro -notin (Get-Distros)) {
            return Stop-Install 'Restart Windows to finish installing WSL, then run this command again.'
        }
    }
    & wsl.exe --update 2>$null | Out-Null   # the mirrored network needs WSL 2.0 or later
    $login = "$(& wsl.exe -d $distro -- id -un)".Trim()
    if (-not $login -or $login -eq 'root') {
        return Stop-Install "$distro has no user of its own: open it once (wsl -d $distro), create one, then run this again."
    }

    # 2. WSL's network mirrors Windows'.
    $config = Join-Path $env:USERPROFILE '.wslconfig'
    $text = if (Test-Path $config) { Get-Content $config -Raw } else { '' }
    $restart = $false
    if ($text -notmatch '(?m)^\s*networkingMode\s*=\s*mirrored') {
        if ($text -match '(?m)^\s*networkingMode\s*=') {
            $text = $text -replace '(?m)^\s*networkingMode\s*=.*$', 'networkingMode=mirrored'
        } elseif ($text -match '(?m)^\s*\[wsl2\]') {
            $text = $text -replace '(?m)^(\s*\[wsl2\]\s*)$', "`$1`nnetworkingMode=mirrored"
        } else {
            $text = $text.TrimEnd() + "`n[wsl2]`nnetworkingMode=mirrored`n"
        }
        Set-Content -Path $config -Value $text.TrimStart() -Encoding ascii
        $restart = $true
    }
    try {
        # The local network may reach WSL (the bridge's door for when the tailnet is down). Only what listens on
        # all addresses is reached: Hermes, the voices and the bridge's own port stay on 127.0.0.1.
        Set-NetFirewallHyperVVMSetting -Name '{40E0AC32-46A5-438A-A0B2-2B479E8F2E90}' -DefaultInboundAction Allow `
            -ErrorAction Stop
    } catch {
        Say 'Note: the local network will not reach the agents (only the tailnet). Hyper-V firewall: not set.'
    }

    # 3. Ubuntu: systemd, the tools, linger.
    Say "Preparing $distro (systemd, git, curl, podman)…"
    $prepare = @"
set -e
export DEBIAN_FRONTEND=noninteractive
if ! grep -Eq '^\s*systemd\s*=\s*true' /etc/wsl.conf 2>/dev/null; then
  if grep -q '^\[boot\]' /etc/wsl.conf 2>/dev/null; then sed -i '/^\[boot\]/a systemd=true' /etc/wsl.conf
  else printf '\n[boot]\nsystemd=true\n' >> /etc/wsl.conf; fi
  touch /run/bipagents-restart
fi
missing=""
for tool in git curl podman newuidmap; do command -v `$tool >/dev/null 2>&1 || missing=1; done
if [ -n "`$missing" ]; then
  apt-get update -qq
  apt-get install -y -qq git curl podman uidmap ca-certificates >/dev/null
fi
"@
    Invoke-Linux $prepare
    if ($LASTEXITCODE -ne 0) { return Stop-Install "Preparing $distro failed (see above)." }
    Invoke-Linux 'test -e /run/bipagents-restart'
    if ($LASTEXITCODE -eq 0) { $restart = $true }
    if ($restart) {
        Say 'Restarting WSL (new settings)…'
        & wsl.exe --shutdown
    }
    Invoke-Linux "loginctl enable-linger $login"
    if ($LASTEXITCODE -ne 0) {
        return Stop-Install "systemd does not answer in $distro: run « wsl --shutdown », then this command again."
    }

    # 4. Windows' Tailscale, signed in.
    if (-not (Test-Path $tailscale)) {
        $answer = Read-Host 'Tailscale is not installed. Install it now with winget? [Y/n]'
        if ($answer -match '^(n|no|non)$') {
            return Stop-Install 'Install Tailscale (https://tailscale.com/download), sign in, then run this again.'
        }
        & winget.exe install --id tailscale.tailscale -e --accept-source-agreements --accept-package-agreements
        if (-not (Test-Path $tailscale)) { return Stop-Install 'Tailscale did not install: https://tailscale.com/download' }
    }
    while ($true) {
        & $tailscale status *> $null
        if ($LASTEXITCODE -eq 0) { break }
        Read-Host 'Sign in to Tailscale (its icon near the clock › Log in), with the same account as your iPhone, then press Enter'
    }

    # 5. WSL kept awake from logon (no window: conhost without a console).
    $action = New-ScheduledTaskAction -Execute 'conhost.exe' -Argument "--headless wsl.exe -d $distro --exec sleep infinity"
    $trigger = New-ScheduledTaskTrigger -AtLogOn -User "$env:USERDOMAIN\$env:USERNAME"
    $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
        -ExecutionTimeLimit ([TimeSpan]::Zero)
    Register-ScheduledTask -TaskName 'BipAgents' -Action $action -Trigger $trigger -Settings $settings -Force `
        -Description "Keeps WSL ($distro) awake: BipAgents' agents live there." | Out-Null
    Start-ScheduledTask -TaskName 'BipAgents'

    # 6. The installer, under the Linux user.
    Say "Starting the BipAgents installer in $distro, as $login…"
    Invoke-Linux "curl -fsSL $installer | bash -s -- $env:BIPAGENTS_ARGS" $login
    if ($LASTEXITCODE -ne 0) { return Stop-Install 'The installer stopped (see above). Run this command again to go on.' }
    Say ''
    Say 'Done. Keep this PC on and awake (Settings › System › Power: never sleep when plugged in).'
    Say "Later: « wsl -d $distro -- bash -lc 'bipagents update' » brings everything up to date."
}

Install-BipAgents
