//! Wired USB-serial transport for protocol v2.1 (Phase C2 / SW-R5.2).
//!
//! One [`serialport`] port at 921600 baud, 8N1, no flow control, carries
//! the same v2.1 payloads as UDP inside the `"RSHS"` envelope
//! ([`SerialFrame`]): stream frames for the engine's [`PacketSource`],
//! HELLO replies (the wired-pairing key disclosure, OPEN-20 — the USB
//! cable is the trust anchor), status packets, ping frames, and raw
//! `"RSHC"` control frames for the [`ControlTransport`] trait (HMAC
//! verification lives in the control client, not here).
//!
//! The design mirrors [`crate::udp::UdpTransport`]:
//! a `std` blocking port (the workspace tokio pin does not enable the
//! `net` feature, and `serialport` is a blocking API by design —
//! `tokio-serial` 5.5.0 stays the escape hatch per spike decision S-4,
//! `docs/DECISIONS.md`), a blocking reader thread feeding the
//! incremental [`SerialDecoder`] and forwarding decoded frames into
//! bounded tokio `mpsc` channels. [`PacketSource::next_frame`] awaits
//! the channel, which is cancellation-safe, so the engine's zero-timeout
//! polls never lose a frame. Full channels drop the newest frame and
//! count it (`dropped_overflow`) instead of blocking the reader — the
//! engine keeps only the latest frame per tick anyway.
//!
//! # DTR/RTS — the board must never reset on open
//!
//! The ESP32 `DevKit V1` wires DTR/RTS through its auto-reset circuit to
//! the EN/BOOT lines: asserting either resets the board or drops it
//! into the bootloader (protocol v2.1 §10.3). This transport therefore
//! opens every port with `.dtr_on_open(false)` **and** explicitly
//! clears both lines ([`SerialPort::write_data_terminal_ready`] /
//! [`SerialPort::write_request_to_send`] with `false`) immediately
//! after open. Explicit `false` is chosen over
//! `.preserve_dtr_on_open()`: preserve keeps whatever the OS/driver
//! default is, and on Linux that default is DTR asserted — exactly the
//! reset this transport must avoid. One honest caveat, straight from
//! the `serialport` docs: on Linux, DTR is asserted for a short moment
//! on open even with `dtr_on_open(false)`; that cannot be prevented
//! from userspace. On Windows `dtr_on_open(false)` prevents the
//! assertion entirely. The no-reset behaviour on real hardware is a
//! carried gap, verified at the final release on the owner PC.
//!
//! # Auto-detect
//!
//! [`detect_board_port`] enumerates ports, ranks them by USB metadata
//! (CP2102 first — the `DevKit V1` bridge — then CH340 clones, then any
//! Espressif VID, then any USB serial whose manufacturer/product names
//! the board), and returns the first candidate that actually opens
//! with the standard settings. A port that fails to open is skipped
//! silently — an open failure is never fatal to detection. This is the
//! metadata half of protocol §10.6: the HELO-probe half (open each
//! candidate, send HELO, keep the first port that answers a valid
//! 92-byte HELLO within 500 ms) is **not** implemented here — it
//! belongs to the session layer, which owns the HELO exchange — and
//! stays an open item for the final-release gate.

use std::fmt;
use std::future::Future;
use std::io::{self, Read, Write};
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::thread::{self, Builder};
use std::time::Duration;

use rsh_engine::engine::PacketSource;
use rsh_protocol::{
    HelloError, HelloReply, HeloRequest, PingError, PingFrame, SerialDecoder, SerialFrame,
    SerialKind, StatusError, StatusPacket, StreamError, StreamFrame,
};
use serialport::{
    DataBits, FlowControl, Parity, SerialPort, SerialPortInfo, SerialPortType, StopBits,
};
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::{ControlTransport, TransportError};

/// Wired baud rate (protocol v2.1 §10.3): 115200 is not enough headroom
/// at 200 Hz; 921600 runs at ~10% utilisation.
pub const SERIAL_BAUD_RATE: u32 = 921_600;
/// Reader-thread `read` timeout: how promptly shutdown is noticed, and
/// how long an idle port waits between reads.
pub const READ_TIMEOUT: Duration = Duration::from_millis(100);
/// Largest read chunk from the port (max frame 1062 B, with slack).
pub const MAX_READ_LEN: usize = 2048;
/// Stream-frame channel capacity (200 Hz; the engine drains each tick).
const FRAME_CHANNEL_CAPACITY: usize = 256;
/// Control-reply channel capacity (at most one request in flight).
const CONTROL_CHANNEL_CAPACITY: usize = 16;
/// HELLO-reply channel capacity.
const HELLO_CHANNEL_CAPACITY: usize = 8;
/// Status-packet channel capacity.
const STATUS_CHANNEL_CAPACITY: usize = 16;
/// Ping-reply channel capacity.
const PING_CHANNEL_CAPACITY: usize = 8;
/// Reader-thread sleep after a non-timeout read error (back-off).
const READ_ERROR_BACKOFF: Duration = Duration::from_millis(10);

/// Silicon Labs CP2102 VID — the ESP32 `DevKit V1` USB-UART bridge
/// (protocol v2.1 §10.3).
pub const CP2102_VID: u16 = 0x10C4;
/// Silicon Labs CP2102 PID.
pub const CP2102_PID: u16 = 0xEA60;
/// WCH CH340 VID — the bridge on most `DevKit V1` clones.
pub const CH340_VID: u16 = 0x1A86;
/// WCH CH340 PID.
pub const CH340_PID: u16 = 0x7523;
/// Espressif VID — ESP32-S3 native USB (not the `DevKit V1` bridge, but
/// still an RSH board when it shows up as a serial port).
pub const ESPRESSIF_VID: u16 = 0x303A;

