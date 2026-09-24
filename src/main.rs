use crate::configuration::{AppConfiguration, describe_addresses, format_addresses};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::process;

mod configuration;
mod listener_service;
mod logging;
mod windows_listener_service;

#[derive(Debug, Parser)]
#[command(
    name = "shutdown-on-lan",
    version,
    about = "A tool for implementing the opposite of wake-on-LAN – the ability to remotely shut down a machine."
)]
struct AppArguments {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print the current configuration
    Get {
        /// Print the port number that this tool listens on (according to the local configuration file, if present)
        #[arg(long = "port")]
        port: bool,

        /// Print the IP address(es) that this tool listens on (according to the local configuration file, if present)
        #[arg(long = "ip-addresses")]
        ip_addresses: bool,

        /// Print the client IP address(es) allowed to connect (according to the local configuration file, if present)
        #[arg(long = "allowed-sources")]
        allowed_sources: bool,

        /// Print the secret that shuts this machine down
        #[arg(long = "secret")]
        secret: bool,
    },
    /// Change the configuration
    Set {
        /// The port to listen on
        #[arg(long = "port")]
        port: Option<u16>,

        /// A comma-separated list of local interface IP addresses to accept connections on. Pass an empty string to accept connections on every interface.
        #[arg(long = "ip-address")]
        ip_address: Option<String>,

        /// The secret that shuts this machine down
        #[arg(long = "secret")]
        secret: Option<String>,

        /// A comma-separated list of client IP addresses allowed to connect. Pass an empty string to allow any client.
        #[arg(long = "allowed-sources")]
        allowed_sources: Option<String>,
    },
    /// Create the configuration, with a random secret, if it doesn't exist yet
    Init {},
    /// Run the tool in standalone mode (mostly only useful on Windows, the same as running with no arguments on other platforms)
    Run {},
}

fn main() -> Result<()> {
    let args = AppArguments::parse();

    let using_system_log = logging::init(args.command.is_none());

    match args.command {
        None => {
            if let Err(error) = run() {
                if using_system_log {
                    log::error!("{:#}", error);
                    process::exit(1);
                }
                return Err(error);
            }
        }
        Some(Command::Set {
            port,
            ip_address,
            secret,
            allowed_sources,
        }) => {
            log::debug!(
                "Updating Configuration: {:?},{:?},{:?}",
                port,
                ip_address,
                allowed_sources
            );

            if port.is_none()
                && ip_address.is_none()
                && secret.is_none()
                && allowed_sources.is_none()
            {
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
                println!(
                    "Set IP Addresses: {}",
                    describe_addresses(&config.addresses)
                );
            }

            if let Some(secret) = secret {
                config.set_secret(secret)?;
                println!("Secret updated");
            }

            if let Some(allowed_sources) = allowed_sources {
                config
                    .set_allowed_sources(&allowed_sources)
                    .with_context(|| format!("Invalid IP address list: {allowed_sources:?}"))?;
                println!("Set Allowed Sources: {}", describe_sources(&config));
            }

            log::debug!("Saving Configuration");

            config.save()?;

            println!("Configuration Changes Saved.");

            // The service only reads its configuration at startup
            println!("Restart the service to apply them: {RESTART_COMMAND}");
        }
        Some(Command::Get {
            port,
            ip_addresses,
            allowed_sources,
            secret,
        }) => {
            let config = get_app_configuration()?;

            if port {
                println!("Current Port: {:?}", config.port_number);
            }

            if ip_addresses {
                println!(
                    "Listening IP Addresses: {}",
                    describe_addresses(&config.addresses)
                );
            }

            if allowed_sources {
                println!("Allowed Sources: {}", describe_sources(&config));
            }

            if secret {
                println!("Secret: {}", config.secret);
            }
        }
        Some(Command::Init {}) => {
            get_app_configuration()?;
            println!("Configuration ready. To see the secret, run `shutdown-on-lan get --secret`.");
        }
        Some(Command::Run {}) => {
            println!("Running in standalone mode");
            run_standalone()?
        }
    }

    Ok(())
}

#[cfg(target_os = "linux")]
const RESTART_COMMAND: &str = "sudo systemctl restart shutdown-on-lan";

#[cfg(target_os = "macos")]
const RESTART_COMMAND: &str = "sudo launchctl kickstart -k system/com.jkmassel.shutdownonlan";

#[cfg(windows)]
const RESTART_COMMAND: &str = "Restart-Service ShutdownOnLan (from an Administrative PowerShell)";

fn describe_sources(config: &AppConfiguration) -> String {
    if config.allowed_sources.is_empty() {
        "any".to_string()
    } else {
        format_addresses(&config.allowed_sources)
    }
}

fn get_app_configuration() -> Result<AppConfiguration> {
    AppConfiguration::load().context("Unable to read the configuration file")
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
    listener_service::run(config).context("Unable to listen for connections")
}
