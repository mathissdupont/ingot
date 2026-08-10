//! The egress proxy: what makes `network allow ["arxiv.org"]` true.
//!
//! A policy has always been able to name hosts, and until this crate nothing
//! bounded a contained tool server to them
//! ([GAP-001](../../../docs/gaps.md#gap-001)). The boundary could give a server
//! a network or withhold one, and it was giving it one.
//!
//! This is the component every contained server's traffic leaves through. It
//! speaks the forward-proxy shape a client already knows — `CONNECT` for TLS,
//! an absolute-URI request line for plain HTTP — and refuses anything the
//! policy does not name.
//!
//! # What makes it correct rather than plausible
//!
//! A host filter is easy to write and easy to write wrongly, and a boundary
//! that is trusted and wrong is worse than one that never existed. Four
//! specific failures, and what stops each:
//!
//! * **DNS rebinding.** The classic bypass is a name that resolves to something
//!   safe when checked and something else when dialled. Here the client never
//!   resolves anything: it hands over a name, and the proxy resolves it once
//!   and dials one of *those* addresses. The check and the connection cannot
//!   disagree because there is only one resolution.
//! * **Address literals.** `CONNECT 93.184.216.34:443` asks to skip name-based
//!   filtering entirely. A policy grants names, so an address is never in the
//!   list — and it is refused as its own thing so the log says which mistake
//!   was made.
//! * **TLS SNI against the Host header.** The proxy does not read either, and
//!   does not need to. A `CONNECT` tunnel carries bytes to the address the
//!   *proxy* dialled, so whatever name the client puts in its SNI or its Host
//!   header, the packets go to a host the policy granted. No TLS is terminated
//!   and no certificate authority is involved.
//! * **An allowed name pointing inward.** A granted host that resolves to
//!   loopback, a private range, or the cloud metadata address would let a
//!   contained server reach the machine running it. Every resolved address is
//!   checked, not just the one that gets used.
//!
//! # What it is not
//!
//! Not a caching proxy, not a TLS-terminating proxy, and not a general one. It
//! bounds a destination and nothing else: it does not read a body, rewrite a
//! header, or look at a path. A policy names hosts, so a host is all this may
//! decide on.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

mod allow;

pub use allow::{Allowlist, Refusal};

/// How long a connection may take to say what it wants.
///
/// A client that opens a socket and says nothing holds a thread; short enough
/// that it cannot be used to exhaust the proxy, long enough that a slow network
/// is not mistaken for one.
const HEADER_TIMEOUT: Duration = Duration::from_secs(30);

/// How long dialling the destination may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// The longest request line and header block this will read.
///
/// Bounded because the alternative is a client that sends header bytes forever
/// and a proxy that buffers them forever.
const MAX_HEADER_BYTES: usize = 32 * 1024;

/// What the proxy did with one connection, for the run log.
///
/// Carries the host and the verdict and never the bytes: a proxy that logged
/// traffic would be a place secrets accumulate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allowed { host: String, port: u16 },
    Refused { refusal: Refusal },
}

impl std::fmt::Display for Decision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Decision::Allowed { host, port } => write!(f, "allowed {host}:{port}"),
            Decision::Refused { refusal } => write!(f, "refused: {refusal}"),
        }
    }
}

/// A running proxy.
pub struct Proxy {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Proxy {
    /// Start on `bind`, filtering against `allow`.
    ///
    /// `on_decision` is called for every connection, allowed or refused, so an
    /// operator can see what a contained server reached for. It must not block:
    /// it runs on the connection's own thread.
    pub fn start<F>(bind: SocketAddr, allow: Allowlist, on_decision: F) -> io::Result<Proxy>
    where
        F: Fn(Decision) + Send + Sync + 'static,
    {
        let listener = TcpListener::bind(bind)?;
        let address = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));