/// Live transport counters, shared with the reader thread.
#[derive(Debug, Default)]
struct AtomicCounters {
    /// Bytes read from the port (any bytes, garbage included).
    bytes_received: AtomicU64,
    /// Valid `"RSHS"` envelopes decoded (any kind, before payload parse).
    frames_received: AtomicU64,
    /// Stream frames parsed and handed to the frame channel.
    frames_delivered: AtomicU64,
    /// Bytes skipped while scanning for the `"RSHS"` magic (boot log /
    /// bootloader output).
    dropped_bad_magic: AtomicU64,
    /// Envelopes dropped for a CRC mismatch.
    dropped_bad_crc: AtomicU64,
    /// Envelopes dropped for a length/kind problem.
    dropped_bad_length: AtomicU64,
    /// Valid envelopes whose payload failed to parse (bad stream /
    /// status / HELLO / ping payload).
    dropped_malformed: AtomicU64,
    /// Frames dropped because a channel was full.
    dropped_overflow: AtomicU64,
    /// Envelopes of a kind the board should never send (HELO, PROBE).
    dropped_unexpected: AtomicU64,
    /// Non-timeout read I/O errors.
    receive_errors: AtomicU64,
    /// Write I/O errors.
    send_errors: AtomicU64,
    /// Raw control replies received from the board.
    control_received: AtomicU64,
    /// HELLO replies parsed and handed to the hello channel.
    hello_received: AtomicU64,
    /// HELLO envelopes whose payload failed to parse (protocol §10.2
    /// `HelloBadCount`; also counted in `dropped_malformed`).
    hello_bad: AtomicU64,
    /// Status packets parsed and handed to the status channel.
    status_received: AtomicU64,
    /// Ping frames parsed and handed to the ping channel.
    ping_received: AtomicU64,
}

impl AtomicCounters {
    /// Snapshot the live counters.
    fn snapshot(&self) -> SerialCounters {
        let load = |c: &AtomicU64| c.load(Ordering::Relaxed);
        SerialCounters {
            bytes_received: load(&self.bytes_received),
            frames_received: load(&self.frames_received),
            frames_delivered: load(&self.frames_delivered),
            dropped_bad_magic: load(&self.dropped_bad_magic),
            dropped_bad_crc: load(&self.dropped_bad_crc),
            dropped_bad_length: load(&self.dropped_bad_length),
            dropped_malformed: load(&self.dropped_malformed),
            dropped_overflow: load(&self.dropped_overflow),
            dropped_unexpected: load(&self.dropped_unexpected),
            frames_dropped: load(&self.dropped_bad_magic)
                + load(&self.dropped_bad_crc)
                + load(&self.dropped_bad_length)
                + load(&self.dropped_malformed)
                + load(&self.dropped_overflow)
                + load(&self.dropped_unexpected),
            receive_errors: load(&self.receive_errors),
            send_errors: load(&self.send_errors),
            control_received: load(&self.control_received),
            hello_received: load(&self.hello_received),
            hello_bad: load(&self.hello_bad),
            status_received: load(&self.status_received),
            ping_received: load(&self.ping_received),
        }
    }
}

/// Snapshot of the [`SerialTransport`] counters (see `AtomicCounters`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SerialCounters {
    /// Bytes read from the port (any bytes, garbage included).
    pub bytes_received: u64,
    /// Valid `"RSHS"` envelopes decoded (any kind, before payload parse).
    pub frames_received: u64,
    /// Stream frames parsed and handed to the frame channel.
    pub frames_delivered: u64,
    /// Bytes skipped while scanning for the `"RSHS"` magic (boot log /
    /// bootloader output).
    pub dropped_bad_magic: u64,
    /// Envelopes dropped for a CRC mismatch.
    pub dropped_bad_crc: u64,
    /// Envelopes dropped for a length/kind problem.
    pub dropped_bad_length: u64,
    /// Valid envelopes whose payload failed to parse (bad stream /
    /// status / HELLO / ping payload).
    pub dropped_malformed: u64,
    /// Frames dropped because a channel was full.
    pub dropped_overflow: u64,
    /// Envelopes of a kind the board should never send (HELO, PROBE).
    pub dropped_unexpected: u64,
    /// Frames dropped for any reason (sum of the `dropped_*` above).
    pub frames_dropped: u64,
    /// Non-timeout read I/O errors.
    pub receive_errors: u64,
    /// Write I/O errors.
    pub send_errors: u64,
    /// Raw control replies received from the board.
    pub control_received: u64,
    /// HELLO replies parsed and handed to the hello channel.
    pub hello_received: u64,
    /// HELLO envelopes whose payload failed to parse (protocol §10.2
    /// `HelloBadCount`; also counted in `dropped_malformed`).
    pub hello_bad: u64,
    /// Status packets parsed and handed to the status channel.
    pub status_received: u64,
    /// Ping frames parsed and handed to the ping channel.
    pub ping_received: u64,
}

/// Configuration for [`SerialTransport::open`].
#[derive(Clone, Debug)]
pub struct SerialConfig {
    /// Port name, e.g. `"COM3"` on Windows or `"/dev/ttyUSB0"` on Linux.
    pub port_name: String,
    /// Baud rate (default [`SERIAL_BAUD_RATE`]).
    pub baud_rate: u32,
    /// Reader `read` timeout (default [`READ_TIMEOUT`]).
    pub read_timeout: Duration,
}

impl SerialConfig {
    /// Build a config for `port_name` at [`SERIAL_BAUD_RATE`], 8N1, no
    /// flow control.
    #[must_use]
    pub fn new(port_name: impl Into<String>) -> Self {
        Self {
            port_name: port_name.into(),
            baud_rate: SERIAL_BAUD_RATE,
            read_timeout: READ_TIMEOUT,
        }
    }
}

/// Channel senders the reader thread forwards decoded frames into.
struct DemuxSenders {
    /// Decoded stream frames.
    frame: mpsc::Sender<StreamFrame>,
    /// Raw control replies (the `"RSHC"` payload bytes).
    control: mpsc::Sender<Vec<u8>>,
    /// Decoded HELLO replies.
    hello: mpsc::Sender<HelloReply>,
    /// Decoded status packets.
    status: mpsc::Sender<StatusPacket>,
    /// Decoded ping frames.
    ping: mpsc::Sender<PingFrame>,
}

