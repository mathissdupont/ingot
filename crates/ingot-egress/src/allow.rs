//! Deciding whether one destination is inside the policy.
//!
//! Split from the proxy so the decision can be tested without a socket, and so
//! that reading "what does this allow" does not mean reading an accept loop.

use std::net::IpAddr;

/// Why a destination was refused.
///
/// Distinguished rather than collapsed into a boolean, because an operator
/// looking at a blocked request needs to know whether they wrote the wrong host
/// or whether something is trying to get out sideways. The two call for
/// completely different responses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The host is not in the policy's list.
    NotListed { host: String },
    /// The request named an address rather than a host.
    ///
    /// A policy grants host names ([Language 0.1 §7.1]), so an address can
    /// never be in the list — but refusing it *by name* matters, because
    /// dialling by address is how a client asks to skip name-based filtering.
    ///
    /// [Language 0.1 §7.1]: ../../../specs/language/v0.1.md
    AddressLiteral { address: String },
    /// The host resolved, and every address it resolved to is one a container
    /// has no business reaching: loopback, link-local, or a private range.
    ///
    /// The classic use of an allowed name pointing somewhere it should not.
    NotGlobal { host: String, address: IpAddr },
    /// The host does not resolve.
    Unresolvable { host: String },
    /// The request was not one this proxy understands.
    Malformed { reason: &'static str },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::NotListed { host } => {
                write!(f, "`{host}` is not a host this agent's policy grants")
            }
            Refusal::AddressLiteral { address } => write!(
                f,
                "`{address}` is an address rather than a host name, and a policy grants names"
            ),
            Refusal::NotGlobal { host, address } => write!(
                f,
                "`{host}` resolves to {address}, which is not a public address"
            ),
            Refusal::Unresolvable { host } => write!(f, "`{host}` does not resolve"),
            Refusal::Malformed { reason } => write!(f, "the request was not usable: {reason}"),
        }
    }
}

/// The hosts a contained server may reach.
#[derive(Debug, Clone, Default)]
pub struct Allowlist {
    hosts: Vec<String>,
    /// Whether a host may resolve to a loopback or private address.
    ///
    /// Off everywhere except in this crate's own tests, which necessarily talk
    /// to `127.0.0.1`. A container reaching the host's own loopback is the
    /// thing a boundary exists to prevent, so this is not a knob an artifact or
    /// a policy can reach.
    allow_private: bool,
}

impl Allowlist {
    /// Build from the hosts a policy named.
    ///
    /// Lower-cased and deduplicated here, because DNS is case-insensitive and a
    /// comparison that is not would refuse `ArXiv.org` while allowing
    /// `arxiv.org` — a difference with no meaning behind it.
    pub fn new<I, S>(hosts: I) -> Allowlist
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut hosts: Vec<String> = hosts
            .into_iter()
            .map(|host| host.as_ref().trim().to_ascii_lowercase())
            .filter(|host| !host.is_empty())
            .collect();
        hosts.sort();
        hosts.dedup();
        Allowlist {
            hosts,
            allow_private: false,
        }
    }

    /// Permit a host that resolves to a loopback or private address.
    ///
    /// For this crate's tests, which have nowhere else to connect to.
    #[doc(hidden)]
    pub fn allowing_private_addresses(mut self) -> Allowlist {
        self.allow_private = true;
        self
    }

    pub fn hosts(&self) -> &[String] {
        &self.hosts
    }

    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }

    /// Whether the policy names this host.
    ///
    /// Exact match, because [Language 0.1 §7.1] says a host is matched exactly
    /// and there are no wildcards. A trailing dot is stripped first: `arxiv.org.`
    /// is the same name written fully qualified, and treating it as a different
    /// one would be a bypass rather than a nicety.
    ///
    /// [Language 0.1 §7.1]: ../../../specs/language/v0.1.md
    pub fn names(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        self.hosts.contains(&host)
    }

    /// Whether this destination may be dialled, and if not, why.
    ///
    /// The check and the connection must agree, which is why this returns the
    /// addresses it resolved rather than a verdict alone: the caller dials
    /// exactly what was checked. Resolving twice is how a name that changes
    /// between the two — DNS rebinding — gets through a filter that looks
    /// correct.
    pub fn resolve(&self, host: &str, port: u16) -> Result<Vec<std::net::SocketAddr>, Refusal> {
        if host.parse::<IpAddr>().is_ok() || (host.starts_with('[') && host.ends_with(']')) {
            return Err(Refusal::AddressLiteral {
                address: host.to_string(),
            });
        }
        if !self.names(host) {
            return Err(Refusal::NotListed {
                host: host.to_string(),
            });
        }

        use std::net::ToSocketAddrs;
        let addresses: Vec<std::net::SocketAddr> = (host, port)
            .to_socket_addrs()
            .map_err(|_| Refusal::Unresolvable {
                host: host.to_string(),
            })?
            .collect();
        if addresses.is_empty() {
            return Err(Refusal::Unresolvable {
                host: host.to_string(),
            });
        }

        if !self.allow_private {
            // Every address, not just the first: a name that resolves to one
            // public and one loopback address would otherwise be reachable by
            // whichever the connect attempt happened to pick.
            for address in &addresses {
                if !is_global(address.ip()) {
                    return Err(Refusal::NotGlobal {
                        host: host.to_string(),
                        address: address.ip(),
                    });
                }
            }
        }
        Ok(addresses)
    }
}

