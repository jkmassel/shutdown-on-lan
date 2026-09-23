# shutdown-on-lan
A cross-platform lightweight complement to wake on lan.

Allows shutting down a computer remotely with a single TCP packet via external control systems (for instance AMX controllers). 

**Supported Platforms:**
- Windows
- macOS
- Linux

### Installation

Installers are provided for Windows, macOS and Linux. Each installation generates its own random secret, and by default accepts connections from any client on every network interface – so once you've given the secret to your control system, it's ready to use. The following settings are available:
 
##### IP Address
Customizing the IP address field allows you to specify which interfaces the service will accept connections on, which is useful when the machine is connected to more than one network – this address should match that of the relevant interface. Multiple addresses can be provided as a comma-separated list. Connections arriving on any other interface are closed without being read. By default this is empty, which accepts connections on every interface. It's important that this IP address doesn't change – you should consider adding either a DHCP reservation or using a static address for this interface.

##### Allowed Sources
Customizing the allowed sources field allows you to specify which clients can connect – for instance, the IP address of your control system. Multiple addresses can be provided as a comma-separated list. Connections from any other address are closed without being read. By default this is empty, which allows any client to connect. As with the IP address, you should use a DHCP reservation or a static address for each client.

On macOS and Linux, this can be set with `shutdown-on-lan set --allowed-sources 10.0.1.50`. On Windows, it's the `allowed_sources` registry value.

##### Port
Customizing the port field allows you to specify which port the service will listen on. By default, this is set to `53632`.

##### Secret
The secret is the string that's sent to the machine in order to shut it down. Each installation generates its own random secret. To see it, run `sudo shutdown-on-lan get --secret` on Linux, or `sudo /Library/Services/shutdownonlan get --secret` on macOS. On Windows, it's the `secret` registry value. If you change it, be sure to use a strong secret – anyone on the network with the port number and this secret can shut down your machine!

_The secret cannot be empty or longer than 4096 bytes._

#### Windows
1. Download the latest version of the application and run the installer.
2. Windows may warn that this software is from an unknown author and provide a popup saying "Windows Protected your PC". Click "More Info" then "Run Anyway".
3. Once the installer has finished, you can configure the service directly in the Registry – all of the configuration settings are in `HKEY_LOCAL_MACHINE\SOFTWARE\ShutdownOnLan`. See details on each setting above.
4. Once settings are in place, restart the `ShutdownOnLan` service.

#### Mac
1. Download the latest version of the application and run the installer.
2. macOS may warn that the package cannot be opened because it is from an unknown developer. Right-clicking on the package and choosing "Open" will allow you to run it.
3. Once the installer is finished, you can configure the service by editing `/Library/Application Support/ShutdownOnLan.plist`. It belongs to the `system` user, so you'll need to use `sudo` to edit it (try `sudo nano /Library/Application\ Support/ShutdownOnLan/ShutDownOnLan.plist`). 
4. Once settings are in place, restart the service by running:
```
sudo launchctl stop com.jkmassel.shutdownonlan
sudo launchctl start com.jkmassel.shutdownonlan
```

#### Linux
Packages are provided for `x86_64` and `aarch64` (for instance, a Raspberry Pi):

- **Debian, Ubuntu and Raspberry Pi OS:** `sudo apt install ./shutdown-on-lan-linux-x86_64.deb`
- **Fedora, RHEL and derivatives:** `sudo dnf install ./shutdown-on-lan-linux-x86_64.rpm`

The package installs a `systemd` service, which starts immediately and on every boot. It runs as root, because shutting the machine down requires it, but is otherwise sandboxed.

1. Run `sudo shutdown-on-lan get --secret` to see this machine's secret, and give it to your control system.
2. Optionally, configure the service by editing `/etc/shutdown-on-lan.toml`, or with `shutdown-on-lan set` (for instance, `sudo shutdown-on-lan set --allowed-sources 10.0.1.50`). The file is only readable by root, because it holds the secret. Upgrading the package never overwrites your changes.

```toml
port_number = 53632
addresses = []
secret = "3f9c2a7e5b1d8f4a6c0e9b2d7a5f1c3e"
allowed_sources = ["10.0.1.50"]
```

3. Once settings are in place, restart the service by running `sudo systemctl restart shutdown-on-lan`.
4. If you're running a firewall, allow the service through it. The package includes profiles for both `firewalld` and `ufw`, which aren't enabled by default:

```
sudo firewall-cmd --permanent --add-service=shutdown-on-lan && sudo firewall-cmd --reload
sudo ufw allow shutdown-on-lan
```

The profiles use the default port – if you've changed it, change it in `/usr/lib/firewalld/services/shutdown-on-lan.xml` or `/etc/ufw/applications.d/shutdown-on-lan` too.

##### Other distributions
The `.tar.gz` contains a statically linked binary that runs on any distribution, along with the `systemd` unit and the firewall profiles:

```
tar -xzf shutdown-on-lan-linux-x86_64.tar.gz && cd shutdown-on-lan
sudo install -m 755 shutdown-on-lan /usr/bin/
sudo shutdown-on-lan init
sudo install -m 644 shutdown-on-lan.service /etc/systemd/system/
sudo systemctl daemon-reload && sudo systemctl enable --now shutdown-on-lan
```

The service can't write to `/etc`, so `shutdown-on-lan init` creates `/etc/shutdown-on-lan.toml` (with a random secret) before it starts.

### How to use

#### Shutting Down

The service can be triggered from a remote machine by sending a string containing the secret to the correct port over TCP. For instance, to do so using `netcat`, you could run:

`echo 'your-secret' | nc 10.0.1.100 53632`

The secret can be terminated by a newline (`\n` or `\r\n`) or by closing the connection. Several newline-separated attempts can be sent over a single connection. After a wrong secret, further attempts from the same client address are delayed – starting at 100ms and doubling with each wrong secret, up to 5 seconds per attempt. The delay resets after 5 minutes without any attempts.

#### Detecting State

This service can also allow a client to maintain a connection to the socket without sending data in order to determine whether the target machine is powered on. Up to 32 connections can be held open at once – further connections are closed immediately.

### Debugging Issues

#### Mac
The macOS service writes error messages to `/var/log/shutdownonlan.error.log` and an audit log (including the source IP address of any remote connections) to `/var/log/shutdownonlan.log`. Additionally, if there are configuration or permission issues with the service, macOS will log them to `/var/log/system.log`.

#### Linux
The service logs to the `systemd` journal, including the source IP address of any remote connections. To follow it, run `journalctl -u shutdown-on-lan -f`.

#### Windows
The Windows service writes its log (including the source IP address of any remote connections) to `C:\ProgramData\ShutdownOnLan\shutdown-on-lan.log`.

The Windows version can be run in standalone mode by running `shutdown-on-lan.exe run` from an Administrative PowerShell. This runs the same code that's used in the service, and should help debug any issues.