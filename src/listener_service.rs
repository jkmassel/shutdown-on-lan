use socket2::{Domain, Protocol, SockRef, Socket, TcpKeepalive, Type};
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use system_shutdown::shutdown;

use crate::configuration::{
    AppConfiguration, MAX_SECRET_LENGTH, describe_addresses, format_addresses,
};

/// Clients may hold a connection open indefinitely to detect whether the machine is on, so cap how many
/// we'll hold at once to avoid exhausting threads and file descriptors. Each source only gets a few of
/// them, so that one client can't take every slot and lock the others out.
const MAX_OPEN_CONNECTIONS: usize = 32;
const MAX_OPEN_CONNECTIONS_PER_SOURCE: usize = 4;

/// Probes idle connections, so that one whose client went away without closing it (because it lost
/// power, for instance) is closed instead of holding a slot forever. A dead client is noticed after
/// `KEEPALIVE_TIME + KEEPALIVE_INTERVAL * KEEPALIVE_RETRIES` – 90 seconds.
const KEEPALIVE_TIME: Duration = Duration::from_secs(60);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
const KEEPALIVE_RETRIES: u32 = 3;

/// After a wrong secret, the next attempt from the same source waits this long, doubling with each
/// further failure up to `MAX_ATTEMPT_DELAY`.
const INITIAL_ATTEMPT_DELAY: Duration = Duration::from_millis(100);
const MAX_ATTEMPT_DELAY: Duration = Duration::from_secs(5);

/// A source that hasn't made an attempt for this long starts over with no delay.
const FORGET_SOURCE_AFTER: Duration = Duration::from_secs(5 * 60);

/// When accepting connections fails – usually because the system is out of file descriptors or memory –
/// wait this long before trying again, doubling up to `MAX_ACCEPT_RETRY_DELAY`. If accepting keeps
/// failing for `GIVE_UP_ACCEPTING_AFTER`, the listener is assumed to be broken, and the service stops so
/// that whatever supervises it can report the problem and restart it.
const INITIAL_ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(100);
const MAX_ACCEPT_RETRY_DELAY: Duration = Duration::from_secs(5);
const GIVE_UP_ACCEPTING_AFTER: Duration = Duration::from_secs(60);

/// Listens for connections until something goes wrong that the service can't recover from – it never
/// returns `Ok`.
pub fn run(configuration: AppConfiguration) -> io::Result<()> {
    configuration
        .validate()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

    let listeners = bind(configuration.port_number)?;
    log::info!(
        "Listening on port {} for connections to {}",
        configuration.port_number,
        describe_addresses(&configuration.addresses)
    );

    let configuration = Arc::new(configuration);
    let slots = Arc::new(ConnectionSlots::new(
        MAX_OPEN_CONNECTIONS,
        MAX_OPEN_CONNECTIONS_PER_SOURCE,
    ));
    let throttle = Arc::new(Throttle::new(INITIAL_ATTEMPT_DELAY, MAX_ATTEMPT_DELAY));

    // Stop as soon as either listener does, rather than carrying on with only IPv4 or only IPv6
    let (stopped_tx, stopped) = mpsc::channel();

    for listener in listeners {
        let configuration = Arc::clone(&configuration);
        let slots = Arc::clone(&slots);
        let throttle = Arc::clone(&throttle);
        let stopped_tx = stopped_tx.clone();

        thread::Builder::new()
            .name(format!("listener {}", listener.local_addr()?))
            .spawn(move || {
                // The panic hook has already logged the details
                let error = panic::catch_unwind(AssertUnwindSafe(|| {
                    accept_connections(&listener, &configuration, &slots, &throttle)
                }))
                .unwrap_or_else(|_| io::Error::other("the listener panicked"));

                let _ = stopped_tx.send(error);
            })?;
    }

    drop(stopped_tx);
    Err(stopped
        .recv()
        .unwrap_or_else(|_| io::Error::other("every listener stopped")))
}

