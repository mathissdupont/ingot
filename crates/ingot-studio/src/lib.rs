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
/// **Modular in the source tree, one document on the wire.** `assets/` is a
/// directory of small files — a stylesheet per region, a script per view — and
/// they are concatenated here at compile time.
///
/// The joining is not an inconvenience to be optimised away later. Two
/// properties depend on the response being a single document:
///
/// * **Every request to this server carries the session token.** A page that
///   fetched its own stylesheet would have to put the token in that URL, which
///   means the token in a `src` attribute, in the history, and in any
///   screenshot of the page.
/// * **The page cannot half-load.** There is no state where the markup arrived
///   and the script did not, so nothing has to be written to survive one.
///
/// The order is the order a browser needs: metadata, style, markup, behaviour.
/// Within the script the order is dependency order — `boot` defines the helpers
/// the views use, and `start` is last because it runs.
const PAGE: &str = concat!(
    include_str!("../assets/head.html"),
    "\n<style>\n",
    include_str!("../assets/css/tokens.css"),
    "\n",
    include_str!("../assets/css/base.css"),
    "\n",
    include_str!("../assets/css/sprite.css"),
    "\n",
    include_str!("../assets/css/rail.css"),
    "\n",
    include_str!("../assets/css/page.css"),
    "\n",
    include_str!("../assets/css/track.css"),
    "\n",
    include_str!("../assets/css/canvas.css"),
    "\n",
    include_str!("../assets/css/waiting.css"),
    "\n",
    include_str!("../assets/css/chat.css"),
    "\n",
    include_str!("../assets/css/contained.css"),
    "\n",
    include_str!("../assets/css/create.css"),
    "\n",
    include_str!("../assets/css/trace.css"),
    "</style>\n\n",
    include_str!("../assets/shell.html"),
    "\n<script>\n",
    include_str!("../assets/js/boot.js"),
    "\n",
    include_str!("../assets/js/words.js"),
    "\n",
    include_str!("../assets/js/sprite.js"),
    "\n",
    include_str!("../assets/js/track.js"),
    "\n",
    include_str!("../assets/js/state.js"),
    "\n",
    include_str!("../assets/js/load.js"),
    "\n",
    include_str!("../assets/js/render.js"),
    "\n",
    include_str!("../assets/js/create.js"),
    "\n",
    include_str!("../assets/js/projects.js"),
    "\n",
    include_str!("../assets/js/project.js"),
    "\n",
    include_str!("../assets/js/launcher-form.js"),
    "\n",
    include_str!("../assets/js/canvas.js"),
    "\n",
    include_str!("../assets/js/contained.js"),
    "\n",
    include_str!("../assets/js/waiting.js"),
    "\n",
    include_str!("../assets/js/conversation.js"),
    "\n",
    include_str!("../assets/js/runs.js"),
    "\n",
    include_str!("../assets/js/machine.js"),
    "\n",
    include_str!("../assets/js/start.js"),
    "</script>\n",
);

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
    fn every_asset_reaches_the_page() {
        // The one hazard the split introduced: `assets/` is now a directory, so
        // a new stylesheet or view can be written, saved, and never added to
        // the `concat!` above — in which case it silently does nothing and the
        // only symptom is a feature that is not there.
        //
        // Asserted by containment rather than by a file count, so it also
        // catches a file that was wired in and then emptied.
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        let mut checked = 0;
        let mut pending = vec![assets.clone()];
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(&directory).expect("reading assets") {
                let path = entry.expect("an entry").path();
                if path.is_dir() {
                    pending.push(path);
                    continue;
                }
                let contents = std::fs::read_to_string(&path).expect("reading an asset");
                let relative = path.strip_prefix(&assets).unwrap_or(&path).display();
                assert!(
                    !contents.trim().is_empty(),
                    "`{relative}` is empty, so whatever it was for is missing"
                );
                assert!(
                    PAGE.contains(contents.trim_end()),
                    "`{relative}` is not in the page: add it to `PAGE`"
                );
                // A control character in an asset is not a style question. One
                // reached `waiting.js` as a separator between two strings and
                // survived review, because git treats a file holding a NUL as
                // binary and shows no diff for it — and it was then served
                // inside the page. Tab and newline are the only ones a source
                // file has any business carrying.
                if let Some(bad) = contents
                    .chars()
                    .find(|c| c.is_control() && *c != '\n' && *c != '\t' && *c != '\r')
                {
                    panic!(
                        "`{relative}` holds the control character {:?}; a source file may not",
                        bad
                    );
                }
                checked += 1;
            }
        }
        // A floor, so the walk silently finding nothing cannot pass.
        assert!(checked >= 20, "only {checked} assets were checked");
    }

    #[test]
    fn every_colour_the_page_uses_exists_in_both_palettes() {
        // The failure this catches is invisible in whichever theme you happen to
        // be looking at. A token added to `:root` and forgotten in the dark
        // block does not break: it *inherits the light value*, so a fill sized
        // for white text on a light ground goes on being used against a dark
        // one, and the contrast quietly stops holding. A misspelled `var(--x)`
        // is worse and just as quiet — it resolves to nothing at all.
        //
        // The rule needs no list to maintain, because the value says which kind
        // of token it is: a hex colour is palette-dependent by construction, and
        // `--radius` and `--mono` are not.
        let css = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("css");
        let tokens = std::fs::read_to_string(css.join("tokens.css")).expect("tokens.css");

        let dark_at = tokens
            .find("prefers-color-scheme: dark")
            .expect("a dark palette");
        let declared = |text: &str| -> Vec<(String, String)> {
            text.lines()
                .filter_map(|line| line.trim().strip_prefix("--"))
                .filter_map(|line| line.split_once(':'))
                .map(|(name, value)| {
                    (
                        name.trim().to_string(),
                        value.trim().trim_end_matches(';').to_string(),
                    )
                })
                .collect()
        };
        let light = declared(&tokens[..dark_at]);
        let dark = declared(&tokens[dark_at..]);
        assert!(light.len() > 12, "only {} light tokens found", light.len());

        for (name, value) in &light {
            if !value.starts_with('#') {
                continue;
            }
            assert!(
                dark.iter().any(|(other, _)| other == name),
                "`--{name}` is a colour with no dark value, so the dark page uses the light one"
            );
        }

        // And every name the page reaches for is a name something defines. The
        // sprite palettes live in `sprite.css` rather than in `:root`, so the
        // search is over every stylesheet.
        let mut defined: Vec<String> = Vec::new();
        let mut used: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(&css).expect("reading css") {
            let text = std::fs::read_to_string(entry.expect("an entry").path()).expect("a file");
            for (name, _) in declared(&text) {
                defined.push(name);
            }
            let mut rest = text.as_str();
            while let Some(at) = rest.find("var(--") {
                rest = &rest[at + "var(--".len()..];
                let end = rest
                    .find(|c: char| !c.is_ascii_alphanumeric() && c != '-')
                    .unwrap_or(rest.len());
                used.push(rest[..end].to_string());
            }
        }
        assert!(used.len() > 40, "only {} uses found", used.len());
        for name in &used {
            assert!(
                defined.contains(name),
                "`var(--{name})` resolves to nothing: no stylesheet defines it"
            );
        }
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