/// Wired USB-serial transport: `RSHS` framing at 921600 baud (C2).
///
/// Owns one open port plus a reader thread; the thread stops promptly
/// after [`Self::shutdown`] or when the transport is dropped. The
/// write side of the port is shared (`Arc<Mutex<…>>`) between
/// [`ControlTransport::send_control`], [`Self::send_helo`] and
/// [`Self::send_ping`]; the reader thread owns a `try_clone`d read
/// side, so reads and writes never block each other.
pub struct SerialTransport {
    /// Port name, as opened.
    port_name: String,
    /// Write side of the port (shared with the async send paths).
    port: Arc<Mutex<Box<dyn SerialPort>>>,
    /// Stream frames for [`PacketSource::next_frame`].
    frame_rx: mpsc::Receiver<StreamFrame>,
    /// Raw control replies for [`ControlTransport::recv_control`].
    control_rx: mpsc::Receiver<Vec<u8>>,
    /// HELLO replies, in arrival order.
    hello_rx: mpsc::Receiver<HelloReply>,
    /// Status packets, in arrival order.
    status_rx: mpsc::Receiver<StatusPacket>,
    /// Ping frames, in arrival order.
    ping_rx: mpsc::Receiver<PingFrame>,
    /// Live counters (shared with the reader thread).
    counters: Arc<AtomicCounters>,
    /// Set by [`Self::shutdown`] / [`Drop`]; the reader exits promptly.
    shutdown: Arc<AtomicBool>,
}

impl fmt::Debug for SerialTransport {
    /// Debug shows only the port name: the port itself and the channel
    /// receivers do not implement `Debug`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SerialTransport")
            .field("port_name", &self.port_name)
            .finish_non_exhaustive()
    }
}

impl SerialTransport {
    /// Open the port from `config` and start the reader thread.
    ///
    /// The port is opened at `config.baud_rate`, 8N1, no flow control,
    /// with DTR/RTS explicitly de-asserted (see the module docs — the
    /// board must never reset on open).
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::Io`] when the port cannot be opened,
    /// the read side cannot be cloned, or the reader thread cannot be
    /// spawned.
    pub fn open(config: SerialConfig) -> Result<Self, TransportError> {
        let port = open_serial_port(&config.port_name, config.baud_rate, config.read_timeout)?;
        let reader_port = port
            .try_clone()
            .map_err(|e| TransportError::Io(io::Error::from(e)))?;
        let counters = Arc::new(AtomicCounters::default());
        let shutdown = Arc::new(AtomicBool::new(false));
        let (frame_tx, frame_rx) = mpsc::channel(FRAME_CHANNEL_CAPACITY);
        let (control_tx, control_rx) = mpsc::channel(CONTROL_CHANNEL_CAPACITY);
        let (hello_tx, hello_rx) = mpsc::channel(HELLO_CHANNEL_CAPACITY);
        let (status_tx, status_rx) = mpsc::channel(STATUS_CHANNEL_CAPACITY);
        let (ping_tx, ping_rx) = mpsc::channel(PING_CHANNEL_CAPACITY);
        let senders = DemuxSenders {
            frame: frame_tx,
            control: control_tx,
            hello: hello_tx,
            status: status_tx,
            ping: ping_tx,
        };
        spawn_reader(
            reader_port,
            senders,
            Arc::clone(&counters),
            Arc::clone(&shutdown),
        )?;
        Ok(Self {
            port_name: config.port_name,
            port: Arc::new(Mutex::new(port)),
            frame_rx,
            control_rx,
            hello_rx,
            status_rx,
            ping_rx,
            counters,
            shutdown,
        })
    }

    /// Open `port_name` with the default config (921600 baud).
    ///
    /// # Errors
    ///
    /// Same as [`Self::open`].
    pub fn open_port(port_name: &str) -> Result<Self, TransportError> {
        Self::open(SerialConfig::new(port_name))
    }

    /// Port name, as opened.
    #[must_use]
    pub fn port_name(&self) -> &str {
        &self.port_name
    }

    /// Snapshot of the live counters.
    #[must_use]
    pub fn counters(&self) -> SerialCounters {
        self.counters.snapshot()
    }

    /// Stop the reader thread. Idempotent; [`Drop`] calls this.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }

    /// Send a HELO request (protocol §2.1) over the serial link,
    /// wrapped as an `RSHS` kind-6 frame. The session layer drives
    /// this — there is no automatic HELO thread here, unlike the UDP
    /// transport's 1 Hz HELO: over serial the session is opened on
    /// demand (connect / pair / reconnect), and the board's serial
    /// session ends on HELO timeout.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::Io`] when the frame cannot be
    /// encoded or written.
    pub fn send_helo(&self, session_flag: u8) -> Result<(), TransportError> {
        let payload = HeloRequest::new_v2(session_flag).encode();
        write_frame_to(
            &self.port,
            &self.counters,
            &SerialFrame::new(SerialKind::Helo, payload),
        )
    }

    /// Send a ping frame over the serial link, wrapped as an `RSHS`
    /// kind-5 frame. The ping client drives this; the reply arrives
    /// on [`Self::next_ping`].
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::Io`] when the frame cannot be
    /// encoded or written.
    pub fn send_ping(&self, frame: &PingFrame) -> Result<(), TransportError> {
        write_frame_to(
            &self.port,
            &self.counters,
            &SerialFrame::new(SerialKind::Ping, frame.encode().to_vec()),
        )
    }

    /// Await the next HELLO reply. Returns [`None`] once the transport
    /// is shut down.
    pub fn next_hello(&mut self) -> impl Future<Output = Option<HelloReply>> + Send {
        self.hello_rx.recv()
    }

    /// Await the next status packet. Returns [`None`] once the
    /// transport is shut down.
    pub fn next_status(&mut self) -> impl Future<Output = Option<StatusPacket>> + Send {
        self.status_rx.recv()
    }

    /// Await the next ping frame. Returns [`None`] once the transport
    /// is shut down.
    pub fn next_ping(&mut self) -> impl Future<Output = Option<PingFrame>> + Send {
        self.ping_rx.recv()
    }
}

impl Drop for SerialTransport {
    /// Signal the reader thread to exit; it notices within ~100 ms.
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl PacketSource for SerialTransport {
    /// Await the next parsed stream frame. Malformed bytes never reach
    /// here (dropped + counted by the reader thread); [`None`] means
    /// the transport shut down.
    fn next_frame(&mut self) -> impl Future<Output = Option<StreamFrame>> + Send {
        self.frame_rx.recv()
    }

    /// Non-blocking drain of one queued status packet (H1 condition
    /// 12: the status pump — same contract as the UDP transport).
    fn try_next_status(&mut self) -> Option<StatusPacket> {
        self.status_rx.try_recv().ok()
    }
}

impl ControlTransport for SerialTransport {
    /// Send one raw control frame, wrapped as an `RSHS` kind-4 frame.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::Io`] when the payload exceeds 1053
    /// bytes or the frame cannot be written.
    fn send_control(
        &mut self,
        bytes: &[u8],
    ) -> impl Future<Output = Result<(), TransportError>> + Send {
        let port = Arc::clone(&self.port);
        let counters = Arc::clone(&self.counters);
        let owned = bytes.to_vec();
        async move {
            write_frame_to(
                &port,
                &counters,
                &SerialFrame::new(SerialKind::Control, owned),
            )
        }
    }

    /// Receive one raw control reply (the `"RSHC"` payload bytes),
    /// waiting at most `timeout_duration`.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::Timeout`] when no reply arrives in
    /// time, and [`TransportError::Closed`] when the transport shut
    /// down.
    async fn recv_control(
        &mut self,
        timeout_duration: Duration,
    ) -> Result<Vec<u8>, TransportError> {
        match timeout(timeout_duration, self.control_rx.recv()).await {
            Ok(Some(bytes)) => Ok(bytes),
            Ok(None) => Err(TransportError::Closed),
            Err(_) => Err(TransportError::Timeout),
        }
    }
}

