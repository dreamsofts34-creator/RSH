//! `rsh-transport`: the UDP transport (`HELO` 1 Hz, stream receive, control
//! client with the sender-address filter) and the wired USB-serial transport
//! (`RSHS` framing, 921600 baud), plus `GET_CAPS`, the config-transaction
//! client, and mDNS discovery with the `192.168.4.1` fallback.
//!
//! Both transports speak the same protocol v2.2 — the engine cannot tell
//! which one is active. They sit behind the [`ControlTransport`] trait (for
//! the unsigned control channel) and the engine's `PacketSource` trait (for the
//! 200 Hz stream).

use std::time::Duration;

pub mod config;
pub mod control_client;
pub mod discovery;
pub mod port_conflict;
pub mod serial;
pub mod session;
pub mod udp;

/// Error from a transport operation.
#[derive(Debug)]
pub enum TransportError {
    /// Underlying I/O error.
    Io(std::io::Error),
    /// No reply within the requested timeout.
    Timeout,
    /// The transport is closed / the peer went away.
    Closed,
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "transport I/O error: {e}"),
            Self::Timeout => write!(f, "transport timeout"),
            Self::Closed => write!(f, "transport closed"),
        }
    }
}

impl std::error::Error for TransportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Timeout | Self::Closed => None,
        }
    }
}

impl From<std::io::Error> for TransportError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Byte-level transport for the unsigned control channel (port 4212 over UDP,
/// `RSHS` frames over serial). Implemented by [`udp::UdpTransport`] (Wave 1)
/// and the serial transport (Wave 2, Phase C).
///
/// Async with desugared `impl Future` returns (same pattern as the engine's
/// `PacketSource`): `async fn` in a trait is not `dyn`-compatible on stable
/// Rust. Clients are therefore generic over the transport type
/// (`ControlClient<T: ControlTransport>`), never `&mut dyn`.
pub trait ControlTransport: Send {
    /// Send one raw control frame.
    fn send_control(
        &mut self,
        bytes: &[u8],
    ) -> impl Future<Output = Result<(), TransportError>> + Send;
    /// Receive one raw control reply, waiting at most `timeout`.
    fn recv_control(
        &mut self,
        timeout: Duration,
    ) -> impl Future<Output = Result<Vec<u8>, TransportError>> + Send;
}
