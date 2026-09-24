//! The service logs to the platform's own log store – the systemd journal on Linux, unified logging on
//! macOS, and the Event Log on Windows – so the system takes care of retention, rotation, and viewing.
//! Anything run from a terminal logs to the terminal instead.

use anyhow::Result;
use log::Level;
use simplelog::{ColorChoice, Config, TermLogger, TerminalMode};
use std::panic;

/// Returns whether the log goes to the system log. If it does, nothing reads the process's own output.
pub fn init(running_as_service: bool) -> bool {
    let level = if cfg!(debug_assertions) {
        Level::Debug
    } else {
        Level::Info
    };

    if running_as_service && system_log_available() {
        match init_system_log(level) {
            Ok(()) => {
                log::set_max_level(level.to_level_filter());
                log_panics();
                log::debug!("Logging to the system log");
                return true;
            }
            Err(error) => eprintln!("Unable to use the system log: {:#}", error),
        }
    }

    if let Err(error) = TermLogger::init(
        level.to_level_filter(),
        Config::default(),
        TerminalMode::Mixed,
        ColorChoice::Auto,
    ) {
        eprintln!("Unable to initialize logging: {}", error);
    }

    log::debug!("Logger Initialized");
    false
}

// The panic message would otherwise only be written to stderr
fn log_panics() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        log::error!("{}", info);
        default_hook(info);
    }));
}

// systemd sets `JOURNAL_STREAM` when the service's output goes to the journal
#[cfg(target_os = "linux")]
fn system_log_available() -> bool {
    systemd_journal_logger::connected_to_journal()
}

#[cfg(target_os = "linux")]
fn init_system_log(_level: Level) -> Result<()> {
    systemd_journal_logger::JournalLog::new()?.install()?;
    Ok(())
}

// launchd and the service control manager don't give the service a terminal
#[cfg(not(target_os = "linux"))]
fn system_log_available() -> bool {
    use std::io::IsTerminal;
    !std::io::stderr().is_terminal()
}

#[cfg(target_os = "macos")]
fn init_system_log(level: Level) -> Result<()> {
    // Unified logging stores `info!` as the persisted "default" level, so it survives a reboot
    oslog::OsLogger::new("com.jkmassel.shutdownonlan")
        .level_filter(level.to_level_filter())
        .init()?;
    Ok(())
}

#[cfg(windows)]
fn init_system_log(level: Level) -> Result<()> {
    // The installer registers this event source, with the executable as its message file
    eventlog::init("ShutdownOnLan", level)?;
    Ok(())
}
