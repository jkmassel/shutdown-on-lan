use std::io::{self, BufRead, BufReader, Read};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use system_shutdown::shutdown;

use crate::configuration::{format_addresses, AppConfiguration, MAX_SECRET_LENGTH};

/// Clients may hold a connection open indefinitely to detect whether the machine is on, so cap how many
/// we'll hold at once to avoid exhausting threads and file descriptors.
const MAX_OPEN_CONNECTIONS: usize = 32;

pub fn run(configuration: &AppConfiguration) -> io::Result<()> {
    let listener = TcpListener::bind(configuration)?;
    log::info!(
        "Listening on port {} for connections to {}",
        configuration.port_number,
        format_addresses(&configuration.addresses)
    );

    let open_connections = Arc::new(AtomicUsize::new(0));

    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                log::error!("Unable to accept connection: {}", error);
                continue;
            }
        };

        let peer = describe_peer(&stream);

        let interface_ip = match stream.local_addr() {
            Ok(address) => address.ip(),
            Err(error) => {
                log::warn!("Dropping connection from {} – {}", peer, error);
                continue;
            }
        };

        if !configuration.accepts_connections_on(&interface_ip) {
            log::info!(
                "Rejected connection from {} on {:?} – the configuration only allows connections on {}",
                peer,
                interface_ip,
                format_addresses(&configuration.addresses)
            );
            continue;
        }

        if open_connections.fetch_add(1, Ordering::SeqCst) >= MAX_OPEN_CONNECTIONS {
            open_connections.fetch_sub(1, Ordering::SeqCst);
            log::warn!(
                "Rejected connection from {} – too many open connections",
                peer
            );
            continue;
        }

        let secret = configuration.secret.clone();
        let open_connections = Arc::clone(&open_connections);

        thread::spawn(move || {
            log::info!("New connection: {}", peer);
            handle_stream(stream, &secret, &peer);
            open_connections.fetch_sub(1, Ordering::SeqCst);
        });
    }

    Ok(())
}

fn describe_peer(stream: &TcpStream) -> String {
    stream
        .peer_addr()
        .map(|address| address.to_string())
        .unwrap_or_else(|_| "unknown peer".to_string())
}

fn handle_stream(stream: TcpStream, secret: &str, peer: &str) {
    match wait_for_secret(BufReader::new(stream), secret) {
        Ok(true) => {
            log::info!("Shutting down - source: {}", peer);

            if let Err(error) = shutdown() {
                log::error!("Failed to shut down: {}", error);
            }
        }
        Ok(false) => log::info!("Connection closed by {}", peer),
        Err(error) => log::warn!("Terminating connection with {}: {}", peer, error),
    }
}

/// Reads newline-delimited messages until one matches `secret` (returning `true`) or the client closes
/// the connection (returning `false`). The last message doesn't need a trailing newline.
fn wait_for_secret<R: BufRead>(mut reader: R, secret: &str) -> io::Result<bool> {
    // Leave room for a `\r\n` terminator
    let limit = MAX_SECRET_LENGTH as u64 + 2;
    let mut message = Vec::new();

    loop {
        message.clear();

        let length = (&mut reader).take(limit).read_until(b'\n', &mut message)?;

        if length == 0 {
            return Ok(false);
        }

        if length as u64 == limit && !message.ends_with(b"\n") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "message exceeds the maximum secret length",
            ));
        }

        if let Ok(input) = std::str::from_utf8(&message) {
            if secrets_match(input.trim().as_bytes(), secret.as_bytes()) {
                return Ok(true);
            }
        }

        log::debug!("Received a message that didn't match the secret");
    }
}

/// Compares in constant time (for a given length) so the secret can't be recovered through timing.
fn secrets_match(input: &[u8], secret: &[u8]) -> bool {
    if secret.is_empty() || input.len() != secret.len() {
        return false;
    }

    input
        .iter()
        .zip(secret)
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const SECRET: &str = "Super Secret String";

    fn wait_for(input: &[u8]) -> io::Result<bool> {
        wait_for_secret(Cursor::new(input.to_vec()), SECRET)
    }

    #[test]
    fn test_secret_without_a_trailing_newline_matches() {
        assert!(wait_for(b"Super Secret String").unwrap());
    }

    #[test]
    fn test_secret_with_line_endings_matches() {
        assert!(wait_for(b"Super Secret String\n").unwrap());
        assert!(wait_for(b"Super Secret String\r\n").unwrap());
        assert!(wait_for(b"  Super Secret String  \r\n").unwrap());
    }

    #[test]
    fn test_secret_on_a_later_line_matches() {
        assert!(wait_for(b"wrong\nSuper Secret String\n").unwrap());
    }

    #[test]
    fn test_wrong_secret_does_not_match() {
        assert!(!wait_for(b"Super Secret Strin").unwrap());
        assert!(!wait_for(b"Super Secret String!").unwrap());
        assert!(!wait_for(b"\xff\xfe").unwrap());
    }

    #[test]
    fn test_closing_without_sending_anything_does_not_match() {
        assert!(!wait_for(b"").unwrap());
    }

    #[test]
    fn test_oversized_message_is_rejected() {
        let input = "a".repeat(MAX_SECRET_LENGTH + 2);
        assert!(wait_for(input.as_bytes()).is_err());
    }

    #[test]
    fn test_longest_allowed_secret_matches() {
        let secret = "a".repeat(MAX_SECRET_LENGTH);
        let input = format!("{}\r\n", secret);
        assert!(wait_for_secret(Cursor::new(input.into_bytes()), &secret).unwrap());
    }

    #[test]
    fn test_empty_secret_never_matches() {
        assert!(!wait_for_secret(Cursor::new(b"\n".to_vec()), "").unwrap());
        assert!(!secrets_match(b"", b""));
    }
}
