extern crate heim;
extern crate log;
extern crate simplelog;
#[macro_use]
extern crate itertools;
extern crate windows_service;

use futures::*;
use heim::net::*;
use log::info;
use simplelog::*;
use std::fs::File;

#[cfg(windows)]
fn main() -> windows_service::Result<()> {
    init_logging();
    return shutdown_on_lan_service::run();
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

#[derive(Debug)]
struct InterfaceDetails {
    name: String,
    mac_address: MacAddr6,
    ip_address: std::net::SocketAddr,
}

#[derive(Debug, Clone)]
struct PartialInterface {
    name: String,
    mac_address: Option<MacAddr6>,
    ip_v4_address: Option<std::net::Ipv4Addr>,
    ip_v6_address: Option<std::net::Ipv6Addr>,
}

impl PartialInterface {
    fn merge(&self, other: PartialInterface) -> PartialInterface {
        if let Some(my_mac_address) = other.mac_address {
            return PartialInterface {
                name: self.name.clone(),
                mac_address: Some(my_mac_address),
                ip_v4_address: self.ip_v4_address,
                ip_v6_address: self.ip_v6_address,
            };
        }

        if let Some(my_ip_v4_address) = other.ip_v4_address {
            return PartialInterface {
                name: self.name.clone(),
                mac_address: self.mac_address,
                ip_v4_address: Some(my_ip_v4_address),
                ip_v6_address: self.ip_v6_address,
            };
        }

        if let Some(my_ip_v6_address) = other.ip_v6_address {
            return PartialInterface {
                name: self.name.clone(),
                mac_address: self.mac_address,
                ip_v4_address: self.ip_v4_address,
                ip_v6_address: Some(my_ip_v6_address),
            };
        }

        return PartialInterface {
            name: self.name.clone(),
            mac_address: self.mac_address,
            ip_v4_address: self.ip_v4_address,
            ip_v6_address: self.ip_v6_address,
        };
    }
}

impl InterfaceDetails {
    fn with(
        name: String,
        mac_address: Option<MacAddr6>,
        ip_address: Option<std::net::SocketAddr>,
    ) -> Option<InterfaceDetails> {
        let mut details: Option<InterfaceDetails> = None;

        if let Some(__mac) = mac_address {
            if let Some(__ip) = ip_address {
                details = Some(InterfaceDetails {
                    name: name,
                    mac_address: __mac,
                    ip_address: __ip,
                });
            }
        }

        details
    }
}

fn get_mac_address() {
    use itertools::Itertools;
    use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};

    let mut nic_stream = heim::net::nic();

    let mut components = Vec::<PartialInterface>::new();

    while let Some(wrapped_nic) = executor::block_on(nic_stream.next()) {
        let nic = wrapped_nic.unwrap();

        let name = nic.name().to_string();
        let mut mac_address: Option<MacAddr6> = None;
        let mut v4_ip_address: Option<std::net::Ipv4Addr> = None;
        let mut v6_ip_address: Option<std::net::Ipv6Addr> = None;

        if let Address::Link(link) = nic.address() {
            if let MacAddr::V6(v6_address) = link {
                mac_address = Some(v6_address).clone();
            }
        }

        if let Address::Inet(ip_address) = nic.address() {
            if let SocketAddr::V4(v4_address) = ip_address {
                v4_ip_address = Some(v4_address.ip().clone());
            }

            if let SocketAddr::V6(v6_address) = ip_address {
                v6_ip_address = Some(v6_address.ip().clone())
            }
        }

        let component = PartialInterface {
            name: name,
            mac_address: mac_address,
            ip_v4_address: v4_ip_address,
            ip_v6_address: v6_ip_address,
        };

        println!(
            "===\n{:?}\n \t{:?}\t{:?}\n\n",
            nic, mac_address, v4_ip_address
        );

        components.push(component);
    }

    use std::collections::HashMap;
    let mut interfaces = HashMap::<String, PartialInterface>::new();

    for component in components {
        let name = component.name.clone();

        if let Some(interface) = interfaces.get(&name).clone() {
            interfaces.insert(name, interface.merge(component));
        } else {
            interfaces.insert(name, component.clone());
        }
    }

    println!("=== {:?}", interfaces);

    // let groups = components.iter()
    //     .group_by(|component| component.0)
    //     .map( |group|

    //         group.foo;

    //         let name = nic.name().to_string();
    //         let mut mac_address: Option<MacAddr6> = None;
    //         let mut v4_ip_address: Option<std::net::SocketAddr> = None;

    //         name
    //     )
    //     .collect::<Vec<_>>();

    // for (key, group) in groups {
    //     println!(":: {:?} {:?}", key, group);
    // }

    // if let Some(interface) = InterfaceDetails::with(name, mac_address, v4_ip_address) {
    //     interfaces.push(interface);
    // }
    // let nics = Vec::<heim::net::Nic>::new();

    // let groups = nics.into_iter().group_by( |nic| nic.name() );
}

#[cfg(not(windows))]
fn main() {
    init_logging();
    print!("Other test\n");
    get_mac_address();
    print!("Exiting");
}

#[cfg(windows)]
mod shutdown_on_lan_service {

    use std::{
        ffi::OsString,
        //net::{IpAddr, SocketAddr, UdpSocket},
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
