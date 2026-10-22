# Installs each earlier release's Windows MSI, upgrades it with the MSI built by build-for-windows.ps1, and
# checks that nothing from the earlier installation is left behind. Must run as an administrator, on a machine
# without ShutdownOnLan installed. Only ever sends a *wrong* secret – the real one would shut the machine down.

$ErrorActionPreference = 'Stop'

$ServiceName = 'ShutdownOnLan'
# 0.2.0's installer didn't create a service. `shutdown-on-lan.exe install` did, under this name.
$LegacyServiceName = 'shutdown-on-lan'
$Port = 53632
$UpgradeCode = '{755FA776-C302-4110-9BE4-93578A393F09}'
$Msi = Resolve-Path 'build\windows\Product.msi'

# Both are WiX bundles that chain the Visual C++ Redistributable ahead of the MSI. 0.3.0's was uploaded zipped.
$Releases = @(
    @{
        Version = '0.2.0'
        Url = 'https://github.com/jkmassel/shutdown-on-lan/releases/download/0.2.0/shutdown-on-lan-windows-installer.exe'
        Sha256 = '65d8314749b4fe0b02ce1a2eff04b3060273271bd30ecbf30ca52882a048b28a'
    },
    @{
        Version = '0.3.0'
        Url = 'https://github.com/jkmassel/shutdown-on-lan/releases/download/0.3.0/shutdown-on-lan-windows-installer.exe.zip'
        Sha256 = 'ecd8c39bb8293b398d5ad1ce14ac564fc565f6ee3389ba0f84b2aec7f5b20805'
    }
)

$script:Failures = @()

function Check([bool] $Condition, [string] $Message) {
    if (-not $Condition) {
        Write-Host "::error::$Message"
        $script:Failures += $Message
    }
}

# MSI products and WiX bundles registered with this upgrade code, as listed in Programs and Features
function Get-Registrations {
    Get-ChildItem 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall', 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall' |
        Get-ItemProperty |
        Where-Object { $_.DisplayName -eq 'ShutdownOnLan' -or ($_.BundleUpgradeCode -contains $UpgradeCode) }
}

function Write-Registrations {
    Get-Registrations | ForEach-Object {
        $kind = if ($_.BundleUpgradeCode) { 'bundle' } else { 'MSI' }
        Write-Host "  $kind $($_.PSChildName): '$($_.DisplayName)' $($_.DisplayVersion)"
    }
}

function Write-Diagnostics {
    Write-Host '--- Services'
    Get-CimInstance Win32_Service | Where-Object { $_.Name -in $ServiceName, $LegacyServiceName } | ForEach-Object {
        $exe = ($_.PathName -replace '^"([^"]+)".*$', '$1')
        Write-Host "$($_.Name): $($_.State), $($_.StartMode), exit code $($_.ExitCode) / $($_.ServiceSpecificExitCode), $($_.PathName) (exists: $(Test-Path $exe))"
    }
    Write-Host "--- Listening on port $Port"
    Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue |
        ForEach-Object { Write-Host "$($_.LocalAddress):$($_.LocalPort), PID $($_.OwningProcess) $((Get-Process -Id $_.OwningProcess -ErrorAction SilentlyContinue).Path)" }
    Write-Host '--- Service control manager events'
    Get-WinEvent -FilterHashtable @{ LogName = 'System'; ProviderName = 'Service Control Manager'; StartTime = $script:ReleaseStart } -ErrorAction SilentlyContinue |
        Where-Object { $_.Message -match 'shutdown' } |
        Sort-Object TimeCreated |
        ForEach-Object { Write-Host "$($_.TimeCreated) $($_.Id): $($_.Message)" }
    Write-Host '--- Application errors'
    Get-WinEvent -FilterHashtable @{ LogName = 'Application'; StartTime = $script:ReleaseStart; Level = 1, 2, 3 } -ErrorAction SilentlyContinue |
        Sort-Object TimeCreated |
        ForEach-Object { Write-Host "$($_.TimeCreated) $($_.ProviderName): $($_.Message)" }
}

function Get-ListeningProcessIds {
    @(Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue | ForEach-Object OwningProcess | Sort-Object -Unique)
}

