use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct TransportUtils;
use crate::TransportError;

type Resolved = Mutex<Option<HashMap<String, (SocketAddr, Option<Instant>)>>>;
/// Answers of host-name lookups, by `host:port` as passed to `to_socket_addrs`.
static RESOLVED: Resolved = Mutex::new(None);

impl TransportUtils {
    /// Parses a string endpoint into a [SocketAddr]. If no port is specified, port 0 is used.
    /// Returns [TransportError::InvalidAddress] if the address is invalid or cannot be resolved.
    ///
    /// Accepts `host`, `host:port`, an IPv4 or IPv6 literal with or without a port, and a
    /// bracketed IPv6 literal. Note that a bare IPv6 literal cannot carry a port, because the
    /// colons are ambiguous - `[::1]:445` is the way to give one.
    pub fn parse_socket_address(endpoint: &str) -> super::error::Result<SocketAddr> {
        let invalid = || TransportError::InvalidAddress(endpoint.to_string());

        // A complete socket address: `1.2.3.4:445` or `[::1]:445`.
        if let Ok(address) = endpoint.parse::<SocketAddr>() {
            return Ok(address);
        }

        // A bare IP literal with no port: `1.2.3.4` or `::1`.
        if let Ok(ip) = endpoint.parse::<IpAddr>() {
            return Ok(SocketAddr::new(ip, 0));
        }

        // A bracketed IPv6 literal with no port: `[::1]`.
        if let Some(inner) = endpoint.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            return inner
                .parse::<Ipv6Addr>()
                .map(|ip| SocketAddr::new(IpAddr::V6(ip), 0))
                .map_err(|_| invalid());
        }

