//! The studio from the other end of a socket.
//!
//! Every test here writes bytes and reads bytes. A guard tested through its own
//! function proves the function; a guard tested through a socket proves the
//! server, which is the thing a browser and everything else on the machine can
//! actually reach.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use ingot_studio::{Answers, Head, Method, Reply, Studio};

/// Answers one route and counts how often a route was reached at all.
struct Counting {
    reached: AtomicUsize,
}

impl Answers for Counting {
    fn answer(&self, request: &Head, _body: &[u8]) -> Reply {
        self.reached.fetch_add(1, Ordering::SeqCst);
        match (request.method, request.path.as_str()) {
            (Method::Get, "/api/projects") => Reply::Json("{\"projects\":[]}".to_string()),
            (Method::Delete, "/api/projects") => Reply::Json("{\"deleted\":true}".to_string()),
            (Method::Get, "/api/echo") => Reply::Json(format!(
                "{{\"path\":\"{}\"}}",
                request.param("path").unwrap_or_default().replace('\\', "/")
            )),
            (Method::Get, "/api/broken") => Reply::Failed("the report could not be built".into()),
            _ => Reply::Unknown,
        }
    }
}

struct Fixture {
    studio: Studio,
    answers: Arc<Counting>,
}

impl Fixture {
    fn start() -> Fixture {
        let answers = Arc::new(Counting {
            reached: AtomicUsize::new(0),
        });
        let studio = Studio::start(
            SocketAddr::from(([127, 0, 0, 1], 0)),
            answers.clone() as Arc<dyn Answers>,
        )
        .expect("the studio must bind to a free loopback port");
        Fixture { studio, answers }
    }

    fn token(&self) -> String {
        // The URL is the only place the token is published, so taking it from
        // there is also a check that the URL carries a usable one.
        let url = self.studio.url();
        url.rsplit_once("token=")
            .expect("the URL must carry a token")
            .1
            .to_string()
    }

    fn address(&self) -> SocketAddr {
        self.studio.address()
    }

    fn reached(&self) -> usize {
        self.answers.reached.load(Ordering::SeqCst)
    }

    /// Send a raw request and read the whole reply.
    fn send(&self, request: &str) -> String {
        let mut stream =
            TcpStream::connect(self.address()).expect("the studio must accept a connection");
        stream
            .write_all(request.as_bytes())
            .expect("the request must be written");
        stream.flush().expect("the request must flush");
        let mut reply = String::new();
        stream
            .read_to_string(&mut reply)
            .expect("the reply must be readable");
        reply
    }

    /// A well-formed request from the page the studio itself served.
    fn get(&self, target: &str) -> String {
        self.send(&format!(
            "GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Ingot-Token: {}\r\n\r\n",
            self.address().port(),
            self.token()
        ))
    }
}

fn status(reply: &str) -> u16 {
    reply
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0)
}

fn body(reply: &str) -> &str {
    reply
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or("")
}

#[test]
fn the_page_is_served_to_the_url_the_studio_printed() {
    let fixture = Fixture::start();
    let reply = fixture.get("/");
    assert_eq!(status(&reply), 200);
    assert!(body(&reply).contains("Ingot Studio"), "{reply}");
    // A page with no route behind it reached no route.
    assert_eq!(fixture.reached(), 0);
}

#[test]
fn a_request_without_the_token_is_refused_before_any_route_runs() {
    // The point is not the 403. It is that nothing behind the guard was asked
    // to do anything, so a stranger cannot make the studio read a directory.
    let fixture = Fixture::start();
    let reply = fixture.send(&format!(
        "GET /api/projects HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
        fixture.address().port()
    ));
    assert_eq!(status(&reply), 403);
    assert_eq!(fixture.reached(), 0);
}

#[test]
fn a_guessed_token_is_refused() {
    let fixture = Fixture::start();
    let reply = fixture.send(&format!(
        "GET /api/projects HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Ingot-Token: {}\r\n\r\n",
        fixture.address().port(),
        "0".repeat(32)
    ));
    assert_eq!(status(&reply), 403);
    assert_eq!(fixture.reached(), 0);
}

#[test]
fn a_name_that_merely_resolves_to_loopback_does_not_get_an_answer() {
    // DNS rebinding: a page on `studio.attacker.example`, whose name has been
    // pointed at 127.0.0.1, is same-origin with itself and can therefore read
    // whatever it fetches. The connection arrives here carrying that name.
    let fixture = Fixture::start();
    let reply = fixture.send(&format!(
        "GET /api/projects HTTP/1.1\r\nHost: studio.attacker.example:{}\r\nX-Ingot-Token: {}\r\n\r\n",
        fixture.address().port(),
        fixture.token()
    ));
    assert_eq!(status(&reply), 403);
    assert!(body(&reply).contains("attacker.example"), "{reply}");
    assert_eq!(fixture.reached(), 0);
}

