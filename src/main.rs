extern crate log;
extern crate simplelog;
extern crate system_shutdown;
extern crate exitcode;

use crate::configuration::AppConfiguration;
use simplelog::*;
use std::fs::File;
use std::vec;
use structopt::StructOpt;
use std::process;


mod configuration;
mod listener_service;
mod windows_listener_service;

#[derive(Debug, StructOpt)]
#[structopt(name = "shutdown-on-lan", about = "A tool for implementing the opposite of wake-on-LAN – the ability to remotely shut down a machine.")]
struct AppArguments {
    #[structopt(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, StructOpt)]
enum Command {
        Get {
        /// Print the port number that this tool listens on (according to the local configuration file, if present)
        #[structopt(long = "port")]
        port: bool,

        /// Print the IP address(es) that this tool listens on (according to the local configuration file, if present)
        #[structopt(long = "ip-addresses")]
        ip_addresses: bool,
    },
    Set {
        #[structopt(long = "port")]
        port: Option<u16>,

        #[structopt(long = "ip-addresses")]
        ip_addresses: Option<String>,

        #[structopt(long = "secret")]
        secret: Option<String>,
    }
}

#[cfg(windows)]
fn main() -> windows_service::Result<()> {
    extern crate windows_service;

    init_logging();
    AppConfiguration::validate();

    use crate::windows_listener_service::shutdown_on_lan_service;
    return shutdown_on_lan_service::run();
}

#[cfg(not(windows))]
use crate::configuration::ConfigurationError;

#[cfg(not(windows))]
fn main() -> Result<(), ConfigurationError> {
    init_logging();

    let args = AppArguments::from_args();

    match args.command {
        None => {
            AppConfiguration::validate()?;
            let config = AppConfiguration::fetch()?;
            listener_service::run(&config);
        },
        Some(Command::Set { port, ip_addresses, secret }) =>  {
            let mut config = AppConfiguration::fetch()?;

            if port.is_none() && ip_addresses.is_none() && secret.is_none() {
                println!("You must specify an option to set. Use --help to list options.");
                process::exit(exitcode::USAGE);
            }

            if let Some(port) = port {
                println!("Set port {:?}", port);
                config.port_number = port;
            }

            if let Some(ip_addresses) = ip_addresses {
                println!("Set IP Addresses: {:?}", ip_addresses);
                config.set_addresses(ip_addresses);
            }

            if let Some(secret) = secret {
                println!("Set Secret: {:?}", secret);
                config.secret = secret;
            }

            config.save()?;

            println!("Configuration Changes Saved.");
        },
        Some(Command::Get { port, ip_addresses }) => {
            let config = AppConfiguration::fetch()?;

            if port {
                println!("Current Port: {:?}", config.port_number);
            }

            if ip_addresses {
                println!("Listening IP Addresses: {:?}", config.addresses);
            }
        }
    }

    Ok(())
}

fn init_logging() {
    let mut loggers = vec![];

    match TermLogger::new(LevelFilter::Warn, Config::default(), TerminalMode::Mixed) {
        Some(logger) => loggers.push(logger as Box<dyn SharedLogger>),
        None => loggers.push(SimpleLogger::new(LevelFilter::Warn, Config::default())),
    }

    if cfg!(debug_assertions) {
        loggers.push(WriteLogger::new(
            LevelFilter::Info,
            Config::default(),
            File::create("output.log").unwrap(),
        ));
    }

    CombinedLogger::init(loggers).unwrap();

    log::debug!("File Logger Initialized");
}
