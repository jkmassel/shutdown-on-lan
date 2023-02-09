extern crate exitcode;
extern crate log;
extern crate simplelog;
extern crate system_shutdown;

use crate::configuration::AppConfiguration;
use anyhow::{Context, Result};
use simplelog::*;
use std::fs::File;
use std::process;
use std::vec;
use structopt::StructOpt;

mod configuration;
mod listener_service;
mod windows_listener_service;

#[derive(Debug, StructOpt)]
#[structopt(
    name = "shutdown-on-lan",
    about = "A tool for implementing the opposite of wake-on-LAN – the ability to remotely shut down a machine."
)]
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

        #[structopt(long = "ip-address")]
        ip_address: Option<String>,

        #[structopt(long = "secret")]
        secret: Option<String>,
    },
    /// Perform Installation Tasks (only used on Windows)
    Install {},
    /// Run the tool in standalone mode (mostly only useful on Windows, the same as running with no arguments on other platforms)
    Run {},
}

fn main() -> Result<()> {
    init_logging();

    let args = AppArguments::from_args();

    match args.command {
        None => run()?,
        Some(Command::Set {
            port,
            ip_address,
            secret,
        }) => {
            log::debug!(
                "Updating Configuartion: {:?},{:?},{:?}",
                port,
                ip_address,
                secret
            );

            let mut config = get_app_configuration()?;

            if port.is_none() && ip_address.is_none() && secret.is_none() {
                println!("You must specify an option to set. Use --help to list options.");
                process::exit(exitcode::USAGE);
            }

            if let Some(port) = port {
                println!("Set port {port:?}");
                config.port_number = port;
            }

            if let Some(ip_address) = ip_address {
                println!("Set IP Addresses: {ip_address:?}");
                config.set_addresses(ip_address);
            }

            if let Some(secret) = secret {
                println!("Set Secret: {secret:?}");
                config.secret = secret;
            }

            log::debug!("Saving Configuration");

            config.save()?;

            println!("Configuration Changes Saved.");
        }
        Some(Command::Get { port, ip_addresses }) => {
            let config = get_app_configuration()?;

            if port {
                println!("Current Port: {:?}", config.port_number);
            }

            if ip_addresses {
                println!("Listening IP Addresses: {:?}", config.addresses);
            }
        }
        Some(Command::Install {}) => {
            println!("Installing (Windows only)");
            install()?
        }
        Some(Command::Run {}) => {
            println!("Running in standalone mode");
            run_standalone()?
        }
    }

    Ok(())
}

fn validate_app_configuration() -> Result<()> {
    AppConfiguration::validate().context("Unable to validate the configuration file")
}

fn get_app_configuration() -> Result<AppConfiguration> {
    AppConfiguration::fetch().context("Unable to read the configuration file")
}

fn init_logging() {
    if cfg!(debug_assertions) {
        CombinedLogger::init(vec![
            TermLogger::new(
                LevelFilter::Debug,
                Config::default(),
                TerminalMode::Mixed,
                ColorChoice::Auto,
            ),
            WriteLogger::new(
                LevelFilter::Debug,
                Config::default(),
                File::create("shutdown-on-lan.log").unwrap(),
            ),
        ])
        .unwrap();
    } else {
        CombinedLogger::init(vec![TermLogger::new(
            LevelFilter::Info,
            Config::default(),
            TerminalMode::Mixed,
            ColorChoice::Auto,
        )])
        .unwrap();
    }

    log::debug!("File Logger Initialized");
}

#[cfg(windows)]
fn install() -> Result<()> {
    AppConfiguration::create_configuration_storage_if_not_exists()?;
    AppConfiguration::create_configuration_if_not_exists()?;
    crate::windows_listener_service::shutdown_on_lan_service::install()
}

#[cfg(not(windows))]
fn install() -> Result<()> {
    println!("Installation is only required on Windows");
    Ok(())
}

#[cfg(windows)]
fn run() -> Result<()> {
    crate::windows_listener_service::shutdown_on_lan_service::run()
}

#[cfg(not(windows))]
fn run() -> Result<()> {
    run_standalone()
}

fn run_standalone() -> Result<()> {
    validate_app_configuration()?;
    let config = get_app_configuration()?;
    listener_service::run(&config);

    Ok(())
}
