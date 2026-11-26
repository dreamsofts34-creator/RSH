//! Board discovery: mDNS first, configured-IP fallback (Phase C1 / SW-R5.3,
//! D33).
//!
//! [`Discovery::resolve_board`] tries mDNS (`rsh.local`, via
//! `mdns-sd`'s [`ServiceDaemon::resolve_hostname`](mdns_sd::ServiceDaemon::resolve_hostname))
//! first and falls back to the configured IP (default `192.168.4.1`)
//! when mDNS finds nothing, errors, or times out. A failed mDNS lookup
//! never blocks the fallback: the query is bounded by
//! [`DEFAULT_MDNS_TIMEOUT`], and any error resolves to the fallback
//! immediately.
//!
//! Successful mDNS answers are cached briefly ([`DEFAULT_CACHE_TTL`]);
//! fallback answers are not cached, so a board that appears later is
//! found on the next resolve.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use mdns_sd::{HostnameResolutionEvent, ScopedIp, ServiceDaemon};

/// Default board IP when mDNS finds nothing (D33).
pub const DEFAULT_BOARD_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 1);
/// mDNS hostname of the board.
pub const MDNS_HOSTNAME: &str = "rsh.local";
/// Default time to wait for an mDNS answer before falling back.
pub const DEFAULT_MDNS_TIMEOUT: Duration = Duration::from_secs(2);
/// How long a successful mDNS answer is cached.
pub const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(30);

/// Board discovery: mDNS `rsh.local` first, configured IP fallback.
///
/// Cheap to construct and safe to share across threads; resolution is
/// synchronous (the mDNS query blocks up to the configured timeout) and
/// must therefore not run on the engine's 200 Hz tick — resolve once at
/// connect time and hand the address to the transport.
#[derive(Debug)]
pub struct Discovery {
    /// IP returned when mDNS finds nothing (default [`DEFAULT_BOARD_IP`]).
    fallback_ip: IpAddr,
    /// Time to wait for an mDNS answer before falling back.
    mdns_timeout: Duration,
    /// How long a successful mDNS answer is cached.
    cache_ttl: Duration,
    /// Cached mDNS answer (`None` = nothing cached, or cache expired).
    cache: Mutex<Option<(IpAddr, Instant)>>,
}

impl Discovery {
    /// Build discovery with `fallback_ip` as the mDNS fallback.
    #[must_use]
    pub fn new(fallback_ip: IpAddr) -> Self {
        Self {
            fallback_ip,
            mdns_timeout: DEFAULT_MDNS_TIMEOUT,
            cache_ttl: DEFAULT_CACHE_TTL,
            cache: Mutex::new(None),
        }
    }

    /// Set the mDNS timeout (builder; default [`DEFAULT_MDNS_TIMEOUT`]).
    #[must_use]
    pub fn with_mdns_timeout(mut self, timeout: Duration) -> Self {
        self.mdns_timeout = timeout;
        self
    }

    /// Set the mDNS cache TTL (builder; default [`DEFAULT_CACHE_TTL`]).
    #[must_use]
    pub fn with_cache_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = ttl;
        self
    }

    /// The configured fallback IP.
    #[must_use]
    pub fn fallback_ip(&self) -> IpAddr {
        self.fallback_ip
    }

    /// The configured mDNS timeout.
    #[must_use]
    pub fn mdns_timeout(&self) -> Duration {
        self.mdns_timeout
    }

    /// Resolve the board address: mDNS `rsh.local` first, then the
    /// configured fallback IP. Never fails — a failed or timed-out mDNS
    /// lookup returns the fallback, and a successful mDNS answer is
    /// cached for [`DEFAULT_CACHE_TTL`].
    #[must_use]
    pub fn resolve_board(&self) -> IpAddr {
        if let Some(cached) = self.cached_mdns() {
            return cached;
        }
        match self.resolve_mdns() {
            Some(ip) => {
                self.store_cache(ip);
                ip
            }
            None => self.fallback_ip,
        }
    }

    /// The cached mDNS answer, if one is still fresh. An expired
    /// entry is dropped (L5: the next `resolve_board` re-resolves
    /// instead of serving a stale address — the cache never outlives
    /// its TTL).
    fn cached_mdns(&self) -> Option<IpAddr> {
        let mut guard = self.cache.lock().ok()?;
        let (ip, at) = (*guard)?;
        if at.elapsed() < self.cache_ttl {
            Some(ip)
        } else {
            *guard = None;
            None
        }
    }

    /// Remember a successful mDNS answer.
    fn store_cache(&self, ip: IpAddr) {
        if let Ok(mut guard) = self.cache.lock() {
            *guard = Some((ip, Instant::now()));
        }
    }

    /// One bounded mDNS query for `rsh.local`. Returns [`None`] when the
    /// daemon cannot start, the lookup errors, times out, or finds nothing.
    ///
    /// `mdns-sd` 0.19's `resolve_hostname` takes a timeout in
    /// milliseconds and returns an event receiver; the answer arrives as
    /// an `AddressesFound` event (or `SearchTimeout` when nothing
    /// answers). The wait is capped at the configured timeout plus a
    /// small grace margin, so a misbehaving daemon can never block the
    /// fallback.
    fn resolve_mdns(&self) -> Option<IpAddr> {
        // PERF P3-5 (polish round): one long-lived `ServiceDaemon`
        // for the process instead of constructing (and shutting down)
        // one per cache miss — the construction cost was paid on every
        // first connect / post-TTL resolve. The daemon is never shut
        // down now; only the per-query resolve is stopped.
        let daemon = long_lived_mdns_daemon()?;
        let timeout_ms = u64::try_from(self.mdns_timeout.as_millis()).ok()?;
        let receiver = daemon
            .resolve_hostname(MDNS_HOSTNAME, Some(timeout_ms))
            .ok()?;
        let deadline = Instant::now() + self.mdns_timeout + Duration::from_millis(500);
        let mut found = None;
        while found.is_none() && Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(remaining) {
                Ok(HostnameResolutionEvent::AddressesFound(_, addresses)) => {
                    found = pick_board_ip(&addresses);
                }
                Ok(
                    HostnameResolutionEvent::SearchTimeout(_)
                    | HostnameResolutionEvent::SearchStopped(_),
                )
                | Err(_) => break,
                Ok(_) => {}
            }
        }
        let _ = daemon.stop_resolve_hostname(MDNS_HOSTNAME);
        found
    }
}

