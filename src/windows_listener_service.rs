#[cfg(windows)]
pub mod shutdown_on_lan_service {
    use crate::{configuration::AppConfiguration, listener_service};

    use std::{ffi::OsString, panic, sync::mpsc, thread, time::Duration};

    use anyhow::anyhow;

    use windows_service::{
        Result, define_windows_service,
        service::{
            ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
            ServiceType,
        },
        service_control_handler::{self, ServiceControlHandlerResult},
        service_dispatcher,
    };

    // Must match the name the installer registers the service with in `Product.wxs`
    const SERVICE_NAME: &str = "ShutdownOnLan";
    const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

    pub fn run() -> anyhow::Result<()> {
        // Register generated `ffi_service_main` with the system and start the service, blocking
        // this thread until the service is stopped.
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)
            .map_err(|error| anyhow!("Unable to start service: {}", error))
    }

    // Generate the windows service boilerplate.
    // The boilerplate contains the low-level service entry function (ffi_service_main) that parses
    // incoming service arguments into Vec<OsString> and passes them to user defined service
    // entry (service_main).
    define_windows_service!(ffi_service_main, service_main);

    enum ServiceEvent {
        Stop,
        ListenerStopped,
    }

    // Service entry function which is called on background thread by the system with service
    // parameters. There is no stdout or stderr at this point, so logging goes to the event log.
    pub fn service_main(_arguments: Vec<OsString>) {
        if let Err(error) = run_service() {
            log::error!("Service failed: {}", error);
        }
    }

    pub fn run_service() -> Result<()> {
        // Stop requests and listener failures are both delivered on this channel
        let (event_tx, event_rx) = mpsc::channel();

        let handler_tx = event_tx.clone();

        // Define system service event handler that will be receiving service events.
        let event_handler = move |control_event| -> ServiceControlHandlerResult {
            match control_event {
                // Notifies a service to report its current status information to the service
                // control manager. Always return NoError even if not implemented.
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,

                // Handle stop
                ServiceControl::Stop => {
                    let _ = handler_tx.send(ServiceEvent::Stop);
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
            process_id: None,
        })?;

        let exit_code = match AppConfiguration::load() {
            Ok(config) => {
                log::info!("Forking listener service thread");
                thread::spawn(move || {
                    // Always report that the listener stopped, even if it panicked – otherwise the
                    // service would keep running without listening
                    match panic::catch_unwind(|| listener_service::run(config)) {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => log::error!("Listener service stopped: {}", error),
                        // The panic hook has already logged the details
                        Err(_) => log::error!("Listener service stopped: it panicked"),
                    }

                    let _ = event_tx.send(ServiceEvent::ListenerStopped);
                });
                log::info!("Started listener service");

                match event_rx.recv() {
                    Ok(ServiceEvent::ListenerStopped) => ServiceExitCode::ServiceSpecific(1),
                    Ok(ServiceEvent::Stop) | Err(_) => ServiceExitCode::Win32(0),
                }
            }
            Err(error) => {
                log::error!("Unable to read configuration: {}", error);
                ServiceExitCode::ServiceSpecific(1)
            }
        };

        log::info!("Attempting to exit");

        // Tell the system that service has stopped.
        status_handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code,
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })?;

        Ok(())
    }
}
