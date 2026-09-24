# Installs the MSI built by build-for-windows.ps1, then checks the service's lifecycle.
# Must run as an administrator. Only ever sends a *wrong* secret – the real one would shut the machine down.

$ErrorActionPreference = 'Stop'

$ServiceName = 'ShutdownOnLan'
$Port = 53632
$EventSource = 'ShutdownOnLan'
$EventSourceKey = "HKLM:\SYSTEM\CurrentControlSet\Services\EventLog\Application\$EventSource"
$TestStart = Get-Date

function Fail([string] $Message) {
    Write-Host "::error::$Message"
    Write-Host '--- Service log'
    Get-ServiceLog | ForEach-Object { Write-Host "$($_.TimeCreated) [$($_.LevelDisplayName)] $($_.Message)" }
    try { Write-Diagnostics } catch { Write-Host "Unable to collect diagnostics: $_" }
    exit 1
}

# Tells a missing log entry apart from a service that isn't doing what the log says
function Write-Diagnostics {
    Write-Host '--- Service state'
    $service = Get-CimInstance Win32_Service -Filter "Name='$ServiceName'"
    if ($service) {
        Write-Host "State: $($service.State), PID: $($service.ProcessId), exit code: $($service.ExitCode) / $($service.ServiceSpecificExitCode)"
    } else {
        Write-Host 'The service is not installed'
    }

    Write-Host "--- Listening on port $Port"
    Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue |
        ForEach-Object { Write-Host "$($_.LocalAddress):$($_.LocalPort), PID $($_.OwningProcess)" }
    $client = [System.Net.Sockets.TcpClient]::new()
    try {
        $client.Connect('127.0.0.1', $Port)
        Write-Host 'A connection to 127.0.0.1 succeeded'
    } catch {
        Write-Host "A connection to 127.0.0.1 failed: $($_.Exception.InnerException.Message)"
    } finally {
        $client.Close()
    }

    Write-Host '--- Other Application and System events'
    # Crashes (Application Error, Windows Error Reporting) and service control manager events
    Get-WinEvent -FilterHashtable @{ LogName = 'Application', 'System'; StartTime = $TestStart; Level = 1, 2, 3 } -ErrorAction SilentlyContinue |
        Where-Object { $_.ProviderName -ne $EventSource } |
        Sort-Object TimeCreated |
        ForEach-Object { Write-Host "$($_.TimeCreated) $($_.LogName)/$($_.ProviderName) [$($_.LevelDisplayName)] $($_.Message)" }
    Get-WinEvent -FilterHashtable @{ LogName = 'System'; ProviderName = 'Service Control Manager'; StartTime = $TestStart } -ErrorAction SilentlyContinue |
        Where-Object { $_.Message -match $ServiceName } |
        Sort-Object TimeCreated |
        ForEach-Object { Write-Host "$($_.TimeCreated) SCM: $($_.Message)" }
}

function Get-ServiceLog {
    # Get-WinEvent reports an error, rather than returning nothing, if there are no matching events
    Get-WinEvent -FilterHashtable @{ LogName = 'Application'; ProviderName = $EventSource; StartTime = $TestStart } -ErrorAction SilentlyContinue |
        Sort-Object RecordId
}

# `Message` is only the logged text if the event source's message file is registered – otherwise it's
# "The description for Event ID 3 from source ShutdownOnLan cannot be found..."
function Wait-ForLog([string] $Text, [string] $Level = 'Information') {
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline) {
        $entry = Get-ServiceLog | Where-Object { $_.Message -and $_.Message.Contains($Text) } | Select-Object -Last 1
        if ($entry) {
            if ($entry.LevelDisplayName -ne $Level) { Fail "Expected '$Text' to be logged as $Level, but it was $($entry.LevelDisplayName)" }
            return
        }
        Start-Sleep -Milliseconds 200
    }
    Fail "Timed out waiting for the event log to contain '$Text'"
}