#[test]
fn a_cross_site_page_is_refused_even_with_the_right_host() {
    // A page on another origin that somehow learned the token still announces
    // where it came from, and a browser will not let it lie about that.
    let fixture = Fixture::start();
    let reply = fixture.send(&format!(
        "GET /api/projects HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: https://example.test\r\nX-Ingot-Token: {token}\r\n\r\n",
        port = fixture.address().port(),
        token = fixture.token()
    ));
    assert_eq!(status(&reply), 403);
    assert_eq!(fixture.reached(), 0);
}

#[test]
fn the_studios_own_origin_is_allowed() {
    let fixture = Fixture::start();
    let port = fixture.address().port();
    let reply = fixture.send(&format!(
        "GET /api/projects HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nX-Ingot-Token: {token}\r\n\r\n",
        token = fixture.token()
    ));
    assert_eq!(status(&reply), 200);
    assert_eq!(body(&reply), "{\"projects\":[]}");
}

#[test]
fn the_token_may_also_arrive_in_the_query_string() {
    // How the page itself is first opened: the URL carries the token, because
    // the browser has nowhere to have got a header from yet.
    let fixture = Fixture::start();
    let reply = fixture.send(&format!(
        "GET /?token={} HTTP/1.1\r\nHost: localhost:{}\r\n\r\n",
        fixture.token(),
        fixture.address().port()
    ));
    assert_eq!(status(&reply), 200);
}

#[test]
fn a_windows_path_survives_the_round_trip() {
    let fixture = Fixture::start();
    let reply = fixture.get("/api/echo?path=C%3A%5CUsers%5Csam%5CMy%20Agent");
    assert_eq!(status(&reply), 200);
    assert_eq!(body(&reply), "{\"path\":\"C:/Users/sam/My Agent\"}");
}

#[test]
fn an_unknown_route_is_a_json_404() {
    let fixture = Fixture::start();
    let reply = fixture.get("/api/nothing");
    assert_eq!(status(&reply), 404);
    assert!(body(&reply).contains("no such route"), "{reply}");
}

#[test]
fn a_failing_route_says_so_without_pretending_to_have_data() {
    let fixture = Fixture::start();
    let reply = fixture.get("/api/broken");
    assert_eq!(status(&reply), 500);
    assert!(body(&reply).contains("could not be built"), "{reply}");
}

#[test]
fn every_reply_forbids_caching_and_outside_loads() {
    // A report is one person's project layout. Nothing may keep a copy of it,
    // and nothing the page renders may reach off the machine.
    let fixture = Fixture::start();
    let reply = fixture.get("/");
    assert!(reply.contains("Cache-Control: no-store"), "{reply}");
    assert!(reply.contains("Referrer-Policy: no-referrer"), "{reply}");
    assert!(reply.contains("default-src 'none'"), "{reply}");
    assert!(reply.contains("connect-src 'self'"), "{reply}");
}

#[test]
fn the_studio_refuses_to_listen_anywhere_but_loopback() {
    // Not a warning. A studio on 0.0.0.0 publishes one person's project paths,
    // environment-variable names and run history to whatever network they are
    // on, and a person who typed the wrong flag would not know.
    let answers = Arc::new(Counting {
        reached: AtomicUsize::new(0),
    });
    let error = Studio::start(SocketAddr::from(([0, 0, 0, 0], 0)), answers)
        .expect_err("a non-loopback bind must be refused");
    assert!(error.to_string().contains("loopback"), "{error}");
}

#[test]
fn a_delete_reaches_the_route_it_names() {
    let fixture = Fixture::start();
    let reply = fixture.send(&format!(
        "DELETE /api/projects?path=x HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Ingot-Token: {}\r\n\r\n",
        fixture.address().port(),
        fixture.token()
    ));
    assert_eq!(status(&reply), 200);
    assert_eq!(body(&reply), "{\"deleted\":true}");
}

#[test]
fn two_studios_do_not_share_a_token() {
    let first = Fixture::start();
    let second = Fixture::start();
    assert_ne!(first.token(), second.token());

    // And one's token is no use against the other, which is what makes a
    // second studio a second studio rather than a way into the first.
    let reply = second.send(&format!(
        "GET /api/projects HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Ingot-Token: {}\r\n\r\n",
        second.address().port(),
        first.token()
    ));
    assert_eq!(status(&reply), 403);
}
