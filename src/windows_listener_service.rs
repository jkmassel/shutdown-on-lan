extern crate windows_service;

#[cfg(windows)]
pub mod shutdown_on_lan_service {

    use crate::{configuration::AppConfiguration, listener_service::listener_service};

    use std::{
        ffi::OsString, io::Result as RustResult, path::Path, sync::mpsc, thread, time::Duration,
    };

    use windows_service::{
        define_windows_service,
        service::{
            ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
            ServiceType,
        },
        service_control_handler::{self, ServiceControlHandlerResult},
        service_dispatcher, Result,
    };

    use winreg::enums::*;

    const SERVICE_NAME: &str = "shutdown-on-lan";
    const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

    pub fn run() -> Result<()> {
        // Register generated `ffi_service_main` with the system and start the service, blocking
        // this thread until the service is stopped.
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)
    }

    // Generate the windows service boilerplate.
    // The boilerplate contains the low-level service entry function (ffi_service_main) that parses
    // incoming service arguments into Vec<OsString> and passes them to user defined service
    // entry (service_main).
    define_windows_service!(ffi_service_main, service_main);

    // Service entry function which is called on background thread by the system with service
    // parameters. There is no stdout or stderr at this point so make sure to configure the log
    // output to file if needed.
    pub fn service_main(_arguments: Vec<OsString>) {
        if let Err(_e) = run_service() {
            // Handle the error, by logging or something.
        }
    }

    // fn print_type_of<T>(_: &T) {
    //     info!("{}", std::any::type_name::<T>())
    // }

    pub fn get_configuration() -> RustResult<AppConfiguration> {
        const IP_ADDRESS_KEY: &str = "ip_addresses";
        const PORT_KEY: &str = "port";
        const SECRET_KEY: &str = "secret";

        log::info!("Looking up configuration");

        // Lookup Configuration
        let hkcu = winreg::RegKey::predef(HKEY_LOCAL_MACHINE);
        let path = Path::new("Software").join("ShutdownOnLan");
        let (key, disposition) = hkcu.create_subkey(&path)?;
        log::debug!("Created Subkey");

        match disposition {
            REG_CREATED_NEW_KEY => {
                log::debug!("Created Registry Key - writing new configuration from defaults");

                let configuration = AppConfiguration::default();

                let addresses: Vec<String> = configuration
                    .addresses
                    .iter()
                    .map(|ip| ip.to_string())
                    .collect();

                let joined_addresses = addresses.join(",");
                key.set_value(IP_ADDRESS_KEY, &joined_addresses)?;

                log::debug!("Set IP Addresses to {}", joined_addresses);

                let u32_port_number = configuration.port_number as u32;
                key.set_value(PORT_KEY, &u32_port_number)?;

                log::debug!("Set Port to {}", u32_port_number);

                key.set_value(SECRET_KEY, &configuration.secret)?;
                log::debug!("Set secret to {}", configuration.secret);

                return Ok(configuration);
            }
            REG_OPENED_EXISTING_KEY => {
                let ips_string: String = key.get_value(IP_ADDRESS_KEY)?;
                let ips_list: Vec<&str> = ips_string.split(",").collect();

                let ip_addresses = ips_list
                    .iter()
                    .map(|&ip| ip.parse().ok())
                    .flat_map(|x| x)
                    .collect();

                let port: u32 = key.get_value(PORT_KEY)?;
                let secret: String = key.get_value(SECRET_KEY)?;

                return Ok(AppConfiguration {
                    port_number: port as u16,
                    addresses: ip_addresses,
                    secret,
                });
            }
        };
    }

    pub fn run_service() -> Result<()> {
        // Create a channel to be able to poll a stop event from the service worker loop.
        let (shutdown_tx, shutdown_rx) = mpsc::channel();

        // Define system service event handler that will be receiving service events.
        let event_handler = move |control_event| -> ServiceControlHandlerResult {
            match control_event {
                // Notifies a service to report its current status information to the service
                // control manager. Always return NoError even if not implemented.
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,

                // Handle stop
                ServiceControl::Stop => {
                    shutdown_tx.send(()).unwrap();
                    ServiceControlHandlerResult::NoError
                }

                _ => ServiceControlHandlerResult::NotImplemented,
            }
        };

        // Register system service event handler.
        // The returned status handle should be used to report service status changes to the system.
        let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)?;

        // Tell the system that service is running
        status_handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::Running,
            controls_accepted: ServiceControlAccept::STOP,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
        })?;

        let config = match get_configuration() {
            Ok(configuration) => configuration,
            Err(err) => {
                log::error!("Configuration retrieval error: {}", err);
                log::error!("Unable to read configuration - exiting");
                return Ok(());
            }
        };

        log::debug!("About to start listener service");
        thread::spawn(move || {
            listener_service::run(&config);
        });
        log::info!("Started listener service");

        loop {
            log::debug!(target: SERVICE_NAME, "-- Event Loop --");

            thread::sleep(Duration::from_millis(1000));

            // Poll shutdown event.
            match shutdown_rx.recv_timeout(Duration::from_secs(1)) {
                // Break the loop either upon stop or channel disconnect
                Ok(_) | Err(mpsc::RecvTimeoutError::Disconnected) => break,

                // Continue work if no events were received within the timeout
                Err(mpsc::RecvTimeoutError::Timeout) => (),
            };
        }

        log::info!("Attempting to exit");

        // Tell the system that service has stopped.
        status_handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
        })?;

        Ok(())
    }
}
