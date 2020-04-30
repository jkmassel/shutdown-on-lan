extern crate log;
extern crate simplelog;
extern crate system_shutdown;

use simplelog::*;
use std::fs::File;
use std::vec;

mod configuration;
mod listener_service;
mod windows_listener_service;

#[cfg(windows)]
fn main() -> windows_service::Result<()> {
    extern crate windows_service;

    init_logging();

    winlog::register("ShutdownOnLan");

    use crate::windows_listener_service::shutdown_on_lan_service;
    return shutdown_on_lan_service::run();
}

#[cfg(not(windows))]
fn main() {
    use crate::{configuration::AppConfiguration, listener_service::listener_service};

    init_logging();

    let config = AppConfiguration::fetch();
    listener_service::run(&config);
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
