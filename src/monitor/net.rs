//! Network monitor.
//! Ping remote host using ICMP PING packets and report the round-trip time.
//! Use ICMP PING sockets <https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/commit/?id=c319b4d76b9e583a5d88d6bf190e079c4e43213d>
use crate::{
    backend::endpoints::NETWORK_MONITOR_HOST,
    config::Config,
    network::WPA_SUPPLICANT_INTERFACE_BIN,
    pid::{derivative::LowPassFilter, InstantTimer, Timer},
    process::Command,
    utils::spawn_named_thread,
};
use eyre::{bail, Result, WrapErr};
use futures::{channel::oneshot, prelude::*, ready};
use pnet::{
    datalink,
    packet::{
        icmp::{
            echo_reply::EchoReplyPacket as EchoReplyPacketV4,
            echo_request::MutableEchoRequestPacket as MutableEchoRequestPacketV4,
            IcmpCode as IcmpCodeV4, IcmpTypes as Icmpv4Types,
        },
        icmpv6::{
            echo_reply::EchoReplyPacket as EchoReplyPacketV6,
            echo_request::MutableEchoRequestPacket as MutableEchoRequestPacketV6,
            Icmpv6Code as IcmpCodeV6, Icmpv6Types,
        },
        Packet,
    },
};
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::{
    io,
    io::Read,
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    pin::Pin,
    str,
    sync::Arc,
    task::{Context, Poll},
    thread,
    time::{Duration, Instant},
};
use tokio::{
    runtime,
    sync::{broadcast, Mutex},
    task,
};
use tokio_stream::wrappers::{errors::BroadcastStreamRecvError, BroadcastStream};

const NO_INTERNET_THRESHOLD: Duration = Duration::from_secs(5);
const NO_WLAN_THRESHOLD: i64 = -100;
const SLOW_WLAN_THRESHOLD: i64 = -70;
const DELAY_BETWEEN_REQUESTS: Duration = Duration::from_secs(1);
const DELAY_BETWEEN_RESOLVES: Duration = Duration::from_secs(1);
const SSID_POLLING_DIVIDER: u16 = 5;
const REPORT_CAPACITY: usize = 10;
const LAG_FILTER_RC: f64 = 2.0;
const RSSI_FILTER_RC: f64 = 1.5;

/// Network monitor trait.
pub trait Monitor: Stream<Item = Report> + Send + Unpin {
    /// Returns a new handler to the shared network monitor interface.
    fn clone(&self) -> Box<dyn Monitor>;

    /// Returns the latest network monitor report.
    fn last_report(&mut self) -> Result<Option<&Report>>;
}

/// Network monitor for the Orb hardware.
pub struct Jetson {
    report_tx: broadcast::Sender<Report>,
    report_rx: BroadcastStream<Report>,
    last_report: Option<Report>,
}

/// Network monitor which does nothing.
pub struct Fake;

/// Periodic network monitor report.
#[derive(Clone, Debug)]
pub struct Report {
    /// Time lag to the backend.
    pub lag: f64,
    /// Threshold for configuring what's the maximum ping delay for the internet connection, to warn for delays in the
    /// signup process.
    pub slow_internet_ping_threshold: Duration,
    /// WiFi Signal level in dBm.
    pub rssi: i64,
    /// WiFi SSID name.
    pub ssid: String,
    /// Mac address of the WiFi interface.
    pub mac_address: String,
}

impl Monitor for Jetson {
    fn clone(&self) -> Box<dyn Monitor> {
        Box::new(Self {
            report_tx: self.report_tx.clone(),
            report_rx: BroadcastStream::new(self.report_tx.subscribe()),
            last_report: None,
        })
    }

    /// Returns the latest network monitor report.
    fn last_report(&mut self) -> Result<Option<&Report>> {
        while let Some(report) = self.next().now_or_never() {
            if let Some(report) = report {
                self.last_report = Some(report);
            } else {
                bail!("broadcast channel of network monitor exited");
            }
        }
        Ok(self.last_report.as_ref())
    }
}

impl Stream for Fake {
    type Item = Report;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Pending
    }
}