        let thread = {
            let stop = Arc::clone(&stop);
            let allow = Arc::new(allow);
            let on_decision = Arc::new(on_decision);
            thread::spawn(move || {
                for incoming in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                    let Ok(client) = incoming else { continue };
                    let allow = Arc::clone(&allow);
                    let on_decision = Arc::clone(&on_decision);
                    thread::spawn(move || {
                        if let Err(error) = serve(client, &allow, &*on_decision) {
                            // A client that hangs up mid-request is ordinary,
                            // not an event. Nothing here is worth failing the
                            // proxy over.
                            let _ = error;
                        }
                    });
                }
            })
        };

        Ok(Proxy {
            address,
            stop,
            thread: Some(thread),
        })
    }

    /// Where a client should point its `HTTP_PROXY`.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Stop accepting. Connections already open are left to finish.
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Unblock the accept loop by connecting to it once.
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// One connection, start to finish.
fn serve(client: TcpStream, allow: &Allowlist, on_decision: &dyn Fn(Decision)) -> io::Result<()> {
    client.set_read_timeout(Some(HEADER_TIMEOUT))?;
    let mut reader = BufReader::new(client.try_clone()?);

    let Some(request) = read_request(&mut reader)? else {
        return Ok(());
    };

    let (host, port) = match &request {
        Request::Connect { host, port } => (host.clone(), *port),
        Request::Absolute { host, port, .. } => (host.clone(), *port),
    };

    let addresses = match allow.resolve(&host, port) {
        Ok(addresses) => addresses,
        Err(refusal) => {
            on_decision(Decision::Refused {
                refusal: refusal.clone(),
            });
            return refuse(client, &refusal);
        }
    };

    let Some(upstream) = dial(&addresses) else {
        let refusal = Refusal::Unresolvable { host: host.clone() };
        on_decision(Decision::Refused {
            refusal: refusal.clone(),
        });
        return refuse(client, &refusal);
    };

    on_decision(Decision::Allowed {
        host: host.clone(),
        port,
    });

    match request {
        Request::Connect { .. } => {
            // 200 and then raw bytes. Nothing in the tunnel is read again: the
            // destination is already bounded by the address that was dialled,
            // and reading further would mean terminating TLS for no gain.
            let mut client = client;
            client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")?;
            client.flush()?;
            client.set_read_timeout(None)?;
            tunnel(client, upstream)
        }
        Request::Absolute { head, .. } => {
            let mut upstream = upstream;
            upstream.write_all(head.as_bytes())?;
            upstream.flush()?;
            // Whatever the client had buffered after the head is its body.
            let client = reader.into_inner();
            client.set_read_timeout(None)?;
            tunnel(client, upstream)
        }
    }
}

/// What a client asked for.
enum Request {
    /// `CONNECT host:port HTTP/1.1`
    Connect { host: String, port: u16 },
    /// `GET http://host/path HTTP/1.1`, rewritten to origin form for upstream.
    Absolute {
        host: String,
        port: u16,
        head: String,
    },
}

/// Read and understand the request line and headers.
///
/// Returns `None` when the client hung up without saying anything, which is an
/// ordinary way for a health check to behave.
fn read_request(reader: &mut BufReader<TcpStream>) -> io::Result<Option<Request>> {
    let mut head = String::new();
    let mut total = 0usize;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            return Ok(if head.is_empty() {
                None
            } else {
                Some(malformed("the request ended before its headers did"))
            });
        }
        total += read;
        if total > MAX_HEADER_BYTES {
            return Ok(Some(malformed("the headers are longer than this accepts")));
        }
        let blank = line.trim().is_empty();
        head.push_str(&line);
        if blank {
            break;
        }
    }

    let Some(request_line) = head.lines().next() else {
        return Ok(Some(malformed("there is no request line")));
    };
    let mut parts = request_line.split_whitespace();
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        return Ok(Some(malformed("the request line is not three fields")));
    };

    if method.eq_ignore_ascii_case("CONNECT") {
        let Some((host, port)) = split_authority(target, 443) else {
            return Ok(Some(malformed("the CONNECT target is not `host:port`")));
        };
        return Ok(Some(Request::Connect { host, port }));
    }

    // Plain HTTP through a proxy uses an absolute URI, which is where the host
    // comes from. The `Host` header is deliberately not consulted: two sources
    // for one fact is how a filter and a destination come to disagree.
    let Some(rest) = target
        .strip_prefix("http://")
        .or_else(|| target.strip_prefix("HTTP://"))
    else {
        return Ok(Some(malformed(
            "a proxied request needs an absolute `http://` target",
        )));
    };
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    let Some((host, port)) = split_authority(authority, 80) else {
        return Ok(Some(malformed("the target's authority is not usable")));
    };

    // Rewritten to origin form, because that is what an origin server expects.
    let mut rewritten = format!("{method} {path} {version}\r\n");
    for line in head.lines().skip(1) {
        if line.trim().is_empty() {
            continue;
        }
        // Hop-by-hop, and meaningless to the origin.
        let lowered = line.to_ascii_lowercase();
        if lowered.starts_with("proxy-connection:") || lowered.starts_with("proxy-authorization:") {
            continue;
        }
        rewritten.push_str(line);
        rewritten.push_str("\r\n");
    }
    rewritten.push_str("\r\n");

    Ok(Some(Request::Absolute {
        host,
        port,
        head: rewritten,
    }))
}

