extern crate log;
extern crate simplelog;
extern crate system_shutdown;
extern crate windows_service;

use simplelog::*;
use std::fs::File;
use std::vec;

mod configuration;
mod listener_service;
mod windows_listener_service;

#[cfg(windows)]
fn main() -> windows_service::Result<()> {
    init_logging();
    use crate::windows_listener_service::shutdown_on_lan_service;
    return shutdown_on_lan_service::run();
}

#[cfg(not(windows))]
fn main() {
    use crate::{
        configuration::AppConfiguration,
        listener_service::listener_service,
    };

    init_logging();

    let config: AppConfiguration = AppConfiguration::default();
    listener_service::run(&config);
}

fn init_logging() {

    if !cfg!(debug_assertions) {
        return
    }

    CombinedLogger::init(vec![WriteLogger::new(
        LevelFilter::Info,
        Config::default(),
        File::create("output.log").unwrap(),
    )])
    .unwrap();

    log::debug!("File Logger Initialized");
}