        // Anything else is a host name, with or without a port. Only a trailing
        // `:digits` counts as a port, so a name is never split on the wrong colon.
        let has_port = endpoint.rsplit_once(':').is_some_and(|(host, port)| {
            !host.is_empty() && !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())
        });
        let endpoint = if has_port {
            endpoint.to_owned()
        } else {
            format!("{endpoint}:0")
        };

        Self::resolve(&endpoint, |e| e.to_socket_addrs().ok()?.next()).ok_or_else(invalid)
    }

    /// Resolves a host name, retrying: mDNS names (`nas.local`) sometimes fail
    /// to resolve for a moment on a busy Wi-Fi link. If it still fails, the
    /// address that last worked for the name in this process is used. An
    /// answer is also reused for ten minutes (until [`TransportUtils::forget`])
    /// without asking again: an mDNS lookup takes 350-400 ms on the Frame
    /// (more, or nothing, when it is busy), and one connection asks twice.
    fn resolve(
        endpoint: &str,
        lookup: impl Fn(&str) -> Option<SocketAddr>,
    ) -> Option<SocketAddr> {
        Self::resolve_within(endpoint, lookup, Duration::from_secs(600))
    }

    fn resolve_within(
        endpoint: &str,
        lookup: impl Fn(&str) -> Option<SocketAddr>,
        fresh: Duration,
    ) -> Option<SocketAddr> {
        const ATTEMPTS: u32 = 3;
        let known = || RESOLVED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((address, at)) = known().as_ref().and_then(|m| m.get(endpoint)).copied()
            && at.is_some_and(|at| at.elapsed() < fresh)
        {
            return Some(address);
        }
        for attempt in 1..=ATTEMPTS {
            if let Some(address) = lookup(endpoint) {
                known()
                    .get_or_insert_with(HashMap::new)
                    .insert(endpoint.to_owned(), (address, Some(Instant::now())));
                return Some(address);
            }
            if attempt < ATTEMPTS {
                std::thread::sleep(Duration::from_millis(100 * attempt as u64));
            }
        }
        known().as_ref()?.get(endpoint).map(|(address, _)| *address)
    }

    /// Stops trusting what is remembered for `host` (with or without a port),
    /// e.g. after a connection to it failed: the next lookup asks again. The
    /// old address stays as the answer if that lookup fails.
    pub fn forget(host: &str) {
        let has_port = host
            .rsplit_once(':')
            .is_some_and(|(_, p)| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
        let key = if has_port {
            host.to_owned()
        } else {
            format!("{host}:0")
        };
        let mut known = RESOLVED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = known.as_mut().and_then(|m| m.get_mut(&key)) {
            entry.1 = None;
        }
    }

    /// Resolves `host` (with or without a port) ahead of the first connection,
    /// so that one doesn't wait for the lookup. Failures are ignored.
    pub fn prefetch(host: &str) {
        let _ = Self::parse_socket_address(host);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddrV4, SocketAddrV6};

    #[test]
    fn a_name_that_stops_resolving_keeps_its_last_address() {
        use std::cell::Cell;
        let address: SocketAddr = "10.0.0.5:445".parse().unwrap();
        let calls = Cell::new(0);
        // Fails once, then resolves: retried.
        let flaky = |_: &str| {
            calls.set(calls.get() + 1);
            (calls.get() > 1).then_some(address)
        };
        let never_fresh = Duration::ZERO;
        assert_eq!(
            TransportUtils::resolve_within("flaky-nas.local:445", flaky, never_fresh),
            Some(address)
        );
        assert_eq!(calls.get(), 2);
        // Then fails every time: the address that worked.
        calls.set(0);
        let down = |_: &str| {
            calls.set(calls.get() + 1);
            None
        };
        assert_eq!(
            TransportUtils::resolve_within("flaky-nas.local:445", down, never_fresh),
            Some(address)
        );
        assert_eq!(calls.get(), 3);
        // Never resolved: nothing.
        assert_eq!(TransportUtils::resolve("never.local:445", |_| None), None);
    }

    #[test]
    fn a_fresh_answer_is_not_asked_for_again() {
        use std::cell::Cell;
        let address: SocketAddr = "10.0.0.6:445".parse().unwrap();
        let calls = Cell::new(0);
        let lookup = |_: &str| {
            calls.set(calls.get() + 1);
            Some(address)
        };
        let minute = Duration::from_secs(60);
        for _ in 0..3 {
            assert_eq!(
                TransportUtils::resolve_within("fresh-nas.local:445", lookup, minute),
                Some(address)
            );
        }
        assert_eq!(calls.get(), 1);
        // Too old to trust: asked again.
        TransportUtils::resolve_within("fresh-nas.local:445", lookup, Duration::ZERO);
        assert_eq!(calls.get(), 2);
    }

    fn parse(endpoint: &str) -> SocketAddr {
        TransportUtils::parse_socket_address(endpoint)
            .unwrap_or_else(|e| panic!("{endpoint:?} should parse: {e}"))
    }

    #[test]
    fn parses_ipv4_with_and_without_a_port() {
        assert_eq!(
            parse("1.2.3.4:445"),
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(1, 2, 3, 4), 445))
        );
        assert_eq!(
            parse("1.2.3.4"),
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(1, 2, 3, 4), 0))
        );
    }

    #[test]
    fn parses_ipv6_in_every_spelling() {
        let loopback = Ipv6Addr::LOCALHOST;

        // Bracketed, with a port: the only form that can carry one.
        assert_eq!(
            parse("[::1]:445"),
            SocketAddr::V6(SocketAddrV6::new(loopback, 445, 0, 0))
        );
        // Bracketed, without a port.
        assert_eq!(
            parse("[::1]"),
            SocketAddr::V6(SocketAddrV6::new(loopback, 0, 0, 0))
        );
        // Bare. Previously this was split on the last colon and read as host
        // "::" with port 1.
        assert_eq!(
            parse("::1"),
            SocketAddr::V6(SocketAddrV6::new(loopback, 0, 0, 0))
        );
        assert_eq!(
            parse("2001:db8::1"),
            SocketAddr::V6(SocketAddrV6::new("2001:db8::1".parse().unwrap(), 0, 0, 0))
        );
        // A full-length literal, whose last group is all digits and so looks
        // most like a port.
        assert_eq!(
            parse("2001:db8:0:0:0:0:0:1"),
            SocketAddr::V6(SocketAddrV6::new("2001:db8::1".parse().unwrap(), 0, 0, 0))
        );
    }

    #[test]
    fn parses_host_names() {
        assert_eq!(parse("localhost:445").port(), 445);
        assert_eq!(parse("localhost").port(), 0);
    }

    #[test]
    fn rejects_malformed_endpoints() {
        for endpoint in ["[::1", "[not-an-address]", "[::1]:notaport"] {
            assert!(
                TransportUtils::parse_socket_address(endpoint).is_err(),
                "{endpoint:?} should be rejected"
            );
        }
    }
}