fn malformed(reason: &'static str) -> Request {
    // Carried as a CONNECT to a host no list can name, so one refusal path
    // serves every way a request can be wrong.
    let _ = reason;
    Request::Connect {
        host: String::new(),
        port: 0,
    }
}

/// `host:port`, or `host` with a default. Handles a bracketed IPv6 literal, so
/// that it reaches the address-literal refusal rather than a parse failure.
fn split_authority(authority: &str, default_port: u16) -> Option<(String, u16)> {
    let authority = authority.trim();
    if authority.is_empty() {
        return None;
    }
    if let Some(end) = authority
        .strip_prefix('[')
        .and_then(|_| authority.find(']'))
    {
        let host = &authority[..=end];
        let port = match &authority[end + 1..] {
            "" => default_port,
            rest => rest.strip_prefix(':')?.parse().ok()?,
        };
        return Some((host.to_string(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => Some((host.to_string(), port.parse().ok()?)),
        Some(_) => None,
        None => Some((authority.to_string(), default_port)),
    }
}

/// Dial the first address that answers, of exactly those that were checked.
fn dial(addresses: &[SocketAddr]) -> Option<TcpStream> {
    for address in addresses {
        if let Ok(stream) = TcpStream::connect_timeout(address, CONNECT_TIMEOUT) {
            return Some(stream);
        }
    }
    None
}

/// Tell the client no, in terms it can render.
///
/// `403` rather than a dropped connection: a tool server that is refused should
/// report being refused, not time out and be debugged as a network fault.
fn refuse(mut client: TcpStream, refusal: &Refusal) -> io::Result<()> {
    let body = format!("ingot egress: {refusal}\n");
    let response = format!(
        "HTTP/1.1 403 Forbidden\r\n\
         content-type: text/plain\r\n\
         content-length: {}\r\n\
         connection: close\r\n\r\n{body}",
        body.len()
    );
    client.write_all(response.as_bytes())?;
    client.flush()?;
    let _ = client.shutdown(Shutdown::Both);
    Ok(())
}

/// Copy bytes both ways until either side is done.
fn tunnel(client: TcpStream, upstream: TcpStream) -> io::Result<()> {
    let (mut client_read, mut client_write) = (client.try_clone()?, client);
    let (mut upstream_read, mut upstream_write) = (upstream.try_clone()?, upstream);

    let outbound = thread::spawn(move || {
        let _ = io::copy(&mut client_read, &mut upstream_write);
        let _ = upstream_write.shutdown(Shutdown::Write);
    });
    let _ = copy_all(&mut upstream_read, &mut client_write);
    let _ = client_write.shutdown(Shutdown::Write);
    let _ = outbound.join();
    Ok(())
}

fn copy_all(from: &mut impl Read, to: &mut impl Write) -> io::Result<()> {
    io::copy(from, to).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_authority_splits_into_a_host_and_a_port() {
        assert_eq!(
            split_authority("arxiv.org:443", 80),
            Some(("arxiv.org".into(), 443))
        );
        assert_eq!(
            split_authority("arxiv.org", 80),
            Some(("arxiv.org".into(), 80))
        );
    }

    #[test]
    fn a_bracketed_address_survives_parsing_so_it_can_be_refused_by_name() {
        // Reaching the address-literal refusal matters more than parsing
        // neatly: the log should say what was attempted.
        assert_eq!(
            split_authority("[::1]:443", 80),
            Some(("[::1]".into(), 443))
        );
        assert_eq!(split_authority("[::1]", 80), Some(("[::1]".into(), 80)));
    }

    #[test]
    fn a_port_that_is_not_a_number_is_not_an_authority() {
        assert_eq!(split_authority("arxiv.org:https", 80), None);
        assert_eq!(split_authority(":443", 80), None);
        assert_eq!(split_authority("", 80), None);
    }
}
