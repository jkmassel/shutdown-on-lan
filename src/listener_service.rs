use std::io::Read;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::thread;
use system_shutdown::shutdown;

use crate::configuration::AppConfiguration;

pub fn run(configuration: &AppConfiguration) {
    let listener = TcpListener::bind(configuration).unwrap();

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let secret = configuration.secret.clone();

                thread::spawn(move || {
                    log::info!("New connection: {}", stream.peer_addr().unwrap());
                    handle_stream(stream, &secret)
                });
            }
            Err(e) => {
                log::error!("Error initializing socket: {}", e);
            }
        }
    }
}

pub fn handle_stream(mut stream: TcpStream, secret: &str) {
    let mut buffer = String::new();

    match stream.read_to_string(&mut buffer) {
        Ok(_) => {
            let input = &buffer.trim();
            log::debug!("Comparing {} and {}", input, secret);
            if &secret == input {
                match shutdown() {
                    Ok(_) => {
                        log::info!("Shutting down - source: {}", stream.peer_addr().unwrap())
                    }
                    Err(error) => log::error!("Failed to shut down: {}", error),
                }
            }

            log::debug!("Received message: {}", input);
        }
        Err(_) => {
            log::error!(
                "An error occurred, terminating connection with {}",
                stream.peer_addr().unwrap()
            );
            stream.shutdown(Shutdown::Both).unwrap();
        }
    }
}
