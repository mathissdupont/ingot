//! Ingot Studio: one place that shows what nine commands each show a piece of.
//!
//! [GAP-025](../../../docs/gaps.md#gap-025) is not a missing command. Every
//! fact about a project is already available — `check` has the diagnostics,
//! `doctor` has the readiness, `sandbox` has the boundary, a run has its event
//! stream — and none of them are in the same place. This is that place.
//!
//! # What this crate is, and what it deliberately is not
//!
//! It is a socket, a parser, a guard and a page. It holds no compiler, no
//! manifest reader, no provider and no policy: everything it shows arrives
//! through [`Answers`], which the command-line tool implements by calling the
//! same functions its own subcommands call.
//!
//! That split is the point. [RFC-0007] refused to build a surface first,
//! because a surface that computed anything would become a second source of
//! truth about what an agent is. A crate with no way to compute cannot become
//! one — the guarantee is structural rather than a rule somebody has to keep.
//!
//! # Why a local server rather than a desktop framework
//!
//! It ships inside the binary that already ships. No Node, no npm, no bundler,
//! no second toolchain to keep current and no dependency tree to audit — the
//! same discipline as `ingot-egress`, for a component that also listens on a
//! socket.
//!
//! # Who can reach it
//!
//! A loopback port is reachable by every process on the machine, and — through
//! a page open in the same browser — by every site the person is visiting.
//! Three checks, and a request has to pass all three:
//!
//! * **The token.** Fresh per process, printed once in the URL, stored nowhere.
//! * **The `Host` header.** It must be a loopback authority naming this exact
//!   port. A name that resolves to `127.0.0.1` is how DNS rebinding turns a
//!   browser into a proxy for a stranger; such a request arrives with that name
//!   in `Host` and is refused here.
//! * **The `Origin` header.** Present means a browser considers the request
//!   cross-site. Anything but this server's own origin is refused, so a page on
//!   another site cannot read a report even from the right machine.
//!
//! [RFC-0007]: ../../../rfcs/0007-the-ingot-product-loop.md

use std::io::{self, BufReader};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

mod http;
mod token;

pub use http::{Head, Method};

/// The page, style and script inline, embedded in the binary.
///
/// One document rather than a directory of assets: nothing then has to be
/// fetched with the token attached, and the page cannot half-load.
const PAGE: &str = include_str!("../assets/index.html");

/// How long a connection may take to say what it wants.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Everything the studio knows, which is nothing it works out itself.
///
/// One method rather than one per panel, because the set of panels is going to
/// change and the seam between "transport" and "the toolchain" should not have
/// to change with it. Routes are named in [`Head::path`] with the `/api/`
/// prefix still attached.
///
/// Called on a connection's own thread, so an implementation must be usable
/// from several at once.
pub trait Answers: Send + Sync {
    fn answer(&self, request: &Head, body: &[u8]) -> Reply;
}

/// What a route said.
pub enum Reply {
    /// A JSON document, already serialized.
    Json(String),
    /// The route exists; the caller asked it for something it will not do.
    Refused(String),
    /// No such route.
    Unknown,
    /// The route exists and the work behind it failed.
    Failed(String),
}