/// The process-wide mDNS daemon (PERF P3-5): created once, reused
/// across resolves. `ServiceDaemon` is a cheap cloneable handle, so
/// the static holds one and hands out clones.
fn long_lived_mdns_daemon() -> Option<ServiceDaemon> {
    static DAEMON: std::sync::OnceLock<ServiceDaemon> = std::sync::OnceLock::new();
    if let Some(daemon) = DAEMON.get() {
        return Some(daemon.clone());
    }
    let daemon = ServiceDaemon::new().ok()?;
    let _ = DAEMON.set(daemon.clone());
    Some(daemon)
}

/// Prefer an IPv4 answer (the board is IPv4); otherwise take any address.
///
/// L5: the answer set is a `HashSet`, whose iteration order is
/// nondeterministic — the pick is sorted (IPv4 first, then by
/// address) so the same mDNS answer always resolves to the same IP
/// instead of a different one per run.
fn pick_board_ip(addresses: &HashSet<ScopedIp>) -> Option<IpAddr> {
    let mut sorted: Vec<IpAddr> = addresses.iter().map(ScopedIp::to_ip_addr).collect();
    sorted.sort_by_key(|ip| (!ip.is_ipv4(), *ip));
    sorted.into_iter().next()
}

impl Default for Discovery {
    /// Default discovery: fallback [`DEFAULT_BOARD_IP`], 2 s mDNS timeout.
    fn default() -> Self {
        Self::new(IpAddr::V4(DEFAULT_BOARD_IP))
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    #[test]
    fn discovery_is_send_and_sync() {
        /// Compile-time assertion: discovery must be shareable.
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Discovery>();
    }

    #[test]
    fn fallback_when_mdns_finds_nothing() {
        let discovery = Discovery::new(IpAddr::V4(DEFAULT_BOARD_IP))
            .with_mdns_timeout(Duration::from_millis(100));
        assert_eq!(discovery.resolve_board(), IpAddr::V4(DEFAULT_BOARD_IP));
    }

    #[test]
    fn configured_ip_is_honoured() {
        let configured = IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3));
        let discovery = Discovery::new(configured).with_mdns_timeout(Duration::from_millis(50));
        assert_eq!(discovery.resolve_board(), configured);
        assert_eq!(discovery.fallback_ip(), configured);
    }

    #[test]
    fn mdns_timeout_is_bounded() {
        let discovery = Discovery::default().with_mdns_timeout(Duration::from_millis(100));
        let started = Instant::now();
        let resolved = discovery.resolve_board();
        assert_eq!(resolved, IpAddr::V4(DEFAULT_BOARD_IP));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "resolve took {:?} with a 100 ms mDNS timeout",
            started.elapsed()
        );
    }

    #[test]
    fn pick_board_ip_prefers_ipv4_and_sorts_deterministically() {
        // L5: same answer set, same pick, every run — IPv4 first,
        // then lowest address.
        use std::net::Ipv6Addr;
        let mut addresses = HashSet::new();
        addresses.insert(ScopedIp::from(IpAddr::V6(Ipv6Addr::new(
            0xfe80, 0, 0, 0, 0, 0, 0, 1,
        ))));
        addresses.insert(ScopedIp::from(IpAddr::V4(Ipv4Addr::new(192, 168, 4, 2))));
        addresses.insert(ScopedIp::from(IpAddr::V4(Ipv4Addr::new(192, 168, 4, 1))));
        for _ in 0..10 {
            assert_eq!(
                pick_board_ip(&addresses),
                Some(IpAddr::V4(Ipv4Addr::new(192, 168, 4, 1)))
            );
        }
        // IPv6 only: lowest IPv6 wins, deterministically.
        let mut v6only = HashSet::new();
        v6only.insert(ScopedIp::from(IpAddr::V6(Ipv6Addr::new(
            0xfe80, 0, 0, 0, 0, 0, 0, 2,
        ))));
        v6only.insert(ScopedIp::from(IpAddr::V6(Ipv6Addr::new(
            0xfe80, 0, 0, 0, 0, 0, 0, 1,
        ))));
        assert_eq!(
            pick_board_ip(&v6only),
            Some(IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)))
        );
        assert_eq!(pick_board_ip(&HashSet::new()), None);
    }

    #[test]
    fn expired_cache_is_dropped_and_re_resolves() {
        // L5: after the TTL the cached answer is gone — the next
        // resolve re-queries instead of serving the stale IP.
        let discovery =
            Discovery::new(IpAddr::V4(DEFAULT_BOARD_IP)).with_cache_ttl(Duration::from_millis(10));
        discovery.store_cache(IpAddr::V4(Ipv4Addr::new(192, 168, 4, 9)));
        assert_eq!(
            discovery.cached_mdns(),
            Some(IpAddr::V4(Ipv4Addr::new(192, 168, 4, 9)))
        );
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(discovery.cached_mdns(), None);
    }
}