impl Monitor for Fake {
    fn clone(&self) -> Box<dyn Monitor> {
        Box::new(Self)
    }

    fn last_report(&mut self) -> Result<Option<&Report>> {
        Ok(None)
    }
}

/// Network monitor external trigger.
pub struct Trigger(oneshot::Sender<()>);

impl Trigger {
    /// Performs the delayed start of the associated network monitor.
    pub fn fire(self) {
        self.0.send(()).expect("network monitor to be listening");
    }
}

impl Jetson {
    /// Spawns a new network monitor.
    pub fn spawn(config: Arc<Mutex<Config>>) -> Result<Self> {
        let (monitor, trigger) = Self::spawn_with_trigger(config)?;
        trigger.fire();
        Ok(monitor)
    }

    /// Spawns a new network monitor after external `trigger`.
    pub fn spawn_with_trigger(config: Arc<Mutex<Config>>) -> Result<(Self, Trigger)> {
        let (trigger_tx, trigger_rx) = oneshot::channel();
        let (report_tx, report_rx) = broadcast::channel(REPORT_CAPACITY);
        let report_tx2 = report_tx.clone();
        task::spawn(async move {
            if trigger_rx.await.is_ok() {
                spawn_named_thread("monitor-net", move || {
                    main_loop(&report_tx2, &config);
                    tracing::warn!("Network monitor main loop exited");
                });
            }
        });

        Ok((
            Self { report_tx, report_rx: BroadcastStream::new(report_rx), last_report: None },
            Trigger(trigger_tx),
        ))
    }
}

impl Fake {
    /// Creates a new fake network monitor with an external trigger.
    #[must_use]
    pub fn spawn_with_trigger() -> (Self, Trigger) {
        let (trigger_tx, trigger_rx) = oneshot::channel();
        task::spawn(async move {
            let _ = trigger_rx.await;
        });
        (Self, Trigger(trigger_tx))
    }
}

impl Stream for Jetson {
    type Item = Report;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            break Poll::Ready(match ready!(self.report_rx.poll_next_unpin(cx)) {
                Some(Ok(item)) => Some(item),
                Some(Err(BroadcastStreamRecvError::Lagged(_))) => continue,
                None => None,
            });
        }
    }
}

impl Report {
    /// Returns `true` if the internet connection considered slow.
    #[must_use]
    pub fn is_slow_internet(&self) -> bool {
        self.lag >= self.slow_internet_ping_threshold.as_secs_f64()
    }

    /// Returns `true` if the internet connection is absent.
    #[must_use]
    pub fn is_no_internet(&self) -> bool {
        self.lag >= NO_INTERNET_THRESHOLD.as_secs_f64()
    }

    /// Returns `true` if the wlan connection considered slow.
    #[must_use]
    pub fn is_slow_wlan(&self) -> bool {
        self.rssi < SLOW_WLAN_THRESHOLD
    }

    /// Returns `true` if the wlan connection is absent.
    #[must_use]
    pub fn is_no_wlan(&self) -> bool {
        self.rssi < NO_WLAN_THRESHOLD
    }
}

