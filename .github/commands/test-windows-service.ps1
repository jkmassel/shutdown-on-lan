# Installs the MSI built by build-for-windows.ps1, then checks the service's lifecycle.
# Must run as an administrator. Only ever sends a *wrong* secret – the real one would shut the machine down.

$ErrorActionPreference = 'Stop'

$ServiceName = 'ShutdownOnLan'
$Port = 53632
$LogPath = 'C:\ProgramData\ShutdownOnLan\shutdown-on-lan.log'

function Fail([string] $Message) {
    Write-Host "::error::$Message"
    if (Test-Path $LogPath) {
        Write-Host '--- Service log'
        Get-Content $LogPath
    }
    exit 1
}

function Wait-ForLog([string] $Text) {
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline) {
        if ((Test-Path $LogPath) -and (Get-Content $LogPath -Raw).Contains($Text)) {
            return
        }
        Start-Sleep -Milliseconds 200
    }
    Fail "Timed out waiting for the service log to contain '$Text'"
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

Write-Host '--- The service accepts connections and logs to ProgramData'
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
Wait-ForLog 'Listener service stopped'

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

Write-Host 'All Windows service checks passed'