function Get-ServiceExitCodes {
    $service = Get-CimInstance Win32_Service -Filter "Name='$ServiceName'"
    return @{ ExitCode = $service.ExitCode; ServiceSpecificExitCode = $service.ServiceSpecificExitCode }
}

function Wait-ForConnection {
    # The service starts listening shortly after it reports that it's running
    $deadline = (Get-Date).AddSeconds(30)
    while ($true) {
        try {
            Send-WrongSecret
            return
        } catch {
            if ((Get-Date) -gt $deadline) { Fail "Unable to connect: $_" }
            Start-Sleep -Milliseconds 200
        }
    }
}

function Get-InstalledProducts {
    Get-ChildItem 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall', 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall' |
        Get-ItemProperty |
        Where-Object DisplayName -eq 'ShutdownOnLan'
}

function Send-WrongSecret {
    $client = [System.Net.Sockets.TcpClient]::new('127.0.0.1', $Port)
    try {
        $stream = $client.GetStream()
        $bytes = [System.Text.Encoding]::ASCII.GetBytes("not-the-secret`n")
        $stream.Write($bytes, 0, $bytes.Length)
        $client.Client.Shutdown([System.Net.Sockets.SocketShutdown]::Send)

        # Wait for the service to close the connection
        $buffer = New-Object byte[] 1024
        while ($stream.Read($buffer, 0, $buffer.Length) -gt 0) { }
    } finally {
        $client.Close()
    }
}

Write-Host '--- Installing'
$msi = Resolve-Path 'build\windows\Product.msi'
$install = Start-Process msiexec.exe -ArgumentList "/i `"$msi`" /qn /l*v msi-install.log" -Wait -PassThru
if ($install.ExitCode -ne 0) {
    Get-Content msi-install.log -Tail 100
    Fail "Installing the MSI failed with exit code $($install.ExitCode)"
}

$service = Get-Service $ServiceName
if ($service.Status -ne 'Running') {
    Fail "Expected the service to be running after install, but it's $($service.Status)"
}

# The service writes its default configuration after reporting that it's running
Wait-ForLog 'Listening on port'

Write-Host '--- The default configuration is written to the registry'
$configuration = Get-ItemProperty 'HKLM:\SOFTWARE\ShutdownOnLan'
if ($configuration.port -ne $Port) { Fail "Expected port $Port, but found '$($configuration.port)'" }
# An empty list accepts connections on every interface
if ($configuration.ip_addresses -ne '') { Fail "Expected empty ip_addresses, but found '$($configuration.ip_addresses)'" }
if ($configuration.secret -notmatch '^[0-9a-f]{32}$') { Fail 'Expected a random 32-character hex secret' }
if ($configuration.allowed_sources -ne '') { Fail "Expected empty allowed_sources, but found '$($configuration.allowed_sources)'" }

Write-Host '--- Only SYSTEM and Administrators can read the configuration'
$acl = Get-Acl 'HKLM:\SOFTWARE\ShutdownOnLan'
if (-not $acl.AreAccessRulesProtected) { Fail 'Expected the registry key not to inherit permissions from its parent' }
$identities = $acl.Access |
    ForEach-Object { $_.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value } |
    Sort-Object -Unique
# LocalSystem and BUILTIN\Administrators
if (($identities -join ',') -ne 'S-1-5-18,S-1-5-32-544') {
    Fail "Expected only SYSTEM and Administrators to have access, but found $($identities -join ', ')"
}

Write-Host '--- The service is installed as 64-bit, and restarts when it fails'
$service = Get-CimInstance Win32_Service -Filter "Name='$ServiceName'"
if ($service.PathName -notlike "*$env:ProgramFiles\ShutdownOnLan\*") { Fail "Expected the service in $env:ProgramFiles, but found $($service.PathName)" }
$failureActions = sc.exe qfailure $ServiceName | Out-String
if ($failureActions -notmatch 'RESTART') { Fail "Expected the service to restart on failure, but found:`n$failureActions" }
$failureFlag = sc.exe qfailureflag $ServiceName | Out-String
if ($failureFlag -notmatch 'TRUE') { Fail "Expected failure actions to apply when the service stops with an error, but found:`n$failureFlag" }

