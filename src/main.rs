extern crate log;
extern crate simplelog;
extern crate system_shutdown;
extern crate windows_service;

use log::info;
use simplelog::*;
use std::fs::File;
use std::net::{ IpAddr, Ipv4Addr };
use std::net::{ToSocketAddrs, SocketAddr};
use std::vec;

#[cfg(windows)]
fn main() -> windows_service::Result<()> {
    init_logging();

    let config = AppConfiguration(
        53632,
        [
            [127, 0, 0, 1],
            [10, 0, 1, 39],
        ],
        "secret!"
    );

    return shutdown_on_lan_service::run(config);
}

fn init_logging() {
    CombinedLogger::init(vec![WriteLogger::new(
        LevelFilter::Info,
        Config::default(),
        File::create("output.log").unwrap(),
    )])
    .unwrap();

    info!("File Logger Initialized");
}

#[cfg(not(windows))]
fn main() {
    init_logging();

    let config = AppConfiguration {
        port_number: 53632,
        addresses: [
            IpAddr::from(Ipv4Addr::new(127, 0, 0, 1)),
            IpAddr::from(Ipv4Addr::new(10, 0, 1, 39)),
        ].to_vec(),
        secret: "secret!".to_string()
    };

    listener_service::run(&config);
}

pub struct AppConfiguration {
    port_number: u16,
    addresses: Vec<IpAddr>,
    secret: String,
}

impl ToSocketAddrs for AppConfiguration {
    type Iter = vec::IntoIter<SocketAddr>;

    fn to_socket_addrs(&self) -> std::io::Result<vec::IntoIter<SocketAddr>> {
        let mut addresses : Vec<SocketAddr> = Vec::new();
        
        for ip in self.addresses.clone() {
            // let mut address = SocketAddr::from((ip, self.port_number));
            addresses.push(SocketAddr::from((ip, self.port_number)));
        }

        let ret = addresses.into_iter();
        return Ok(ret);
    }
}

mod listener_service {
    use crate::AppConfiguration;
    use std::io::Read;
    use std::net::{Shutdown, TcpListener, TcpStream};
    use system_shutdown::shutdown;

    pub fn run(configuration: &AppConfiguration) {
        let listener = TcpListener::bind(configuration).unwrap();

        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    println!("New connection: {}", stream.peer_addr().unwrap());
                    handle_stream(stream, &configuration.secret)
                }
                Err(e) => {
                    eprintln!("Error initializing socket: {}", e);
                }
            }
        }
    }

    pub fn handle_stream(mut stream: TcpStream, secret: &String) {
        let mut buffer = String::new();

        match stream.read_to_string(&mut buffer) {
            Ok(_) => {
                let input = &buffer.trim();
                if secret == input {

                    match shutdown() {
                        Ok(_) => println!("Shutting down."),
                        Err(error) => eprintln!("Failed to shut down: {}", error),
                    }
                }
            }
            Err(_) => {
                eprintln!(
                    "An error occurred, terminating connection with {}",
                    stream.peer_addr().unwrap()
                );
                stream.shutdown(Shutdown::Both).unwrap();
            }
        }
    }
}

#[cfg(windows)]
mod shutdown_on_lan_service {

    use std::{
        ffi::OsString,
        sync::mpsc,
        thread,
        time::Duration,
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
    // entry (my_service_main).
    define_windows_service!(ffi_service_main, my_service_main);

    // Service entry function which is called on background thread by the system with service
    // parameters. There is no stdout or stderr at this point so make sure to configure the log
    // output to file if needed.
    pub fn my_service_main(_arguments: Vec<OsString>) {
        if let Err(_e) = run_service() {
            // Handle the error, by logging or something.
        }
    }

    fn print_type_of<T>(_: &T) {
        info!("{}", std::any::type_name::<T>())
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

        loop {
            info!(target: SERVICE_NAME, "HERE");

            thread::sleep(Duration::from_millis(1000));

            // Poll shutdown event.
            match shutdown_rx.recv_timeout(Duration::from_secs(1)) {
                // Break the loop either upon stop or channel disconnect
                Ok(_) | Err(mpsc::RecvTimeoutError::Disconnected) => break,

                // Continue work if no events were received within the timeout
                Err(mpsc::RecvTimeoutError::Timeout) => (),
            };
        }

        warn!("Attempting to exit");

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