/// Whether an address is one the public internet routes to.
///
/// Hand-written because the standard library's equivalents are unstable, and
/// because being explicit about which ranges are excluded is worth more here
/// than brevity: this list is the difference between a container that can reach
/// the host's own services and one that cannot.
fn is_global(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
                // 100.64.0.0/10, carrier-grade NAT.
                || (a == 100 && (64..128).contains(&b))
                // 192.0.0.0/24, IETF protocol assignments.
                || (a == 192 && b == 0 && v4.octets()[2] == 0)
                // 198.18.0.0/15, benchmarking.
                || (a == 198 && (18..20).contains(&b))
                // 240.0.0.0/4, reserved.
                || a >= 240)
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // fc00::/7, unique local.
                || (first & 0xfe00) == 0xfc00
                // fe80::/10, link-local.
                || (first & 0xffc0) == 0xfe80
                // Anything mapped from a non-global v4 address.
                || v6.to_ipv4_mapped().is_some_and(|v4| !is_global(v4.into())))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> Allowlist {
        Allowlist::new(["arxiv.org", "GitHub.com"])
    }

    #[test]
    fn a_host_is_matched_exactly_and_case_insensitively() {
        // DNS is case-insensitive, so a comparison that is not would refuse
        // `ArXiv.org` for no reason anybody could act on.
        assert!(list().names("arxiv.org"));
        assert!(list().names("ARXIV.ORG"));
        assert!(list().names("github.com"));
        assert!(!list().names("evil.org"));
    }

    #[test]
    fn a_subdomain_is_not_the_host() {
        // Language 0.1 has no wildcards, and inventing one here would grant
        // reach the source never asked for.
        assert!(!list().names("files.arxiv.org"));
        assert!(!list().names("arxiv.org.evil.test"));
    }

    #[test]
    fn a_trailing_dot_is_the_same_name() {
        // Fully qualified, and treating it as different would be a bypass
        // rather than a nicety.
        assert!(list().names("arxiv.org."));
    }

    #[test]
    fn an_address_literal_is_refused_by_name() {
        // Dialling by address is how a client asks to skip name-based
        // filtering, so it is refused as its own thing rather than falling
        // through to "not listed".
        for literal in ["93.184.216.34", "127.0.0.1", "::1"] {
            let refusal = list().resolve(literal, 443).unwrap_err();
            assert!(
                matches!(refusal, Refusal::AddressLiteral { .. }),
                "{literal}: {refusal:?}"
            );
        }
    }

    #[test]
    fn a_host_the_policy_does_not_name_is_refused_before_it_is_resolved() {
        let refusal = list().resolve("evil.test", 443).unwrap_err();
        assert!(matches!(refusal, Refusal::NotListed { .. }), "{refusal:?}");
    }

    #[test]
    fn an_allowed_name_pointing_at_loopback_is_refused() {
        // The classic shape: the policy names something reasonable and DNS
        // sends it at the host's own services.
        let refusal = Allowlist::new(["localhost"])
            .resolve("localhost", 80)
            .unwrap_err();
        assert!(matches!(refusal, Refusal::NotGlobal { .. }), "{refusal:?}");
    }

    #[test]
    fn private_ranges_are_not_global() {
        for address in [
            "127.0.0.1",
            "10.0.0.1",
            "192.168.1.1",
            "172.16.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fe80::1",
            "fc00::1",
            "::ffff:127.0.0.1",
        ] {
            let ip: IpAddr = address.parse().expect(address);
            assert!(!is_global(ip), "{address} must not be treated as public");
        }
    }

    #[test]
    fn public_addresses_are_global() {
        for address in ["93.184.216.34", "1.1.1.1", "2606:4700::1111"] {
            let ip: IpAddr = address.parse().expect(address);
            assert!(is_global(ip), "{address} must be treated as public");
        }
    }

    #[test]
    fn the_link_local_metadata_address_is_refused() {
        // 169.254.169.254 is the cloud metadata endpoint, and reaching it from
        // a tool container is how a boundary leaks credentials.
        let ip: IpAddr = "169.254.169.254".parse().unwrap();
        assert!(!is_global(ip));
    }

    #[test]
    fn an_empty_list_reaches_nothing() {
        let empty = Allowlist::new(Vec::<String>::new());
        assert!(empty.is_empty());
        assert!(matches!(
            empty.resolve("arxiv.org", 443).unwrap_err(),
            Refusal::NotListed { .. }
        ));
    }
}
