//! Network monitor.

use crate::{
    backend::endpoints::NETWORK_MONITOR_HOST,
    config::Config,
    pid::{derivative::LowPassFilter, InstantTimer, Timer},
    utils::spawn_named_thread,
};
use eyre::{bail, Result, WrapErr};
use futures::{channel::oneshot, prelude::*, ready};
use pnet::{
    packet::{
        icmp::{
            echo_reply::EchoReplyPacket as EchoReplyPacketV4,
            echo_request::MutableEchoRequestPacket as MutableEchoRequestPacketV4, IcmpPacket,
            IcmpType, IcmpTypes,
        },
        icmpv6::{
            echo_reply::EchoReplyPacket as EchoReplyPacketV6,
            echo_request::MutableEchoRequestPacket as MutableEchoRequestPacketV6, Icmpv6Packet,
            Icmpv6Type, Icmpv6Types,
        },
        ip::IpNextHeaderProtocols,
        Packet,
    },
    transport::{
        icmp_packet_iter, icmpv6_packet_iter, transport_channel, TransportChannelType,
        TransportProtocol, TransportReceiver, TransportSender,
    },
    util,
};
use rand::random;
use std::{
    io,
    net::{IpAddr, ToSocketAddrs},
    pin::Pin,
    process::Command,
    str,
    sync::{mpsc, Arc},
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
}

struct Reply {
    addr: IpAddr,
    identifier: u16,
    sequence_number: u16,
    timestamp: Instant,
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
        let (listen_tx, listen_rx) = mpsc::channel();
        let (mut icmpv4_transport_tx, mut icmpv4_transport_rx) = icmpv4_transport()?;
        let (mut icmpv6_transport_tx, mut icmpv6_transport_rx) = icmpv6_transport()?;
        let report_tx2 = report_tx.clone();
        let listen_tx2 = listen_tx.clone();
        task::spawn(async move {
            if trigger_rx.await.is_ok() {
                spawn_named_thread("monitor-net", move || {
                    main_loop(
                        &mut icmpv4_transport_tx,
                        &mut icmpv6_transport_tx,
                        &report_tx2,
                        &listen_rx,
                        &config,
                    );
                    tracing::warn!("Network monitor main loop exited");
                });
                spawn_named_thread("monitor-net-ipv4", move || {
                    listen_v4_loop(&mut icmpv4_transport_rx, &listen_tx);
                    tracing::warn!("Network monitor listen loop exited");
                });
                spawn_named_thread("monitor-net-ipv6", move || {
                    listen_v6_loop(&mut icmpv6_transport_rx, &listen_tx2);
                    tracing::warn!("Network monitor listen loop exited");
                });
            }
        });

        Ok((
            Self { report_tx, report_rx: BroadcastStream::new(report_rx), last_report: None },
            Trigger(trigger_tx),
        ))
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

impl Reply {
    fn try_from_raw_v4((packet, addr): (IcmpPacket, IpAddr)) -> Option<Self> {
        let icmp_type = packet.get_icmp_type();
        if icmp_type != IcmpType::new(0) {
            tracing::trace!(
                "ICMP packet with unexpected type {icmp_type:?} received from {addr:?}"
            );
            return None;
        }
        if let Some(echo_reply) = EchoReplyPacketV4::new(packet.packet()) {
            return Some(Reply {
                addr,
                identifier: echo_reply.get_identifier(),
                sequence_number: echo_reply.get_sequence_number(),
                timestamp: Instant::now(),
            });
        }
        None
    }