/// Enumerate the serial ports on this machine.
///
/// Never fails from the caller's point of view: an enumeration error
/// (no ports, no permission) yields an empty list, matching
/// `serialport`'s own contract that a listed port may not exist or be
/// available.
#[must_use]
pub fn available_ports() -> Vec<SerialPortInfo> {
    serialport::available_ports().unwrap_or_default()
}

/// Detect the RSH board's port.
///
/// Ports are ranked by [`board_match_score`] (CP2102 first, then
/// CH340, then Espressif VID, then any USB serial naming the board,
/// then any USB serial); ties keep enumeration order. Candidates are
/// tried in rank order and the first one that opens with the standard
/// settings (921600 baud, DTR/RTS off — the open is closed again
/// immediately, so the board is not reset) wins. A port that fails to
/// open is skipped, never fatal. Returns [`None`] when no port
/// matches or none opens.
///
/// This is the metadata half of protocol §10.6 auto-detect. It does
/// **not** send a HELO probe: a port that opens and matches is
/// returned on metadata alone, and the session layer confirms it is
/// an RSH board with the HELO/HELLO exchange after opening it for
/// real.
#[must_use]
pub fn detect_board_port() -> Option<String> {
    let mut ports = available_ports();
    ports.sort_by_key(|info| std::cmp::Reverse(board_match_score(info)));
    for info in ports {
        if board_match_score(&info) == 0 {
            continue;
        }
        if open_serial_port(&info.port_name, SERIAL_BAUD_RATE, READ_TIMEOUT).is_ok() {
            return Some(info.port_name);
        }
    }
    None
}

/// Rank a port by how likely it is the RSH board (higher is better).
///
/// * 100 — CP2102 (`0x10C4:0xEA60`), the `DevKit V1` bridge;
/// * 90 — CH340 (`0x1A86:0x7523`), the bridge on most clones;
/// * 80 — any Espressif VID (`0x303A`) device;
/// * 70 — a USB port whose manufacturer/product names the board
///   (`esp32`, `cp210`, `ch340`, `silicon labs`);
/// * 10 — any other USB serial port;
/// * 0 — non-USB / unknown port types (never a board candidate).
#[must_use]
pub fn board_match_score(info: &SerialPortInfo) -> u8 {
    let SerialPortType::UsbPort(usb) = &info.port_type else {
        return 0;
    };
    if usb.vid == CP2102_VID && usb.pid == CP2102_PID {
        return 100;
    }
    if usb.vid == CH340_VID && usb.pid == CH340_PID {
        return 90;
    }
    if usb.vid == ESPRESSIF_VID {
        return 80;
    }
    let haystack = format!(
        "{} {}",
        usb.manufacturer.as_deref().unwrap_or_default(),
        usb.product.as_deref().unwrap_or_default(),
    )
    .to_lowercase();
    if haystack.contains("esp32")
        || haystack.contains("cp210")
        || haystack.contains("ch340")
        || haystack.contains("silicon labs")
    {
        return 70;
    }
    10
}

/// Open a port with the standard RSH settings: 8N1, no flow control,
/// and DTR/RTS explicitly de-asserted so the board never resets (see
/// the module docs). The explicit post-open clear is best-effort —
/// the builder's `.dtr_on_open(false)` is the primary mechanism, and
/// a driver that refuses the line write must not fail the open.
fn open_serial_port(
    port_name: &str,
    baud_rate: u32,
    read_timeout: Duration,
) -> Result<Box<dyn SerialPort>, TransportError> {
    let mut port = serialport::new(port_name, baud_rate)
        .timeout(read_timeout)
        .data_bits(DataBits::Eight)
        .parity(Parity::None)
        .stop_bits(StopBits::One)
        .flow_control(FlowControl::None)
        .dtr_on_open(false)
        .open()
        .map_err(|e| TransportError::Io(io::Error::from(e)))?;
    let _ = port.write_data_terminal_ready(false);
    let _ = port.write_request_to_send(false);
    Ok(port)
}