/// Makes a single ping-pong with the backend server.
pub fn ping(remote: &str) -> io::Result<f64> {
    let mut socket = PingSocket::new(remote)?;
    socket.ping()
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn main_loop(report_tx: &broadcast::Sender<Report>, config: &Arc<Mutex<Config>>) {
    let mut lag_timer = InstantTimer::default();
    let mut lag_filter = LowPassFilter::default();
    let mut rssi_timer = InstantTimer::default();
    let mut rssi_filter = LowPassFilter::default();
    let mut ssid = String::new();
    let mut sequence_number: u16 = 0;

    let mut socket = loop {
        if let Ok(socket) = PingSocket::new(&NETWORK_MONITOR_HOST) {
            break socket;
        }
        thread::sleep(DELAY_BETWEEN_RESOLVES);
    };

    // Create Tokio runtime once and reuse it.
    let rt = runtime::Builder::new_current_thread().enable_all().build().unwrap();

    loop {
        let lag = match socket.ping() {
            Ok(lag) => lag,
            Err(err) => {
                tracing::debug!("Failed to ping remote host {0}: {err:?}", &*NETWORK_MONITOR_HOST);
                NO_INTERNET_THRESHOLD.as_secs_f64() * 1.25
            }
        };
        let dt = lag_timer.get_dt().unwrap_or(0.0);
        let lag = lag_filter.add(lag, dt, LAG_FILTER_RC);
        let rssi = poll_rssi().map_or_else(
            |err| {
                NO_WLAN_THRESHOLD as f64 * 1.25
            },
            |rssi| rssi as f64,
        );
        let dt = rssi_timer.get_dt().unwrap_or(0.0);
        let rssi = rssi_filter.add(rssi, dt, RSSI_FILTER_RC) as i64;

        if sequence_number % SSID_POLLING_DIVIDER == 0 {
            ssid = poll_ssid().unwrap_or_else(|err| {
                String::new()
            });
        }

        // Get what we want from the config and drop the mutex fast.
        let slow_internet_ping_threshold = rt.block_on(config.lock()).slow_internet_ping_threshold;
        if report_tx
            .send(Report {
                lag,
                slow_internet_ping_threshold,
                rssi,
                ssid: ssid.clone(),
                mac_address: mac_address().unwrap_or_default(),
            })
            .is_err()
        {
            break;
        }
        thread::sleep(DELAY_BETWEEN_REQUESTS);
        sequence_number = sequence_number.wrapping_add(1);
    }
}

fn poll_rssi() -> Result<i64> {
    let output = Command::new(WPA_SUPPLICANT_INTERFACE_BIN)
        .arg("signal")
        .output()
        .wrap_err("running `wpa-supplicant-interface`")?;
    if output.status.success() {
        Ok(str::from_utf8(&output.stdout)
            .wrap_err("parsing `wpa-supplicant-interface` output")?
            .parse()?)
    } else {
        bail!("`wpa-supplicant-interface` terminated unsuccessfully");
    }
}

fn poll_ssid() -> Result<String> {
    let output = Command::new(WPA_SUPPLICANT_INTERFACE_BIN)
        .arg("ssid")
        .output()
        .wrap_err("running `wpa-supplicant-interface`")?;
    if output.status.success() {
        Ok(str::from_utf8(&output.stdout)
            .wrap_err("parsing `wpa-supplicant-interface` output")?
            .trim()
            .to_owned())
    } else {
        bail!("`wpa-supplicant-interface` terminated unsuccessfully");
    }
}

struct PingSocket {
    inner: Socket,
    addr: SockAddr,
    sequence_number: u16,
}

impl PingSocket {
    fn new(remote: &str) -> io::Result<Self> {
        let addr = resolve_addr(remote)?;
        let inner = match addr.ip() {
            IpAddr::V4(_) => Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::ICMPV4)),
            IpAddr::V6(_) => Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::ICMPV6)),
        }?;
        inner.set_send_buffer_size(324 /* value copied from ping */)?;
        inner.set_recv_buffer_size(65536 /* value copied from ping */)?;
        inner.set_read_timeout(Some(NO_INTERNET_THRESHOLD))?;
        inner.set_write_timeout(Some(NO_INTERNET_THRESHOLD))?;
        //inner.set_cloexec(true)?;
        //TODO set inner.SO_TIMESTAMP
        Ok(Self { inner, addr: SockAddr::from(addr), sequence_number: 0 })
    }

    /// Send ICMP echo request and wait for the reply.
    ///
    /// # Returns
    /// the round-trip time in seconds.
    /// `Err(err)` if there was an IO error.
    fn ping(&mut self) -> io::Result<f64> {
        let begin = Instant::now();

        self.sequence_number = self.sequence_number.wrapping_add(1);

        self.make_request()?;
        self.read_reply()?;
        Ok(begin.elapsed().as_secs_f64())
    }

    fn make_request(&self) -> io::Result<()> {
        let mut buf = [0u8; 64];
        if self.inner.local_addr()?.is_ipv4() {
            let mut echo_request = MutableEchoRequestPacketV4::new(&mut buf).unwrap();
            echo_request.set_icmp_type(Icmpv4Types::EchoRequest);
            echo_request.set_icmp_code(IcmpCodeV4::new(0));
            echo_request.set_sequence_number(self.sequence_number);
            self.inner.send_to(echo_request.packet(), &self.addr)?;
        } else {
            let mut echo_request = MutableEchoRequestPacketV6::new(&mut buf).unwrap();
            echo_request.set_icmpv6_type(Icmpv6Types::EchoRequest);
            echo_request.set_icmpv6_code(IcmpCodeV6::new(0));
            echo_request.set_sequence_number(self.sequence_number);
            self.inner.send_to(echo_request.packet(), &self.addr)?;
        }
        Ok(())
    }

    // return sequence number or None
    fn parse_echo_response(&self, buf: &[u8]) -> Option<u16> {
        if self.inner.local_addr().ok()?.is_ipv4() {
            let echo_response = EchoReplyPacketV4::new(buf)?;
            Some(echo_response.get_sequence_number())
        } else {
            let echo_response = EchoReplyPacketV6::new(buf)?;
            Some(echo_response.get_sequence_number())
        }
    }

    fn read_reply(&mut self) -> io::Result<()> {
        let mut buf = [0u8; 64];

        let begin = Instant::now();
        loop {
            match self.inner.read(&mut buf) {
                Ok(len) => {
                    if self.parse_echo_response(&buf[0..len]) == Some(self.sequence_number) {
                        return Ok(());
                    }
                    if begin.elapsed() > NO_INTERNET_THRESHOLD {
                        return Err(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            "No ICMP reply received in time",
                        ));
                    }
                }
                Err(err) => {
                    tracing::warn!("Error reading ICMP reply: {:?}", err);
                    return Err(err);
                }
            }
        }
    }
}