    fn try_from_raw_v6((packet, addr): (Icmpv6Packet, IpAddr)) -> Option<Self> {
        let icmp_type = packet.get_icmpv6_type();
        // looking for type 129 "Echo Reply"
        if icmp_type != Icmpv6Type::new(129) {
            tracing::trace!(
                "ICMP packet with unexpected type {icmp_type:?} received from {addr:?}"
            );
            return None;
        }
        if let Some(echo_reply) = EchoReplyPacketV6::new(packet.packet()) {
            return Some(Reply {
                addr,
                identifier: echo_reply.get_identifier(),
                sequence_number: echo_reply.get_sequence_number(),
                timestamp: Instant::now(),
            });
        }
        None
    }
}

/// Makes a single ping to the backend server.
#[must_use]
pub fn ping() -> Option<Instant> {
    let addr = resolve_addr()?;
    match addr {
        IpAddr::V4(_) => ping_ipv4(addr),
        IpAddr::V6(_) => ping_ipv6(addr),
    }
}

fn ping_ipv4(addr: IpAddr) -> Option<Instant> {
    let (mut transport_tx, mut transport_rx) = match icmpv4_transport() {
        Ok(transport) => transport,
        Err(err) => {
            tracing::error!("Couldn't initialize ICMP transport: {err:?}");
            return None;
        }
    };
    let mut iter = icmp_packet_iter(&mut transport_rx);
    let mut buf = vec![0; 16];
    let sequence_number = 0;
    let identifier = random();
    match transport_tx.send_to(echo_request_v4(&mut buf, sequence_number, identifier), addr) {
        Ok(_) => {
            let start = Instant::now();
            let mut timeout = NO_INTERNET_THRESHOLD;
            while !timeout.is_zero() {
                match iter.next_with_timeout(timeout) {
                    Ok(packet) => {
                        if let Some(reply) = Reply::try_from_raw_v4(packet?) {
                            if reply.sequence_number == sequence_number
                                && reply.identifier == identifier
                                && reply.addr == addr
                            {
                                return Some(reply.timestamp);
                            }
                        }
                    }
                    Err(err) => tracing::debug!("Couldn't receive ICMP packet: {err:?}"),
                }
                timeout = timeout.saturating_sub(start.elapsed());
            }
        }
        Err(err) => {
            tracing::debug!("Couldn't send ICMP packet: {err:?}");
        }
    }
    None
}

fn ping_ipv6(addr: IpAddr) -> Option<Instant> {
    let (mut transport_tx, mut transport_rx) = match icmpv6_transport() {
        Ok(transport) => transport,
        Err(err) => {
            tracing::error!("Couldn't initialize ICMP transport: {err:?}");
            return None;
        }
    };
    let mut iter = icmpv6_packet_iter(&mut transport_rx);
    let mut buf = vec![0; 16];
    let sequence_number = 0;
    let identifier = random();
    match transport_tx.send_to(echo_request_v6(&mut buf, sequence_number, identifier), addr) {
        Ok(_) => {
            let start = Instant::now();
            let mut timeout = NO_INTERNET_THRESHOLD;
            while !timeout.is_zero() {
                match iter.next_with_timeout(timeout) {
                    Ok(packet) => {
                        if let Some(reply) = Reply::try_from_raw_v6(packet?) {
                            if reply.sequence_number == sequence_number
                                && reply.identifier == identifier
                                && reply.addr == addr
                            {
                                return Some(reply.timestamp);
                            }
                        }
                    }
                    Err(err) => tracing::debug!("Couldn't receive ICMP packet: {err:?}"),
                }
                timeout = timeout.saturating_sub(start.elapsed());
            }
        }
        Err(err) => {
            tracing::debug!("Couldn't send ICMP packet: {err:?}");
        }
    }
    None
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn main_loop(
    icmpv4_transport_tx: &mut TransportSender,
    icmpv6_transport_tx: &mut TransportSender,
    report_tx: &broadcast::Sender<Report>,
    listen_rx: &mpsc::Receiver<Reply>,
    config: &Arc<Mutex<Config>>,
) {
    let mut lag_timer = InstantTimer::default();
    let mut lag_filter = LowPassFilter::default();
    let mut rssi_timer = InstantTimer::default();
    let mut rssi_filter = LowPassFilter::default();
    let mut ssid = String::new();
    let mut sequence_number = 0;
    let identifier = random();
    let addr = loop {
        if let Some(addr) = resolve_addr() {
            break addr;
        }
        thread::sleep(DELAY_BETWEEN_RESOLVES);
    };
    tracing::info!("{} resolved to {addr:?}", *NETWORK_MONITOR_HOST);

    // Create Tokio runtime once and reuse it.
    let rt = runtime::Builder::new_current_thread().enable_all().build().unwrap();

    'outer: loop {
        let request_timestamp = Instant::now();
        let mut buf = vec![0; 16];
        let result = match addr {
            IpAddr::V4(_) => icmpv4_transport_tx
                .send_to(echo_request_v4(&mut buf, sequence_number, identifier), addr),
            IpAddr::V6(_) => icmpv6_transport_tx
                .send_to(echo_request_v6(&mut buf, sequence_number, identifier), addr),
        };
        let reply_timestamp = match result {
            Ok(_) => loop {
                match listen_rx.recv_timeout(NO_INTERNET_THRESHOLD) {
                    Ok(reply) => {
                        if reply.sequence_number != sequence_number {
                            tracing::trace!(
                                "ICMP packet with unexpected sequence number {:?} received from \
                                 {:?}",
                                reply.sequence_number,
                                reply.addr
                            );
                            continue;
                        }
                        if reply.identifier != identifier {
                            tracing::trace!(
                                "ICMP packet with unexpected identifier {:?} received from {:?}",
                                reply.identifier,
                                reply.addr
                            );
                            continue;
                        }
                        if reply.addr != addr {
                            tracing::trace!(
                                "ICMP packet received from {:?} while {:?} was expected",
                                reply.addr,
                                addr
                            );
                            continue;
                        }
                        break Some(reply.timestamp);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => break None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break 'outer,
                }
            },
            Err(err) => {
                tracing::debug!("Couldn't send ICMP packet: {err:?}");
                None
            }
        };

        let lag = reply_timestamp
            .map_or(NO_INTERNET_THRESHOLD.as_secs_f64() * 1.25, |reply_timestamp| {
                reply_timestamp.duration_since(request_timestamp).as_secs_f64()
            });
        let dt = lag_timer.get_dt().unwrap_or(0.0);
        let lag = lag_filter.add(lag, dt, LAG_FILTER_RC);

        let rssi = poll_rssi().map_or_else(
            |err| {
                tracing::debug!("Couldn't poll signal strength: {err}");
                NO_WLAN_THRESHOLD as f64 * 1.25
            },
            |rssi| rssi as f64,
        );
        let dt = rssi_timer.get_dt().unwrap_or(0.0);
        let rssi = rssi_filter.add(rssi, dt, RSSI_FILTER_RC) as i64;

        if sequence_number % SSID_POLLING_DIVIDER == 0 {
            ssid = poll_ssid().unwrap_or_else(|err| {
                tracing::debug!("Couldn't poll SSID: {err}");
                String::new()
            });
        }

        // Get what we want from the config and drop the mutex fast.
        let slow_internet_ping_threshold = rt.block_on(config.lock()).slow_internet_ping_threshold;
        if report_tx
            .send(Report { lag, slow_internet_ping_threshold, rssi, ssid: ssid.clone() })
            .is_err()
        {
            break 'outer;
        }
        sequence_number = sequence_number.wrapping_add(1);
        thread::sleep(DELAY_BETWEEN_REQUESTS);
    }
}

fn listen_v4_loop(transport_rx: &mut TransportReceiver, listen_tx: &mpsc::Sender<Reply>) {
    let mut iter = icmp_packet_iter(transport_rx);
    loop {
        match iter.next() {
            Ok(packet) => {
                if let Some(reply) = Reply::try_from_raw_v4(packet) {
                    if listen_tx.send(reply).is_err() {
                        break;
                    }
                }
            }
            Err(err) => {
                tracing::debug!("Couldn't receive ICMP packet: {err:?}");
            }
        }
    }
}

fn listen_v6_loop(transport_rx: &mut TransportReceiver, listen_tx: &mpsc::Sender<Reply>) {
    let mut iter = icmpv6_packet_iter(transport_rx);
    loop {
        match iter.next() {
            Ok(packet) => {
                if let Some(reply) = Reply::try_from_raw_v6(packet) {
                    if listen_tx.send(reply).is_err() {
                        break;
                    }
                }
            }
            Err(err) => {
                tracing::debug!("Couldn't receive ICMP packet: {err:?}");
            }
        }
    }
}

fn poll_rssi() -> Result<i64> {
    let output = Command::new("wpa-supplicant-interface")
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
    let output = Command::new("wpa-supplicant-interface")
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

fn resolve_addr() -> Option<IpAddr> {
    match (NETWORK_MONITOR_HOST.as_str(), 0).to_socket_addrs() {
        Ok(mut addrs) => {
            if let Some(addr) = addrs.next() {
                return Some(addr.ip());
            }
            tracing::error!("{} resolved to 0 addresses", *NETWORK_MONITOR_HOST);
        }
        Err(err) => tracing::error!("Couldn't resolve {}: {err:?}", *NETWORK_MONITOR_HOST),
    }
    None
}

fn icmpv4_transport() -> Result<(TransportSender, TransportReceiver), io::Error> {
    transport_channel(
        4096,
        TransportChannelType::Layer4(TransportProtocol::Ipv4(IpNextHeaderProtocols::Icmp)),
    )
}

fn icmpv6_transport() -> Result<(TransportSender, TransportReceiver), io::Error> {
    transport_channel(
        4096,
        TransportChannelType::Layer4(TransportProtocol::Ipv6(IpNextHeaderProtocols::Icmpv6)),
    )
}

fn echo_request_v4(
    buf: &mut [u8],
    sequence_number: u16,
    identifier: u16,
) -> MutableEchoRequestPacketV4 {
    let mut echo_request = MutableEchoRequestPacketV4::new(buf).unwrap();
    echo_request.set_icmp_type(IcmpTypes::EchoRequest);
    echo_request.set_sequence_number(sequence_number);
    echo_request.set_identifier(identifier);
    echo_request.set_checksum(util::checksum(echo_request.packet(), 1));
    echo_request
}

fn echo_request_v6(
    buf: &mut [u8],
    sequence_number: u16,
    identifier: u16,
) -> MutableEchoRequestPacketV6 {
    let mut echo_request = MutableEchoRequestPacketV6::new(buf).unwrap();
    echo_request.set_icmpv6_type(Icmpv6Types::EchoRequest);
    echo_request.set_sequence_number(sequence_number);
    echo_request.set_identifier(identifier);
    echo_request.set_checksum(util::checksum(echo_request.packet(), 1));
    echo_request
}