/// Lock the shared write side, tolerating a poisoned lock.
fn lock_port(port: &Mutex<Box<dyn SerialPort>>) -> MutexGuard<'_, Box<dyn SerialPort>> {
    match port.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Encode `frame` as `RSHS` bytes and write them to the shared port.
/// A write failure is counted in `send_errors`.
fn write_frame_to(
    port: &Arc<Mutex<Box<dyn SerialPort>>>,
    counters: &Arc<AtomicCounters>,
    frame: &SerialFrame,
) -> Result<(), TransportError> {
    let bytes = frame.encode().map_err(|_| {
        TransportError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "serial payload exceeds 1053 bytes",
        ))
    })?;
    let mut guard = lock_port(port);
    match guard.write_all(&bytes).and_then(|()| guard.flush()) {
        Ok(()) => Ok(()),
        Err(e) => {
            counters.send_errors.fetch_add(1, Ordering::Relaxed);
            Err(TransportError::Io(e))
        }
    }
}

/// Spawn the reader thread (detached; it exits on shutdown).
fn spawn_reader(
    port: Box<dyn SerialPort>,
    senders: DemuxSenders,
    counters: Arc<AtomicCounters>,
    shutdown: Arc<AtomicBool>,
) -> Result<(), TransportError> {
    Builder::new()
        .name("rsh-serial-reader".to_owned())
        .spawn(move || {
            reader_loop(port, &senders, &counters, &shutdown);
        })
        .map(|_| ())
        .map_err(TransportError::Io)
}

/// The reader loop: `read`, feed the incremental decoder, demux the
/// decoded envelopes. A read timeout is the normal idle case (no data
/// yet) and is neither an error nor counted; only other I/O errors
/// are counted, with a short back-off so a dead port cannot spin.
fn reader_loop(
    mut port: Box<dyn SerialPort>,
    senders: &DemuxSenders,
    counters: &AtomicCounters,
    shutdown: &AtomicBool,
) {
    let mut decoder = SerialDecoder::new();
    let mut buf = [0u8; MAX_READ_LEN];
    while !shutdown.load(Ordering::Relaxed) {
        match port.read(&mut buf) {
            Ok(0) => thread::sleep(Duration::from_millis(1)),
            Ok(n) => {
                counters
                    .bytes_received
                    .fetch_add(u64::try_from(n).unwrap_or(u64::MAX), Ordering::Relaxed);
                let before = (
                    decoder.dropped_bad_magic,
                    decoder.dropped_bad_crc,
                    decoder.dropped_bad_length,
                );
                let frames = decoder.feed(&buf[..n]);
                let after = (
                    decoder.dropped_bad_magic,
                    decoder.dropped_bad_crc,
                    decoder.dropped_bad_length,
                );
                counters
                    .dropped_bad_magic
                    .fetch_add(after.0 - before.0, Ordering::Relaxed);
                counters
                    .dropped_bad_crc
                    .fetch_add(after.1 - before.1, Ordering::Relaxed);
                counters
                    .dropped_bad_length
                    .fetch_add(after.2 - before.2, Ordering::Relaxed);
                for frame in frames {
                    counters.frames_received.fetch_add(1, Ordering::Relaxed);
                    handle_serial_frame(frame, senders, counters);
                }
            }
            Err(ref e)
                if e.kind() == io::ErrorKind::TimedOut
                    || e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::Interrupted =>
            {
                // Idle port / spurious wakeup: nothing to do.
            }
            Err(_) => {
                counters.receive_errors.fetch_add(1, Ordering::Relaxed);
                thread::sleep(READ_ERROR_BACKOFF);
            }
        }
    }
}