function Wait-ForListener {
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-ListeningProcessIds).Count -eq 0) {
        if ((Get-Date) -gt $deadline) { return $false }
        Start-Sleep -Milliseconds 200
    }
    return $true
}

function Send-WrongSecret {
    $client = [System.Net.Sockets.TcpClient]::new('127.0.0.1', $Port)
    try {
        $stream = $client.GetStream()
        $bytes = [System.Text.Encoding]::ASCII.GetBytes("not-the-secret`n")
        $stream.Write($bytes, 0, $bytes.Length)
        $client.Client.Shutdown([System.Net.Sockets.SocketShutdown]::Send)
        $buffer = New-Object byte[] 1024
        while ($stream.Read($buffer, 0, $buffer.Length) -gt 0) { }
    } finally {
        $client.Close()
    }
}

# Removes anything a failed check left behind, so that each release starts from a clean machine
function Clear-Machine {
    foreach ($name in $ServiceName, $LegacyServiceName) {
        if (Get-Service $name -ErrorAction SilentlyContinue) {
            sc.exe stop $name | Out-Null
            Start-Sleep -Seconds 2
            sc.exe delete $name | Out-Null
        }
    }
    foreach ($registration in Get-Registrations) {
        if ($registration.BundleUpgradeCode) {
            Start-Process cmd.exe -ArgumentList "/c $($registration.QuietUninstallString)" -Wait
        } else {
            Start-Process msiexec.exe -ArgumentList "/x $($registration.PSChildName) /qn" -Wait
        }
    }
    Remove-Item 'HKLM:\SOFTWARE\ShutdownOnLan' -Recurse -ErrorAction SilentlyContinue
}

# The failing action is logged with "Return value 3", well before the end of the log
function Write-MsiLog([string] $Log) {
    Select-String -Path $Log -Pattern 'Return value 3' -Context 40, 0 | Select-Object -First 1 | ForEach-Object {
        $_.Context.PreContext + $_.Line
    } | Out-Host
    Write-Host '--- End of the log'
    Get-Content $Log -Tail 40 | Out-Host
}

function Invoke-Msiexec([string] $Arguments, [string] $Log) {
    $process = Start-Process msiexec.exe -ArgumentList "$Arguments /qn /l*v $Log" -Wait -PassThru
    if ($process.ExitCode -ne 0) { Write-MsiLog $Log }
    return $process.ExitCode
}