/// A running studio.
pub struct Studio {
    address: SocketAddr,
    token: String,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

/// Deliberately hand-written, and deliberately without the token: a studio is
/// the kind of thing that ends up in a panic message or a log line, and a
/// derived `Debug` would put the session's credential in one.
impl std::fmt::Debug for Studio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Studio")
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

impl Studio {
    /// Bind, and start answering.
    ///
    /// `bind` must be a loopback address. A studio on `0.0.0.0` would publish
    /// one person's project paths, environment-variable names and run history
    /// to the network they are on, so it is refused rather than warned about.
    pub fn start(bind: SocketAddr, answers: Arc<dyn Answers>) -> io::Result<Studio> {
        if !bind.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{bind} is not a loopback address; the studio serves one person on one machine"
                ),
            ));
        }

        let listener = TcpListener::bind(bind)?;
        let address = listener.local_addr()?;
        let token = token::fresh();
        let stop = Arc::new(AtomicBool::new(false));

        let thread = {
            let stop = Arc::clone(&stop);
            let token = token.clone();
            thread::spawn(move || {
                for incoming in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                    let Ok(client) = incoming else { continue };
                    let answers = Arc::clone(&answers);
                    let token = token.clone();
                    thread::spawn(move || {
                        // A browser opens and abandons connections as a matter
                        // of course. None of that is worth a message.
                        let _ = serve(client, address, &token, answers.as_ref());
                    });
                }
            })
        };

        Ok(Studio {
            address,
            token,
            stop,
            thread: Some(thread),
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// The one URL that opens the studio, token included.
    pub fn url(&self) -> String {
        format!("http://{}/?token={}", self.address, self.token)
    }

    /// Block until [`Studio::shutdown`] is called or the listener dies.
    pub fn wait(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    /// Stop accepting. Connections already open are left to finish.
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Studio {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// One connection: read, check, answer, close.
fn serve(
    mut client: TcpStream,
    address: SocketAddr,
    token: &str,
    answers: &dyn Answers,
) -> io::Result<()> {
    client.set_read_timeout(Some(READ_TIMEOUT))?;
    let mut reader = BufReader::new(client.try_clone()?);

    let Some(head) = http::read_head(&mut reader)? else {
        return Ok(());
    };
    let body = http::read_body(&mut reader, &head)?;

    let (status, reason, content_type, payload) = decide(&head, &body, address, token, answers);
    http::respond(
        &mut client,
        status,
        reason,
        content_type,
        payload.as_bytes(),
    )?;

    // Say the connection is over rather than letting the socket go and hoping.
    //
    // `Connection: close` promises a clean end, and dropping the stream does
    // not always deliver one: a route may have started a child process, which
    // on some platforms inherits a duplicate of this very socket, so the
    // connection outlives the handle this function is holding. The reader then
    // waits for an end that never comes, or is cut off by a reset. `shutdown`
    // acts on the connection rather than on one handle to it, so the caller
    // sees the end of the body exactly when the body ends.
    let _ = client.shutdown(Shutdown::Both);
    Ok(())
}

/// What to answer, without writing anything.
///
/// Separated from the writing so that every path — refused, unknown, answered —
/// leaves through the same close.
fn decide(
    head: &Head,
    body: &[u8],
    address: SocketAddr,
    token: &str,
    answers: &dyn Answers,
) -> (u16, &'static str, &'static str, String) {
    if let Err(reason) = admitted(head, address, token) {
        // The refusal says which check failed and nothing about the token, so
        // the message is useful to the person who started this and useless to
        // anyone guessing.
        return (
            403,
            "Forbidden",
            "text/plain; charset=utf-8",
            format!("refused: {reason}\n"),
        );
    }

    if head.path == "/" {
        return (200, "OK", "text/html; charset=utf-8", PAGE.to_string());
    }

    let route = head.path.strip_prefix("/api/").unwrap_or_default();
    if route.is_empty() {
        return not_found();
    }

    match answers.answer(head, body) {
        Reply::Json(document) => (200, "OK", "application/json; charset=utf-8", document),
        Reply::Refused(reason) => problem(400, "Bad Request", &reason),
        Reply::Unknown => not_found(),
        Reply::Failed(reason) => problem(500, "Internal Server Error", &reason),
    }
}

/// Whether this request may be answered at all.
fn admitted(head: &Head, address: SocketAddr, token: &str) -> Result<(), String> {
    let Some(host) = &head.host else {
        return Err("the request carries no Host header".to_string());
    };
    if !is_own_authority(host, address) {
        return Err(format!(
            "Host `{host}` is not this server's own address; the studio answers only to \
             127.0.0.1:{port} or localhost:{port}",
            port = address.port()
        ));
    }
    if let Some(origin) = &head.origin {
        let allowed = origin
            .strip_prefix("http://")
            .map(|authority| is_own_authority(authority, address))
            .unwrap_or(false);
        if !allowed {
            return Err(format!("Origin `{origin}` is not this studio"));
        }
    }

    let offered = head
        .header_token
        .as_deref()
        .or_else(|| head.param("token"))
        .unwrap_or_default();
    if !token::matches(token, offered) {
        return Err(
            "the session token is missing or wrong; open the URL `ingot studio` printed"
                .to_string(),
        );
    }
    Ok(())
}

/// Whether an authority names this server and no other.
///
/// The port has to match as well as the name: another local process on another
/// port is as much a stranger as a remote one.
fn is_own_authority(authority: &str, address: SocketAddr) -> bool {
    let expected = address.port();
    let (name, port) = match authority.rsplit_once(':') {
        // `[::1]:7317` splits correctly; a bare `[::1]` does not have a port.
        Some((name, port)) if !name.ends_with('[') => (name, port.parse::<u16>().ok()),
        _ => (authority, None),
    };
    if port != Some(expected) {
        return false;
    }
    matches!(name, "127.0.0.1" | "localhost" | "[::1]")
}

fn not_found() -> (u16, &'static str, &'static str, String) {
    problem(404, "Not Found", "no such route")
}

fn problem(
    status: u16,
    reason: &'static str,
    detail: &str,
) -> (u16, &'static str, &'static str, String) {
    (
        status,
        reason,
        "application/json; charset=utf-8",
        format!("{{\"error\":{}}}", json_string(detail)),
    )
}

/// A JSON string literal, without pulling in a serializer for one field.
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", control as u32))
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn address(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    }

    #[test]
    fn a_host_naming_another_port_is_not_this_server() {
        // The bypass this refuses: something on 7318 persuading a browser that
        // a page it serves is same-origin with the studio on 7317.
        assert!(is_own_authority("127.0.0.1:7317", address(7317)));
        assert!(!is_own_authority("127.0.0.1:7318", address(7317)));
    }

    #[test]
    fn a_name_that_merely_resolves_here_is_refused() {
        // DNS rebinding: `studio.attacker.example` resolving to 127.0.0.1
        // reaches the socket, and arrives with its own name in Host.
        assert!(!is_own_authority(
            "studio.attacker.example:7317",
            address(7317)
        ));
        assert!(!is_own_authority(
            "localhost.attacker.example:7317",
            address(7317)
        ));
    }

    #[test]
    fn the_three_loopback_spellings_are_the_same_server() {
        for authority in ["127.0.0.1:7317", "localhost:7317", "[::1]:7317"] {
            assert!(is_own_authority(authority, address(7317)), "{authority}");
        }
    }

    #[test]
    fn an_authority_without_a_port_is_not_this_server() {
        // A studio always has a port in its own URL, so a bare name is either a
        // default-port request or a mistake, and neither is this.
        assert!(!is_own_authority("127.0.0.1", address(7317)));
        assert!(!is_own_authority("[::1]", address(7317)));
    }

    #[test]
    fn a_control_character_cannot_break_out_of_an_error_message() {
        assert_eq!(json_string("a\"b\nc"), "\"a\\\"b\\nc\"");
    }

    #[test]
    fn the_page_never_turns_a_report_into_markup() {
        // Everything the page shows is somebody's path, diagnostic or model
        // output. It builds nodes and sets `textContent`; the moment one of
        // these appears, a report can carry markup into the document.
        //
        // Matched on the form that *does* something — `.innerHTML`, not
        // `innerHTML` — because a comment explaining the rule is not a breach
        // of it, and a test that cannot tell the difference gets deleted.
        for hazard in [
            ".innerHTML",
            ".outerHTML",
            ".insertAdjacentHTML(",
            "document.write(",
            "eval(",
            "new Function(",
        ] {
            assert!(!PAGE.contains(hazard), "the page uses `{hazard}`");
        }
    }

    #[test]
    fn the_page_asks_for_nothing_from_outside_this_machine() {
        // The response header forbids it, but a page written to need a font or
        // a script from elsewhere would simply be broken rather than refused —
        // and broken only for the person whose network blocks it.
        //
        // Again the fetching form, not the substring: the connections panel
        // shows a `base-url = "https://…"` line as an example of what to write
        // in a manifest, and displayed text reaches nowhere.
        for outside in [
            "src=\"http",
            "src=\"//",
            "href=\"http",
            "href=\"//",
            "url(http",
            "url(//",
            "@import",
            "fetch(\"http",
        ] {
            assert!(!PAGE.contains(outside), "the page reaches for `{outside}`");
        }
    }
}