/// Accepts connections until it fails in a way that it can't recover from, and returns why.
fn accept_connections(
    listener: &TcpListener,
    configuration: &AppConfiguration,
    slots: &Arc<ConnectionSlots>,
    throttle: &Arc<Throttle>,
) -> io::Error {
    let mut retry = AcceptRetry::new(
        INITIAL_ACCEPT_RETRY_DELAY,
        MAX_ACCEPT_RETRY_DELAY,
        GIVE_UP_ACCEPTING_AFTER,
    );

    loop {
        let stream = match listener.accept() {
            Ok((stream, _)) => {
                retry.succeeded();
                stream
            }
            // The client went away before the connection was accepted
            Err(error) if affects_only_this_connection(&error) => {
                log::debug!("Unable to accept connection: {}", error);
                continue;
            }
            Err(error) => match retry.failed(Instant::now()) {
                Some(delay) => {
                    log::warn!(
                        "Unable to accept connections, trying again in {:?} – {}",
                        delay,
                        error
                    );
                    thread::sleep(delay);
                    continue;
                }
                None => {
                    return io::Error::new(
                        error.kind(),
                        format!(
                            "unable to accept connections for {:?} – {}",
                            GIVE_UP_ACCEPTING_AFTER, error
                        ),
                    );
                }
            },
        };

        // Compare and log IPv4-mapped IPv6 addresses as IPv4, in case the system hands one over
        let peer_address = match stream.peer_addr() {
            Ok(address) => SocketAddr::new(address.ip().to_canonical(), address.port()),
            Err(error) => {
                log::warn!("Dropping connection from unknown peer – {}", error);
                continue;
            }
        };
        let peer = peer_address.to_string();

        let interface_ip = match stream.local_addr() {
            Ok(address) => address.ip().to_canonical(),
            Err(error) => {
                log::warn!(
                    peer_addr:% = peer_address.ip();
                    "Dropping connection from {} – {}",
                    peer,
                    error
                );
                continue;
            }
        };

        if !configuration.accepts_connections_on(&interface_ip) {
            log::info!(
                peer_addr:% = peer_address.ip();
                "Rejected connection from {} on {:?} – the configuration only allows connections on {}",
                peer,
                interface_ip,
                format_addresses(&configuration.addresses)
            );
            continue;
        }

        if !configuration.accepts_connections_from(&peer_address.ip()) {
            log::info!(
                peer_addr:% = peer_address.ip();
                "Rejected connection from {} – the configuration only allows connections from {}",
                peer,
                format_addresses(&configuration.allowed_sources)
            );
            continue;
        }

        let slot = match ConnectionSlots::acquire(slots, peer_address.ip()) {
            Ok(slot) => slot,
            Err(error) => {
                log::warn!(
                    peer_addr:% = peer_address.ip();
                    "Rejected connection from {} – {}",
                    peer,
                    error
                );
                continue;
            }
        };

        if let Err(error) = enable_keepalive(&stream) {
            log::warn!(
                peer_addr:% = peer_address.ip();
                "Unable to enable keepalive for {} – {}",
                peer,
                error
            );
        }

        let secret = configuration.secret.clone();
        let throttle = Arc::clone(throttle);

        // If the thread can't be created, the connection and its slot are dropped with the closure
        let spawned = thread::Builder::new().spawn(move || {
            log::info!(peer_addr:% = peer_address.ip(); "New connection: {}", peer);
            handle_stream(stream, &secret, &throttle, peer_address);
            drop(slot);
        });

        if let Err(error) = spawned {
            log::warn!(
                peer_addr:% = peer_address.ip();
                "Dropping connection from {} – unable to handle it: {}",
                peer_address,
                error
            );
        }
    }
}

/// Errors from `accept` that are about the connection being accepted, rather than the listener.
fn affects_only_this_connection(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::Interrupted
            | io::ErrorKind::WouldBlock
    )
}

/// Spaces out attempts to accept connections after a failure, and decides when to give up.
struct AcceptRetry {
    initial_delay: Duration,
    max_delay: Duration,
    give_up_after: Duration,
    /// When the current run of failures started, and how long to wait after the next one
    failing: Option<(Instant, Duration)>,
}

impl AcceptRetry {
    fn new(initial_delay: Duration, max_delay: Duration, give_up_after: Duration) -> Self {
        AcceptRetry {
            initial_delay,
            max_delay,
            give_up_after,
            failing: None,
        }
    }

    fn succeeded(&mut self) {
        self.failing = None;
    }