fn resolve_addr(remote: &str) -> io::Result<SocketAddr> {
    match (remote, 0).to_socket_addrs() {
        Ok(mut addrs) => {
            if let Some(addr) = addrs.next() {
                tracing::trace!("{remote} resolved to {addr}");
                return Ok(addr);
            }
            tracing::error!("{remote} resolved to 0 addresses");
            Err(io::Error::other(format!("{remote} resolved to 0 addresses")))
        }
        Err(err) => {
            tracing::error!("Couldn't resolve {remote}: {err:?}");
            Err(err)
        }
    }
}

fn mac_address() -> Option<String> {
    datalink::interfaces()
        .iter()
        .find(|e| e.name == "wlan0")
        .and_then(|interface| interface.mac)
        .map(|mac| mac.to_string())
}

// Tests are disabled because they require a network, but nix sandboxing doesn't allow it.
// If you find a way to run these tests, please enable them.
//
#[cfg(test)]
mod tests {
    use crate::{backend::endpoints::NETWORK_MONITOR_HOST, monitor::net::ping};

    /// IPv4 has no *official* blackhole address, use a IP range reserved for documentation (TEST-NET-3)
    /// https://datatracker.ietf.org/doc/html/rfc5735#section-4
    const BLACKHOLE_LEGACY_IP: &str = "203.0.113.1";
    /// https://datatracker.ietf.org/doc/html/rfc6666
    const BLACKHOLE_IP: &str = "0100::1";

    #[test]
    #[ignore]
    fn test_ping_loopback() {
        assert!(ping("::1").is_ok());
    }

    #[test]
    #[ignore]
    fn test_ping_loopback_legacy_ip() {
        assert!(ping("127.0.0.1").is_ok());
    }

    #[test]
    #[ignore]
    fn test_ping_success() {
        assert!(ping(&NETWORK_MONITOR_HOST).is_ok());
    }

    #[test]
    #[ignore]
    fn test_ping_blackhole() {
        let ret = ping(BLACKHOLE_IP);
        assert!(ret.is_err());
        assert_eq!(ret.err().unwrap().kind(), std::io::ErrorKind::WouldBlock);
    }

    #[test]
    #[ignore]
    fn test_ping_blackhole_legacy_ip() {
        let ret = ping(BLACKHOLE_LEGACY_IP);
        assert!(ret.is_err());
        assert_eq!(ret.err().unwrap().kind(), std::io::ErrorKind::WouldBlock);
    }
}
