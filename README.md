# shutdown-on-lan
A cross-platform lightweight complement to wake on lan.

Allows shutting down a computer remotely with a single TCP packet via external control systems (for instance AMX controllers). 

**Supported Platforms:**
- Windows
- macOS
- Linux (some assembly required)

### Installation

Installers are provided for Windows and macOS, which then require some configuration. By default, access is only allowed from the local machine. The following settings are available:
 
##### IP Address
Customizing the IP address field allows you to specify which interfaces the service will listen on – this address should match that of the relevant interface. It's important that this IP address doesn't change – you should consider adding either a DHCP reservation or using a static address for this interface.

##### Port
Customizing the port field allows you to specify which port the service will listen on. By default, this is set to `53632`.

##### Secret
The secret is the string that's sent to the machine in order to shut it down. By default, this is set to `Super Secret String`. Be sure to use a strong secret for this – anyone on the network with the port number and this secret can shut down your machine!

_The secret cannot be longer than 4096 characters._

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

### How to use

#### Shutting Down

The service can be triggered from a remote machine by sending a string containing the secret to the correct port over TCP. For instance, to do so using `netcat`, you could run:

`echo 'Super Secret String' | nc 10.0.1.100 53632`

#### Detecting State

This service can also allow a client to maintain a connection to the socket without sending data in order to determine whether the target machine is powered on.

### Debugging Issues

#### Mac
The macOS service writes error messages to `/var/log/shutdownonlan.error.log` and an audit log (including the source IP address of any remote connections) to `/var/log/shutdownonlan.log`. Additionally, if there are configuration or permission issues with the service, macOS will log them to `/var/log/system.log`.

#### Windows
The Windows version can be run in standalone mode by running `shutdown-on-lan.exe run` from an Administrative PowerShell. This runs the same code that's used in the service, and should help debug any issues.