/// Demux one decoded envelope by kind, parse its payload, and forward
/// it — or drop it and count the reason. Never panics on untrusted
/// input; a partial frame can never reach here (the decoder only
/// delivers complete, CRC-valid envelopes).
fn handle_serial_frame(frame: SerialFrame, senders: &DemuxSenders, counters: &AtomicCounters) {
    match frame.kind {
        SerialKind::Stream => match StreamFrame::parse(&frame.payload) {
            Ok(parsed) => {
                counters.frames_delivered.fetch_add(1, Ordering::Relaxed);
                if senders.frame.try_send(parsed).is_err() {
                    count_overflow(counters);
                }
            }
            Err(StreamError::BadCrc) => {
                counters.dropped_bad_crc.fetch_add(1, Ordering::Relaxed);
                counters.dropped_malformed.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                counters.dropped_malformed.fetch_add(1, Ordering::Relaxed);
            }
        },
        SerialKind::Status => match StatusPacket::parse(&frame.payload) {
            Ok(packet) => {
                counters.status_received.fetch_add(1, Ordering::Relaxed);
                if senders.status.try_send(packet).is_err() {
                    count_overflow(counters);
                }
            }
            Err(StatusError::BadCrc) => {
                counters.dropped_bad_crc.fetch_add(1, Ordering::Relaxed);
                counters.dropped_malformed.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                counters.dropped_malformed.fetch_add(1, Ordering::Relaxed);
            }
        },
        SerialKind::Hello => match HelloReply::parse(&frame.payload) {
            Ok(reply) => {
                counters.hello_received.fetch_add(1, Ordering::Relaxed);
                if senders.hello.try_send(reply).is_err() {
                    count_overflow(counters);
                }
            }
            Err(HelloError::BadCrc) => {
                counters.dropped_bad_crc.fetch_add(1, Ordering::Relaxed);
                counters.dropped_malformed.fetch_add(1, Ordering::Relaxed);
                counters.hello_bad.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                counters.dropped_malformed.fetch_add(1, Ordering::Relaxed);
                counters.hello_bad.fetch_add(1, Ordering::Relaxed);
            }
        },
        SerialKind::Control => {
            // Control replies are raw bytes here; HMAC verification
            // belongs to the control client, which matches them by
            // `reqId`.
            counters.control_received.fetch_add(1, Ordering::Relaxed);
            if senders.control.try_send(frame.payload).is_err() {
                count_overflow(counters);
            }
        }
        SerialKind::Ping => match PingFrame::parse(&frame.payload) {
            Ok(parsed) => {
                counters.ping_received.fetch_add(1, Ordering::Relaxed);
                if senders.ping.try_send(parsed).is_err() {
                    count_overflow(counters);
                }
            }
            Err(PingError::BadCrc) => {
                counters.dropped_bad_crc.fetch_add(1, Ordering::Relaxed);
                counters.dropped_malformed.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                counters.dropped_malformed.fetch_add(1, Ordering::Relaxed);
            }
        },
        // HELO and PROBE from the board are wrong-direction traffic:
        // dropped and counted, never delivered.
        SerialKind::Helo | SerialKind::Probe => {
            counters.dropped_unexpected.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Count a frame lost because its channel was full.
fn count_overflow(counters: &AtomicCounters) {
    counters.dropped_overflow.fetch_add(1, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use rsh_protocol::StreamFlags;
    use serialport::UsbPortInfo;

    use super::*;

    /// A stream frame with steering on slot 0 and two encoders.
    fn test_frame(seq: u16) -> StreamFrame {
        StreamFrame::new(
            seq,
            u32::from(seq) * 5_000,
            StreamFlags::default(),
            &[(0, 1650)],
            &[10, -5],
            0b101,
        )
    }

    /// A status packet with only the loss counters set.
    fn test_status() -> StatusPacket {
        StatusPacket {
            seq: 0,
            uptime_ms: 0,
            rssi: 0,
            battery_pct: 255,
            supply_mv: 0,
            heap: 0,
            overruns: 0,
            max_cycle_us: 0,
            governor: 0,
            reset_reason: 1,
            cfg_crc: 0,
            cfg_ver: 0,
            seq_gap: 7,
            tx_fail: 3,
            btns_ex: None,
        }
    }

    /// Decode a lowercase hex string (test helper only).
    fn from_hex(hex: &str) -> Vec<u8> {
        let digits = hex.as_bytes();
        let mut out = Vec::with_capacity(hex.len() / 2);
        for pair in digits.as_chunks::<2>().0 {
            let hi = char::from(pair[0]).to_digit(16).expect("hex digit");
            let lo = char::from(pair[1]).to_digit(16).expect("hex digit");
            out.push(u8::try_from(hi * 16 + lo).expect("hex byte fits"));
        }
        out
    }

    /// A demux harness: channels plus counters, with no port attached.
    struct DemuxHarness {
        /// Frame channel receiver.
        frame_rx: mpsc::Receiver<StreamFrame>,
        /// Control channel receiver.
        control_rx: mpsc::Receiver<Vec<u8>>,
        /// HELLO channel receiver.
        hello_rx: mpsc::Receiver<HelloReply>,
        /// Status channel receiver.
        status_rx: mpsc::Receiver<StatusPacket>,
        /// Ping channel receiver.
        ping_rx: mpsc::Receiver<PingFrame>,
        /// Counters the demux writes to.
        counters: AtomicCounters,
        /// Senders handed to `handle_serial_frame`.
        senders: DemuxSenders,
    }

    impl DemuxHarness {
        /// Build the harness.
        fn new() -> Self {
            let (frame_tx, frame_rx) = mpsc::channel(FRAME_CHANNEL_CAPACITY);
            let (control_tx, control_rx) = mpsc::channel(CONTROL_CHANNEL_CAPACITY);
            let (hello_tx, hello_rx) = mpsc::channel(HELLO_CHANNEL_CAPACITY);
            let (status_tx, status_rx) = mpsc::channel(STATUS_CHANNEL_CAPACITY);
            let (ping_tx, ping_rx) = mpsc::channel(PING_CHANNEL_CAPACITY);
            Self {
                frame_rx,
                control_rx,
                hello_rx,
                status_rx,
                ping_rx,
                counters: AtomicCounters::default(),
                senders: DemuxSenders {
                    frame: frame_tx,
                    control: control_tx,
                    hello: hello_tx,
                    status: status_tx,
                    ping: ping_tx,
                },
            }
        }

        /// Feed one envelope through the demux.
        fn feed(&self, frame: SerialFrame) {
            handle_serial_frame(frame, &self.senders, &self.counters);
        }

        /// Snapshot the counters.
        fn snapshot(&self) -> SerialCounters {
            self.counters.snapshot()
        }
    }

    /// A `SerialPortInfo` for a USB device with the given VID/PID.
    fn usb_info(port_name: &str, vid: u16, pid: u16) -> SerialPortInfo {
        SerialPortInfo {
            port_name: port_name.to_owned(),
            port_type: SerialPortType::UsbPort(UsbPortInfo {
                vid,
                pid,
                serial_number: None,
                manufacturer: None,
                product: None,
            }),
        }
    }

    #[test]
    fn transport_is_send() {
        /// Compile-time assertion: the transport must be spawnable.
        fn assert_send<T: Send>() {}
        assert_send::<SerialTransport>();
    }

    #[test]
    fn config_defaults_to_921600_baud() {
        let config = SerialConfig::new("/dev/ttyUSB0");
        assert_eq!(config.port_name, "/dev/ttyUSB0");
        assert_eq!(config.baud_rate, SERIAL_BAUD_RATE);
        assert_eq!(config.baud_rate, 921_600);
        assert_eq!(config.read_timeout, READ_TIMEOUT);
    }

    #[test]
    fn canonical_vectors_byte_exact() {
        // Protocol §10.1 canonical vectors (3-language match).
        let stream = SerialFrame::new(SerialKind::Stream, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(
            stream.encode().expect("stream frame encodes"),
            from_hex("525348530108000102030405060708d95d"),
            "stream vector (CRC 0x5DD9)"
        );
        let helo = SerialFrame::new(SerialKind::Helo, b"HELO\x02\x00".to_vec());
        assert_eq!(
            helo.encode().expect("helo frame encodes"),
            from_hex("5253485306060048454c4f02004749"),
            "helo vector (CRC 0x4947)"
        );
        // And both parse back to the same frame.
        for hex in [
            "525348530108000102030405060708d95d",
            "5253485306060048454c4f02004749",
        ] {
            let bytes = from_hex(hex);
            let parsed = SerialFrame::parse(&bytes).expect("canonical vector parses");
            assert_eq!(parsed.encode().expect("re-encodes"), bytes);
        }
    }

    #[test]
    fn serial_frame_round_trip_all_kinds() {
        let payload = test_frame(9).encode();
        for kind in [
            SerialKind::Stream,
            SerialKind::Status,
            SerialKind::Hello,
            SerialKind::Control,
            SerialKind::Ping,
            SerialKind::Helo,
            SerialKind::Probe,
        ] {
            let frame = SerialFrame::new(kind, payload.clone());
            let bytes = frame.encode().expect("frame encodes");
            assert_eq!(SerialFrame::parse(&bytes).expect("frame parses"), frame);
        }
    }

    #[test]
    fn encode_rejects_oversize_payload() {
        let frame = SerialFrame::new(SerialKind::Control, vec![0u8; MAX_READ_LEN]);
        assert!(
            frame.encode().is_err(),
            "payload over 1053 B must not encode"
        );
        let max = SerialFrame::new(SerialKind::Control, vec![0u8; 1053]);
        assert!(max.encode().is_ok(), "exactly 1053 B must encode");
    }

    #[test]
    fn decoder_resyncs_over_boot_log_and_bad_crc() {
        let good = SerialFrame::new(SerialKind::Stream, test_frame(4).encode());
        let good_bytes = good.encode().expect("good frame encodes");
        let mut bad = SerialFrame::new(SerialKind::Stream, test_frame(5).encode())
            .encode()
            .expect("bad frame encodes");
        let last = bad.len() - 1;
        bad[last] ^= 0xFF; // corrupt the CRC trailer
        let mut decoder = SerialDecoder::new();
        let mut out = Vec::new();
        // Boot log, then a bad-CRC frame, then the good frame.
        out.extend(decoder.feed(b"ets Jul 29 2019 12:21:46\r\nrst:0x1\r\n"));
        out.extend(decoder.feed(&bad));
        out.extend(decoder.feed(&good_bytes));
        assert_eq!(out.len(), 1, "only the good frame is delivered");
        assert_eq!(out[0].kind, SerialKind::Stream);
        assert_eq!(decoder.dropped_bad_crc, 1);
        assert!(decoder.dropped_bad_magic > 0, "boot log bytes counted");
        assert_eq!(decoder.dropped_total(), decoder.dropped_bad_magic + 1);
    }

    #[test]
    fn decoder_split_magic_and_partial_frame_never_delivered() {
        let frame = SerialFrame::new(SerialKind::Helo, b"HELO\x02\x00".to_vec());
        let bytes = frame.encode().expect("frame encodes");
        let mut decoder = SerialDecoder::new();
        // Magic split across feeds, then a partial frame: nothing yet.
        assert_eq!(decoder.feed(&bytes[..3]).len(), 0);
        assert_eq!(decoder.feed(&bytes[3..7]).len(), 0);
        assert!(
            decoder.feed(&bytes[7..10]).is_empty(),
            "partial, no delivery"
        );
        // The rest completes it.
        let out = decoder.feed(&bytes[10..]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], frame);
    }

    #[test]
    fn decoder_drops_unknown_kind_and_oversize_length() {
        let mut frame = SerialFrame::new(SerialKind::Stream, vec![1, 2, 3])
            .encode()
            .expect("frame encodes");
        frame[4] = 0x77; // unknown kind
        // Fix the CRC so only the kind is wrong.
        let crc = rsh_protocol::crc16(&frame[..frame.len() - 2]);
        let n = frame.len();
        frame[n - 2..].copy_from_slice(&crc.to_le_bytes());
        let mut decoder = SerialDecoder::new();
        assert_eq!(decoder.feed(&frame).len(), 0);
        assert_eq!(decoder.dropped_bad_length, 1);
    }

    #[test]
    fn demux_routes_all_kinds() {
        let mut harness = DemuxHarness::new();
        let frame = test_frame(11);
        harness.feed(SerialFrame::new(SerialKind::Stream, frame.encode()));
        harness.feed(SerialFrame::new(SerialKind::Status, test_status().encode()));
        harness.feed(SerialFrame::new(
            SerialKind::Control,
            b"RSHC-reply".to_vec(),
        ));
        let ping = PingFrame::reply(3, 1000, 1100, 1150);
        harness.feed(SerialFrame::new(SerialKind::Ping, ping.encode().to_vec()));
        assert_eq!(
            harness.frame_rx.try_recv().expect("stream frame routed"),
            frame
        );
        assert_eq!(
            harness.status_rx.try_recv().expect("status routed").seq_gap,
            7
        );
        assert_eq!(
            harness.control_rx.try_recv().expect("control routed"),
            b"RSHC-reply"
        );
        assert_eq!(harness.ping_rx.try_recv().expect("ping routed").seq, 3);
        let counters = harness.snapshot();
        assert_eq!(counters.frames_delivered, 1);
        assert_eq!(counters.status_received, 1);
        assert_eq!(counters.control_received, 1);
        assert_eq!(counters.ping_received, 1);
        assert_eq!(counters.frames_dropped, 0);
    }

    #[test]
    fn demux_counts_bad_payloads_and_hello_bad() {
        let mut harness = DemuxHarness::new();
        // Stream payload with a corrupt CRC trailer.
        let mut stream_bytes = test_frame(2).encode();
        let last = stream_bytes.len() - 1;
        stream_bytes[last] ^= 0xFF;
        harness.feed(SerialFrame::new(SerialKind::Stream, stream_bytes));
        // HELLO payload that is not a HELLO at all (valid envelope,
        // garbage payload): counts as malformed AND hello_bad (§10.2).
        harness.feed(SerialFrame::new(SerialKind::Hello, vec![0u8; 10]));
        // Wrong-direction kinds from the board.
        harness.feed(SerialFrame::new(SerialKind::Helo, vec![1]));
        harness.feed(SerialFrame::new(SerialKind::Probe, vec![1]));
        let counters = harness.snapshot();
        assert_eq!(counters.dropped_bad_crc, 1);
        assert_eq!(counters.dropped_malformed, 2);
        assert_eq!(counters.hello_bad, 1);
        assert_eq!(counters.dropped_unexpected, 2);
        assert_eq!(counters.frames_dropped, 1 + 2 + 2);
        assert_eq!(counters.frames_delivered, 0);
        assert!(harness.frame_rx.try_recv().is_err(), "no frame delivered");
        assert!(harness.hello_rx.try_recv().is_err(), "no HELLO delivered");
    }

    #[test]
    fn demux_overflow_drops_newest_and_counts() {
        // A 1-capacity channel stands in for a full channel: the second
        // frame cannot be delivered and is counted, never blocking.
        let (frame_tx, mut frame_rx) = mpsc::channel(1);
        let (control_tx, _control_rx) = mpsc::channel(CONTROL_CHANNEL_CAPACITY);
        let (hello_tx, _hello_rx) = mpsc::channel(HELLO_CHANNEL_CAPACITY);
        let (status_tx, _status_rx) = mpsc::channel(STATUS_CHANNEL_CAPACITY);
        let (ping_tx, _ping_rx) = mpsc::channel(PING_CHANNEL_CAPACITY);
        let senders = DemuxSenders {
            frame: frame_tx,
            control: control_tx,
            hello: hello_tx,
            status: status_tx,
            ping: ping_tx,
        };
        let counters = AtomicCounters::default();
        let first = SerialFrame::new(SerialKind::Stream, test_frame(1).encode());
        let second = SerialFrame::new(SerialKind::Stream, test_frame(2).encode());
        handle_serial_frame(first, &senders, &counters);
        handle_serial_frame(second, &senders, &counters);
        assert_eq!(frame_rx.try_recv().expect("first frame kept").seq, 1);
        assert!(frame_rx.try_recv().is_err(), "second frame dropped");
        let snapshot = counters.snapshot();
        assert_eq!(snapshot.frames_delivered, 2, "both parsed, one dropped");
        assert_eq!(snapshot.dropped_overflow, 1);
        assert_eq!(snapshot.frames_dropped, 1);
    }

    #[test]
    fn board_match_score_ranks_cp2102_first() {
        assert_eq!(
            board_match_score(&usb_info("/dev/ttyUSB0", CP2102_VID, CP2102_PID)),
            100
        );
        assert_eq!(
            board_match_score(&usb_info("/dev/ttyUSB1", CH340_VID, CH340_PID)),
            90
        );
        assert_eq!(
            board_match_score(&usb_info("/dev/ttyACM0", ESPRESSIF_VID, 0x1001)),
            80
        );
        let named = SerialPortInfo {
            port_name: "/dev/ttyUSB2".to_owned(),
            port_type: SerialPortType::UsbPort(UsbPortInfo {
                vid: 0x0403,
                pid: 0x6001,
                serial_number: None,
                manufacturer: Some("Silicon Labs".to_owned()),
                product: Some("CP2102 USB to UART Bridge".to_owned()),
            }),
        };
        assert_eq!(board_match_score(&named), 70);
        assert_eq!(
            board_match_score(&usb_info("/dev/ttyUSB3", 0x0403, 0x6001)),
            10
        );
        assert_eq!(
            board_match_score(&SerialPortInfo {
                port_name: "COM1".to_owned(),
                port_type: SerialPortType::PciPort,
            }),
            0,
            "non-USB ports are never board candidates"
        );
        assert_eq!(
            board_match_score(&SerialPortInfo {
                port_name: "COM2".to_owned(),
                port_type: SerialPortType::Unknown,
            }),
            0
        );
    }

    #[test]
    fn open_missing_port_returns_typed_error_not_panic() {
        let err = SerialTransport::open(SerialConfig::new("/dev/ttyRSH-does-not-exist"))
            .expect_err("a port that does not exist must fail to open");
        assert!(
            matches!(err, TransportError::Io(_)),
            "typed I/O error, got {err:?}"
        );
    }

    #[test]
    fn detect_board_port_never_panics_without_hardware() {
        // This sandbox has no serial ports, so detection finds nothing
        // — but the assertion that matters is that enumeration +
        // ranking + dead-port skipping completes without panicking.
        // (On a machine with a real board attached this returns its
        // port instead; that path is a carried gap for the final
        // release gate.)
        let _found = detect_board_port();
    }

    #[test]
    fn counters_snapshot_sums_drops() {
        let counters = AtomicCounters::default();
        counters.dropped_bad_magic.store(3, Ordering::Relaxed);
        counters.dropped_bad_crc.store(1, Ordering::Relaxed);
        counters.dropped_bad_length.store(2, Ordering::Relaxed);
        counters.dropped_malformed.store(1, Ordering::Relaxed);
        counters.dropped_overflow.store(1, Ordering::Relaxed);
        counters.dropped_unexpected.store(2, Ordering::Relaxed);
        assert_eq!(counters.snapshot().frames_dropped, 10);
    }
}