Write-Host '--- The service accepts connections and logs to the event log'
Send-WrongSecret
Wait-ForLog 'Connection closed by 127.0.0.1'

Write-Host '--- The service stops cleanly'
Stop-Service $ServiceName
(Get-Service $ServiceName).WaitForStatus('Stopped', '00:00:30')
$codes = Get-ServiceExitCodes
if ($codes.ExitCode -ne 0) { Fail "Expected exit code 0 after stopping, but found $($codes.ExitCode)" }

Write-Host '--- The service reports a failure when it cannot listen'
$blocker = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Any, $Port)
$blocker.Start()
try {
    try {
        Start-Service $ServiceName
    } catch {
        # The service may already have stopped by the time Start-Service checks on it
        Write-Host "Start-Service reported: $_"
    }
    (Get-Service $ServiceName).WaitForStatus('Stopped', '00:00:30')
} finally {
    $blocker.Stop()
}

# ERROR_SERVICE_SPECIFIC_ERROR, with the service-specific code set by the service
$codes = Get-ServiceExitCodes
if ($codes.ExitCode -ne 1066 -or $codes.ServiceSpecificExitCode -ne 1) {
    Fail "Expected exit code 1066 with service-specific code 1, but found $($codes.ExitCode) / $($codes.ServiceSpecificExitCode)"
}
Wait-ForLog 'Listener service stopped' -Level 'Error'

Write-Host '--- The service restarts by itself once the port is free'
# The failure actions restart it 5 seconds after it stops
(Get-Service $ServiceName).WaitForStatus('Running', '00:00:30')
Wait-ForConnection

Write-Host '--- Upgrading replaces the installation and keeps the configuration'
$current = [version](cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).packages[0].version
$next = "$($current.Major).$($current.Minor).$($current.Build + 1)"
& "$PSScriptRoot\build-for-windows.ps1" -Version $next -Output 'Upgrade.msi'

$upgradeMsi = Resolve-Path 'build\windows\Upgrade.msi'
$upgrade = Start-Process msiexec.exe -ArgumentList "/i `"$upgradeMsi`" /qn /l*v msi-upgrade.log" -Wait -PassThru
if ($upgrade.ExitCode -ne 0) {
    Get-Content msi-upgrade.log -Tail 100
    Fail "Upgrading failed with exit code $($upgrade.ExitCode)"
}

$products = @(Get-InstalledProducts)
if ($products.Count -ne 1 -or $products[0].DisplayVersion -ne $next) {
    Fail "Expected only version $next to be installed, but found: $(($products | ForEach-Object DisplayVersion) -join ', ')"
}
if ((Get-ItemProperty 'HKLM:\SOFTWARE\ShutdownOnLan').secret -ne $configuration.secret) {
    Fail 'Expected the secret to be kept when upgrading'
}
(Get-Service $ServiceName).WaitForStatus('Running', '00:00:30')
Wait-ForConnection
if (-not (Test-Path $EventSourceKey)) { Fail 'Expected the event source to still be registered after upgrading' }
$msi = $upgradeMsi

Write-Host '--- Uninstalling'
$uninstall = Start-Process msiexec.exe -ArgumentList "/x `"$msi`" /qn /l*v msi-uninstall.log" -Wait -PassThru
if ($uninstall.ExitCode -ne 0) {
    Get-Content msi-uninstall.log -Tail 100
    Fail "Uninstalling the MSI failed with exit code $($uninstall.ExitCode)"
}
if (Get-Service $ServiceName -ErrorAction SilentlyContinue) {
    Fail 'Expected the service to be removed after uninstall'
}
if (Test-Path $EventSourceKey) {
    Fail 'Expected the event source to be removed after uninstall'
}

Write-Host 'All Windows service checks passed'
