extern crate exitcode;
extern crate log;
extern crate simplelog;
extern crate system_shutdown;

use crate::configuration::{format_addresses, AppConfiguration};
use anyhow::{Context, Result};
use simplelog::*;
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::process;
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
    /// Run the tool in standalone mode (mostly only useful on Windows, the same as running with no arguments on other platforms)
    Run {},
}

fn main() -> Result<()> {
    let args = AppArguments::from_args();

    init_logging(args.command.is_none());

    match args.command {
        None => run()?,
        Some(Command::Set {
            port,
            ip_address,
            secret,
        }) => {
            log::debug!("Updating Configuration: {:?},{:?}", port, ip_address);

            if port.is_none() && ip_address.is_none() && secret.is_none() {
                println!("You must specify an option to set. Use --help to list options.");
                process::exit(exitcode::USAGE);
            }

            let mut config = get_app_configuration()?;

            if let Some(port) = port {
                println!("Set port {port:?}");
                config.port_number = port;
            }

            if let Some(ip_address) = ip_address {
                config
                    .set_addresses(&ip_address)
                    .with_context(|| format!("Invalid IP address list: {ip_address:?}"))?;
                println!("Set IP Addresses: {}", format_addresses(&config.addresses));
            }

            if let Some(secret) = secret {
                config.set_secret(secret)?;
                println!("Secret updated");
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
                println!(
                    "Listening IP Addresses: {}",
                    format_addresses(&config.addresses)
                );
            }
        }
        Some(Command::Run {}) => {
            println!("Running in standalone mode");
            run_standalone()?
        }
    }

    Ok(())
}

fn get_app_configuration() -> Result<AppConfiguration> {
    AppConfiguration::load().context("Unable to read the configuration file")
}

fn init_logging(running_as_service: bool) {
    let level = if cfg!(debug_assertions) {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };

    let mut loggers: Vec<Box<dyn SharedLogger>> = vec![TermLogger::new(
        level,
        Config::default(),
        TerminalMode::Mixed,
        ColorChoice::Auto,
    )];

    if let Some(path) = log_file_path(running_as_service) {
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => loggers.push(WriteLogger::new(level, Config::default(), file)),
            Err(error) => eprintln!("Unable to open log file at {}: {}", path.display(), error),
        }
    }

    if let Err(error) = CombinedLogger::init(loggers) {
        eprintln!("Unable to initialize logging: {}", error);
    }

    log::debug!("Logger Initialized");
}

// A Windows service has no terminal, so write its log to a file
#[cfg(windows)]
fn log_file_path(running_as_service: bool) -> Option<PathBuf> {
    if !running_as_service {
        return debug_log_file_path();
    }

    let directory = std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
        .join("ShutdownOnLan");

    if let Err(error) = std::fs::create_dir_all(&directory) {
        eprintln!(
            "Unable to create log directory {}: {}",
            directory.display(),
            error
        );
        return None;
    }

    Some(directory.join("shutdown-on-lan.log"))
}

// launchd captures the terminal output on macOS
#[cfg(not(windows))]
fn log_file_path(_running_as_service: bool) -> Option<PathBuf> {
    debug_log_file_path()
}

fn debug_log_file_path() -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        Some(PathBuf::from("shutdown-on-lan.log"))
    } else {
        None
    }
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
    let config = get_app_configuration()?;
    listener_service::run(&config).context("Unable to start listening for connections")
}