    /// How long to wait before trying again, or `None` if accepting has been failing for too long.
    fn failed(&mut self, now: Instant) -> Option<Duration> {
        let (since, delay) = self.failing.unwrap_or((now, self.initial_delay));

        if now.duration_since(since) >= self.give_up_after {
            return None;
        }

        self.failing = Some((since, (delay * 2).min(self.max_delay)));
        Some(delay)
    }
}

/// Listens on every interface, for IPv4 and – where the system supports it – IPv6.
///
/// This binds every interface rather than just the configured `addresses`. On Windows the service starts
/// before the network interfaces are up, so binding a specific address fails at boot and the service
/// never listens. Instead, `accept_connections` rejects connections that arrive on interfaces that aren't
/// in `addresses` after `accept`.
///
/// IPv4 and IPv6 use separate sockets rather than one dual-stack socket. On Windows a dual-stack socket
/// binds successfully even when another program is already using the port for IPv4, so the service would
/// never receive IPv4 connections, and wouldn't report the problem either.
fn bind(port: u16) -> io::Result<Vec<TcpListener>> {
    let mut listeners = vec![listen(
        Domain::IPV4,
        SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)),
    )?];

    match listen(
        Domain::IPV6,
        SocketAddr::from((Ipv6Addr::UNSPECIFIED, listeners[0].local_addr()?.port())),
    ) {
        Ok(listener) => listeners.push(listener),
        Err(error) if error.kind() == io::ErrorKind::AddrInUse => return Err(error),
        Err(error) => {
            log::info!("IPv6 is unavailable, so only listening for IPv4 connections – {error}")
        }
    }

    Ok(listeners)
}

fn listen(domain: Domain, address: SocketAddr) -> io::Result<TcpListener> {
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;

    // Leave IPv4 to its own socket – see `bind`
    if domain == Domain::IPV6 {
        socket.set_only_v6(true)?;
    }

    // Matches `TcpListener::bind`, which allows restarting while old connections are in `TIME_WAIT`. On
    // Windows the same option would let another program take over the port, so it's left off there.
    #[cfg(not(windows))]
    socket.set_reuse_address(true)?;

    socket.bind(&address.into())?;
    socket.listen(128)?;
    Ok(socket.into())
}

fn enable_keepalive(stream: &TcpStream) -> io::Result<()> {
    SockRef::from(stream).set_tcp_keepalive(
        &TcpKeepalive::new()
            .with_time(KEEPALIVE_TIME)
            .with_interval(KEEPALIVE_INTERVAL)
            .with_retries(KEEPALIVE_RETRIES),
    )
}

/// Counts open connections, in total and by source address.
struct ConnectionSlots {
    limit: usize,
    per_source_limit: usize,
    open: Mutex<OpenConnections>,
}