foreach ($release in $Releases) {
    $version = $release.Version
    Write-Host "=== Upgrading from $version"

    Clear-Machine
    $script:ReleaseStart = Get-Date

    $directory = Join-Path $env:RUNNER_TEMP "shutdown-on-lan-$version"
    New-Item -ItemType Directory -Force $directory | Out-Null
    $download = Join-Path $directory (Split-Path $release.Url -Leaf)
    Invoke-WebRequest $release.Url -OutFile $download
    $hash = (Get-FileHash $download -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($hash -ne $release.Sha256) { throw "Unexpected SHA-256 for the $version installer: $hash" }
    if ($download.EndsWith('.zip')) {
        Expand-Archive $download -DestinationPath $directory -Force
    }
    $bundle = Get-ChildItem $directory -Filter '*.exe' | Select-Object -First 1

    # The bundle can't be installed here: its Visual C++ 2015 Redistributable exits with 1638 when a newer one is
    # installed, which the bundle treats as a failure. Install the MSI it contains instead – that's what registers
    # the product and the service.
    $extracted = Join-Path $directory 'extracted'
    & "$env:WIX\bin\dark.exe" -nologo -x $extracted $bundle.FullName (Join-Path $directory 'bundle.wxs') | Out-Host
    $legacyMsi = Get-ChildItem $extracted -Recurse -File |
        Where-Object { $_.Length -gt 8 -and -not (Compare-Object ([System.IO.File]::ReadAllBytes($_.FullName)[0..3]) ([byte[]](0xD0, 0xCF, 0x11, 0xE0))) } |
        Select-Object -First 1
    if (-not $legacyMsi) { throw "Unable to find the MSI in the $version bundle" }

    Write-Host "--- Installing $version"
    $exitCode = Invoke-Msiexec "/i `"$($legacyMsi.FullName)`"" "$version-install.log"
    if ($exitCode -ne 0) { throw "Installing $version failed with exit code $exitCode" }

    if ($version -eq '0.2.0') {
        # Creates the service, and the default configuration
        & "${env:ProgramFiles(x86)}\ShutdownOnLan\shutdown-on-lan.exe" install
        Start-Service $LegacyServiceName
    } else {
        # The 0.3.0 service panics if the configuration is missing, and nothing in 0.3.0 creates it – so this is
        # its default configuration, as written by 0.2.0
        $key = New-Item 'HKLM:\SOFTWARE\ShutdownOnLan' -Force
        $key | New-ItemProperty -Name 'ip_addresses' -Value '127.0.0.1' | Out-Null
        $key | New-ItemProperty -Name 'port' -Value $Port -PropertyType DWord | Out-Null
        $key | New-ItemProperty -Name 'secret' -Value 'Super Secret String' | Out-Null
        Start-Service $ServiceName
    }

    if (-not (Wait-ForListener)) {
        Write-Diagnostics
        throw "$version isn't listening on port $Port"
    }
    Write-Host "Installed $version, registered as:"
    Write-Registrations

    Write-Host '--- Upgrading'
    $exitCode = Invoke-Msiexec "/i `"$Msi`"" "$version-upgrade.log"
    Check ($exitCode -eq 0) "Upgrading $version failed with exit code $exitCode"

    Write-Host 'Registered as:'
    Write-Registrations
    Write-Diagnostics
    $registrations = @(Get-Registrations)
    Check ($registrations.Count -eq 1 -and -not $registrations[0].BundleUpgradeCode) "Expected only the new MSI in Programs and Features after upgrading $version"

    Check (-not (Get-Service $LegacyServiceName -ErrorAction SilentlyContinue)) "Expected the $version service, '$LegacyServiceName', to be removed"
    $service = Get-CimInstance Win32_Service -Filter "Name='$ServiceName'"
    Check ($service -and $service.State -eq 'Running') "Expected the $ServiceName service to be running after upgrading $version"
    Check ($service.PathName -like "*$env:ProgramFiles\ShutdownOnLan\*") "Expected the $ServiceName service in $env:ProgramFiles, but found $($service.PathName)"

    # The service writes its configuration, then starts listening
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-ListeningProcessIds) -notcontains $service.ProcessId -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 200
    }
    $listeners = Get-ListeningProcessIds
    $owners = $listeners | ForEach-Object { (Get-Process -Id $_ -ErrorAction SilentlyContinue).Path }
    Check ($listeners.Count -eq 1 -and $listeners[0] -eq $service.ProcessId) "Expected only the $ServiceName service to be listening on port $Port, but found: $($owners -join ', ')"
    try { Send-WrongSecret } catch { Check $false "Unable to connect after upgrading $version`: $_" }

    Write-Host '--- Uninstalling'
    $productCode = @(Get-Registrations)[0].PSChildName
    $exitCode = Invoke-Msiexec "/x `"$Msi`"" "$version-uninstall.log"
    Check ($exitCode -eq 0) "Uninstalling after upgrading $version failed with exit code $exitCode"
    Check (-not (Get-Service $ServiceName, $LegacyServiceName -ErrorAction SilentlyContinue)) "Expected no service to be left after uninstalling"
    Check ((Get-ListeningProcessIds).Count -eq 0) "Expected nothing to be listening on port $Port after uninstalling"
    Check (@(Get-Registrations).Count -eq 0) "Expected nothing in Programs and Features after uninstalling"
    $leftovers = @(Get-ChildItem "$env:windir\Installer" -Filter "*$productCode*" -ErrorAction SilentlyContinue)
    Check ($leftovers.Count -eq 0) "Expected nothing to be left in $env:windir\Installer after uninstalling, but found: $($leftovers.Name -join ', ')"
    Write-Registrations
}

if ($script:Failures.Count -gt 0) {
    Write-Host "$($script:Failures.Count) check(s) failed"
    exit 1
}
Write-Host 'All legacy upgrade checks passed'