#[derive(Default)]
struct OpenConnections {
    total: usize,
    by_source: HashMap<IpAddr, usize>,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
enum SlotError {
    #[error("too many open connections")]
    TooManyConnections,
    #[error("too many open connections from this source")]
    TooManyConnectionsFromSource,
}

impl ConnectionSlots {
    fn new(limit: usize, per_source_limit: usize) -> ConnectionSlots {
        ConnectionSlots {
            limit,
            per_source_limit,
            open: Mutex::new(OpenConnections::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, OpenConnections> {
        self.open.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Takes a slot for a connection from `source`, which is released when the returned value is dropped.
    fn acquire(slots: &Arc<ConnectionSlots>, source: IpAddr) -> Result<ConnectionSlot, SlotError> {
        let mut open = slots.lock();

        if open.by_source.get(&source).copied().unwrap_or(0) >= slots.per_source_limit {
            return Err(SlotError::TooManyConnectionsFromSource);
        }

        if open.total >= slots.limit {
            return Err(SlotError::TooManyConnections);
        }

        open.total += 1;
        *open.by_source.entry(source).or_insert(0) += 1;

        Ok(ConnectionSlot {
            slots: Arc::clone(slots),
            source,
        })
    }
}

struct ConnectionSlot {
    slots: Arc<ConnectionSlots>,
    source: IpAddr,
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        let mut open = self.slots.lock();
        open.total -= 1;

        if let Some(count) = open.by_source.get_mut(&self.source) {
            *count -= 1;
            if *count == 0 {
                open.by_source.remove(&self.source);
            }
        }
    }
}

fn handle_stream(stream: TcpStream, secret: &str, throttle: &Throttle, peer: SocketAddr) {
    match wait_for_secret(BufReader::new(stream), secret, throttle, peer.ip()) {
        Ok(true) => {
            log::info!(peer_addr:% = peer.ip(); "Shutting down - source: {}", peer);

            if let Err(error) = shutdown() {
                log::error!("Failed to shut down: {}", error);
            }
        }
        Ok(false) => log::info!(peer_addr:% = peer.ip(); "Connection closed by {}", peer),
        Err(error) => {
            log::warn!(peer_addr:% = peer.ip(); "Terminating connection with {}: {}", peer, error)
        }
    }
}

/// Reads newline-delimited messages until one matches `secret` (returning `true`) or the client closes
/// the connection (returning `false`). The last message doesn't need a trailing newline.
///
/// Each message waits its turn in `throttle` before being checked, so wrong guesses slow down every
/// connection from the same source.
fn wait_for_secret<R: BufRead>(
    mut reader: R,
    secret: &str,
    throttle: &Throttle,
    source: IpAddr,
) -> io::Result<bool> {
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

        thread::sleep(throttle.reserve_attempt(source, Instant::now()));

        if let Ok(input) = std::str::from_utf8(&message)
            && secrets_match(input.trim().as_bytes(), secret.as_bytes())
        {
            return Ok(true);
        }

        throttle.record_failure(source);
        log::debug!(peer_addr:% = source; "Received a message that didn't match the secret");
    }
}

struct SourceState {
    failures: u32,
    next_attempt: Instant,
}

/// Spaces out secret attempts from each source address, with the gap growing exponentially as wrong
/// guesses accumulate. Tracking by source rather than by connection means opening more connections
/// doesn't buy more guesses.
struct Throttle {
    initial_delay: Duration,
    max_delay: Duration,
    sources: Mutex<HashMap<IpAddr, SourceState>>,
}

impl Throttle {
    fn new(initial_delay: Duration, max_delay: Duration) -> Throttle {
        Throttle {
            initial_delay,
            max_delay,
            sources: Mutex::new(HashMap::new()),
        }
    }

    /// Reserves the next attempt slot for `source`, returning how long to wait before using it.
    fn reserve_attempt(&self, source: IpAddr, now: Instant) -> Duration {
        let mut sources = self
            .sources
            .lock()
            .unwrap_or_else(|error| error.into_inner());

        sources.retain(|_, state| now < state.next_attempt + FORGET_SOURCE_AFTER);

        let state = sources.entry(source).or_insert(SourceState {
            failures: 0,
            next_attempt: now,
        });

        // Space the following attempt as though this one will fail – if it succeeds, the machine shuts down
        let start = state.next_attempt.max(now);
        state.next_attempt = start + self.delay_after(state.failures.saturating_add(1));
        start - now
    }

    fn record_failure(&self, source: IpAddr) {
        let mut sources = self
            .sources
            .lock()
            .unwrap_or_else(|error| error.into_inner());

        if let Some(state) = sources.get_mut(&source) {
            state.failures = state.failures.saturating_add(1);
        }
    }

    /// The gap between attempts after `failures` consecutive wrong guesses.
    fn delay_after(&self, failures: u32) -> Duration {
        if failures == 0 {
            return Duration::ZERO;
        }

        let multiplier = 1u32.checked_shl(failures - 1).unwrap_or(u32::MAX);
        self.initial_delay
            .saturating_mul(multiplier)
            .min(self.max_delay)
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

    fn source() -> IpAddr {
        "10.0.1.50".parse().unwrap()
    }

    fn unthrottled() -> Throttle {
        Throttle::new(Duration::ZERO, Duration::ZERO)
    }

    fn wait_for(input: &[u8]) -> io::Result<bool> {
        wait_for_secret(
            Cursor::new(input.to_vec()),
            SECRET,
            &unthrottled(),
            source(),
        )
    }

    #[test]
    fn test_accept_retry_backs_off_then_gives_up() {
        let mut retry = AcceptRetry::new(
            Duration::from_millis(100),
            Duration::from_millis(300),
            Duration::from_secs(1),
        );
        let start = Instant::now();

        assert_eq!(retry.failed(start), Some(Duration::from_millis(100)));
        assert_eq!(retry.failed(start), Some(Duration::from_millis(200)));
        assert_eq!(retry.failed(start), Some(Duration::from_millis(300)));
        assert_eq!(retry.failed(start), Some(Duration::from_millis(300)));
        assert_eq!(
            retry.failed(start + Duration::from_millis(999)),
            Some(Duration::from_millis(300))
        );
        assert_eq!(retry.failed(start + Duration::from_secs(1)), None);
    }

    #[test]
    fn test_accept_retry_starts_over_after_a_success() {
        let mut retry = AcceptRetry::new(
            Duration::from_millis(100),
            Duration::from_millis(300),
            Duration::from_secs(1),
        );
        let start = Instant::now();

        retry.failed(start);
        retry.failed(start);
        retry.succeeded();

        // The give-up time is measured from the first failure after the success
        let later = start + Duration::from_secs(5);
        assert_eq!(retry.failed(later), Some(Duration::from_millis(100)));
        assert_eq!(
            retry.failed(later + Duration::from_millis(999)),
            Some(Duration::from_millis(200))
        );
    }

    #[test]
    fn test_client_errors_during_accept_only_affect_that_connection() {
        for kind in [
            io::ErrorKind::ConnectionAborted,
            io::ErrorKind::ConnectionReset,
            io::ErrorKind::Interrupted,
        ] {
            assert!(affects_only_this_connection(&io::Error::from(kind)));
        }

        // For instance, EMFILE – out of file descriptors
        assert!(!affects_only_this_connection(&io::Error::other(
            "too many open files"
        )));
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
        assert!(
            wait_for_secret(
                Cursor::new(input.into_bytes()),
                &secret,
                &unthrottled(),
                source()
            )
            .unwrap()
        );
    }

    #[test]
    fn test_empty_secret_never_matches() {
        assert!(
            !wait_for_secret(Cursor::new(b"\n".to_vec()), "", &unthrottled(), source()).unwrap()
        );
        assert!(!secrets_match(b"", b""));
    }

    #[test]
    fn test_an_invalid_secret_is_rejected_before_listening() {
        let configuration = AppConfiguration {
            secret: String::new(),
            ..AppConfiguration::default()
        };

        let error = run(configuration).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    fn slots() -> Arc<ConnectionSlots> {
        Arc::new(ConnectionSlots::new(3, 2))
    }

    #[test]
    fn test_connections_from_one_source_are_capped() {
        let slots = slots();

        let _first = ConnectionSlots::acquire(&slots, source()).unwrap();
        let second = ConnectionSlots::acquire(&slots, source()).unwrap();
        assert_eq!(
            ConnectionSlots::acquire(&slots, source()).err(),
            Some(SlotError::TooManyConnectionsFromSource)
        );

        // Closing a connection frees its slot
        drop(second);
        assert!(ConnectionSlots::acquire(&slots, source()).is_ok());
    }

    #[test]
    fn test_connections_from_every_source_are_capped() {
        let slots = slots();

        let _held = [
            ConnectionSlots::acquire(&slots, "10.0.1.50".parse().unwrap()).unwrap(),
            ConnectionSlots::acquire(&slots, "10.0.1.51".parse().unwrap()).unwrap(),
            ConnectionSlots::acquire(&slots, "10.0.1.52".parse().unwrap()).unwrap(),
        ];

        assert_eq!(
            ConnectionSlots::acquire(&slots, "10.0.1.53".parse().unwrap()).err(),
            Some(SlotError::TooManyConnections)
        );
    }

    #[test]
    fn test_released_slots_are_forgotten() {
        let slots = slots();
        drop(ConnectionSlots::acquire(&slots, source()).unwrap());

        let open = slots.lock();
        assert_eq!(open.total, 0);
        assert!(open.by_source.is_empty());
    }

    /// Connects to the listener from `bind` for `host`'s address family, returning the addresses as
    /// `accept_connections` sees them.
    fn connect(listeners: &[TcpListener], host: IpAddr) -> (IpAddr, IpAddr) {
        let listener = listeners
            .iter()
            .find(|listener| listener.local_addr().unwrap().is_ipv6() == host.is_ipv6())
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let _client = TcpStream::connect((host, port)).unwrap();
        let (stream, peer) = listener.accept().unwrap();

        (
            stream.local_addr().unwrap().ip().to_canonical(),
            peer.ip().to_canonical(),
        )
    }

    #[test]
    fn test_listener_accepts_ipv4_connections() {
        let listeners = bind(0).unwrap();
        let localhost = IpAddr::V4(Ipv4Addr::LOCALHOST);

        assert_eq!(connect(&listeners, localhost), (localhost, localhost));
    }

    #[test]
    fn test_listener_accepts_ipv6_connections() {
        let listeners = bind(0).unwrap();
        let localhost = IpAddr::V6(Ipv6Addr::LOCALHOST);

        assert_eq!(connect(&listeners, localhost), (localhost, localhost));
    }

    #[test]
    fn test_ipv4_and_ipv6_use_the_same_port() {
        let listeners = bind(0).unwrap();
        let ports: Vec<u16> = listeners
            .iter()
            .map(|listener| listener.local_addr().unwrap().port())
            .collect();

        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0], ports[1]);
    }

    #[test]
    fn test_binding_fails_when_the_ipv4_port_is_in_use() {
        let other = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).unwrap();
        let port = other.local_addr().unwrap().port();

        let error = bind(port).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
    }

    #[test]
    fn test_accepted_connections_are_kept_alive() {
        let listeners = bind(0).unwrap();
        let listener = &listeners[0];
        let port = listener.local_addr().unwrap().port();
        let _client = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        let (stream, _peer) = listener.accept().unwrap();

        enable_keepalive(&stream).unwrap();

        let socket = SockRef::from(&stream);
        assert!(socket.keepalive().unwrap());
        // Windows can't report the keepalive timings
        #[cfg(not(windows))]
        assert_eq!(socket.tcp_keepalive_time().unwrap(), KEEPALIVE_TIME);
    }

    fn throttle() -> Throttle {
        Throttle::new(Duration::from_millis(100), Duration::from_secs(5))
    }

    /// Simulates `count` wrong guesses from `source`, each made as soon as it's allowed, and returns the
    /// time the next attempt would be allowed.
    fn fail(throttle: &Throttle, source: IpAddr, mut now: Instant, count: usize) -> Instant {
        for _ in 0..count {
            now += throttle.reserve_attempt(source, now);
            throttle.record_failure(source);
        }

        now + throttle.reserve_attempt(source, now)
    }

    #[test]
    fn test_first_attempt_is_not_delayed() {
        let now = Instant::now();
        assert_eq!(throttle().reserve_attempt(source(), now), Duration::ZERO);
    }

    #[test]
    fn test_delay_doubles_after_each_failure() {
        let throttle = throttle();
        let start = Instant::now();

        // 0 + 100 + 200 + 400ms
        assert_eq!(
            fail(&throttle, source(), start, 3) - start,
            Duration::from_millis(700)
        );
    }

    #[test]
    fn test_delay_is_capped() {
        let throttle = throttle();
        assert_eq!(throttle.delay_after(7), Duration::from_millis(5000));
        assert_eq!(throttle.delay_after(u32::MAX), Duration::from_secs(5));
    }

    #[test]
    fn test_parallel_attempts_from_one_source_are_serialized() {
        let throttle = throttle();
        let now = Instant::now();

        fail(&throttle, source(), now, 20);

        // Two connections asking at the same moment get consecutive slots rather than the same one
        let first = throttle.reserve_attempt(source(), now);
        let second = throttle.reserve_attempt(source(), now);
        assert_eq!(second - first, Duration::from_secs(5));
    }

    #[test]
    fn test_sources_are_throttled_independently() {
        let throttle = throttle();
        let now = Instant::now();

        fail(&throttle, source(), now, 20);

        assert_eq!(
            throttle.reserve_attempt("10.0.1.51".parse().unwrap(), now),
            Duration::ZERO
        );
    }

    #[test]
    fn test_idle_sources_are_forgotten() {
        let throttle = throttle();
        let now = Instant::now();

        let next_attempt = fail(&throttle, source(), now, 20);
        let later = next_attempt + MAX_ATTEMPT_DELAY + FORGET_SOURCE_AFTER;

        assert_eq!(throttle.reserve_attempt(source(), later), Duration::ZERO);
    }
}
