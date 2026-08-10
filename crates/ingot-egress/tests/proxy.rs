//! The proxy against real sockets.
//!
//! The unit tests assert what the filter *decides*. These assert what a client
//! can actually reach, which is the only claim that matters: a boundary is
//! judged by traffic, not by its opinion of traffic.
//!
//! Everything here talks to `127.0.0.1`, so the allowlist is built with
//! `allowing_private_addresses`. That switch exists for these tests and nothing
//! else — in a real boundary a granted name resolving to loopback is refused,
//! and there is a test below that proves the refusal is still reachable.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use ingot_egress::{Allowlist, Decision, Proxy};

/// An origin server that answers once and reports that it was reached.
fn origin(body: &'static str) -> (SocketAddr, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binding an origin");
    let address = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        for incoming in listener.incoming() {
            let Ok(mut stream) = incoming else { return };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            let _ = reader.read_line(&mut request_line);
            let mut headers = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                    break;
                }
                headers.push_str(&line);
            }
            let _ = tx.send(format!("{request_line}{headers}"));
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.flush();
        }
    });

    (address, rx)
}

fn proxy(hosts: &[&str]) -> (Proxy, mpsc::Receiver<Decision>) {
    let (tx, rx) = mpsc::channel();
    let allow = Allowlist::new(hosts).allowing_private_addresses();
    let proxy = Proxy::start("127.0.0.1:0".parse().unwrap(), allow, move |decision| {
        let _ = tx.send(decision);
    })
    .expect("starting the proxy");
    (proxy, rx)
}

/// Send a raw request through the proxy and read everything back.
fn through(proxy: &Proxy, request: &str) -> String {
    let mut stream = TcpStream::connect(proxy.address()).expect("connecting to the proxy");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    stream.flush().unwrap();
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response
}

#[test]
fn a_granted_host_is_forwarded_to_the_origin() {
    let (address, seen) = origin("the answer");
    let (proxy, decisions) = proxy(&["localhost"]);

    let response = through(
        &proxy,
        &format!(
            "GET http://localhost:{}/paper HTTP/1.1\r\nhost: localhost\r\n\r\n",
            address.port()
        ),
    );

    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with("the answer"), "{response}");

    // The origin saw an origin-form request, which is what an origin expects.
    let request = seen.recv_timeout(Duration::from_secs(5)).expect("reached");
    assert!(request.starts_with("GET /paper HTTP/1.1"), "{request}");

    assert!(matches!(
        decisions.recv_timeout(Duration::from_secs(5)).unwrap(),
        Decision::Allowed { .. }
    ));
}

#[test]
fn a_host_the_policy_does_not_name_is_refused_and_never_dialled() {
    let (address, seen) = origin("must not be reached");
    let (proxy, decisions) = proxy(&["arxiv.org"]);

    let response = through(
        &proxy,
        &format!("GET http://localhost:{}/ HTTP/1.1\r\n\r\n", address.port()),
    );

    assert!(response.starts_with("HTTP/1.1 403"), "{response}");
    // Said in terms a tool server can report, rather than a dropped connection
    // that gets debugged as a network fault.
    assert!(
        response.contains("not a host this agent's policy grants"),
        "{response}"
    );

    assert!(
        seen.recv_timeout(Duration::from_millis(300)).is_err(),
        "the origin was contacted for a host the policy does not grant"
    );
    assert!(matches!(
        decisions.recv_timeout(Duration::from_secs(5)).unwrap(),
        Decision::Refused { .. }
    ));
}

#[test]
fn an_address_literal_is_refused_even_when_it_resolves_to_a_granted_host() {
    // The bypass this closes: `localhost` is granted and points at 127.0.0.1,
    // so dialling the address directly would reach exactly the same place while
    // skipping the name check. A policy grants names.
    let (address, seen) = origin("must not be reached");
    let (proxy, _decisions) = proxy(&["localhost"]);

    let response = through(
        &proxy,
        &format!("CONNECT 127.0.0.1:{} HTTP/1.1\r\n\r\n", address.port()),
    );

    assert!(response.starts_with("HTTP/1.1 403"), "{response}");
    assert!(
        response.contains("address rather than a host name"),
        "{response}"
    );
    assert!(seen.recv_timeout(Duration::from_millis(300)).is_err());
}

#[test]
fn a_connect_tunnel_carries_bytes_to_the_host_that_was_dialled() {
    let (address, seen) = origin("tunnelled");
    let (proxy, _decisions) = proxy(&["localhost"]);

    let mut stream = TcpStream::connect(proxy.address()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "CONNECT localhost:{} HTTP/1.1\r\nhost: localhost\r\n\r\n",
        address.port()
    )
    .unwrap();
    stream.flush().unwrap();

    // The tunnel is open once the proxy says so, and everything after is raw.
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut established = String::new();
    reader.read_line(&mut established).unwrap();
    assert!(established.starts_with("HTTP/1.1 200"), "{established}");
    let mut blank = String::new();
    reader.read_line(&mut blank).unwrap();

    write!(stream, "GET /through HTTP/1.1\r\nhost: localhost\r\n\r\n").unwrap();
    stream.flush().unwrap();

    let mut response = String::new();
    let _ = reader.read_to_string(&mut response);
    assert!(response.ends_with("tunnelled"), "{response}");

    let request = seen.recv_timeout(Duration::from_secs(5)).expect("reached");
    assert!(request.starts_with("GET /through"), "{request}");
}

#[test]
fn a_connect_to_an_ungranted_host_is_refused_before_the_tunnel_opens() {
    let (address, seen) = origin("must not be reached");
    let (proxy, _decisions) = proxy(&["arxiv.org"]);

    let response = through(
        &proxy,
        &format!("CONNECT localhost:{} HTTP/1.1\r\n\r\n", address.port()),
    );

    assert!(response.starts_with("HTTP/1.1 403"), "{response}");
    assert!(
        !response.contains("Connection Established"),
        "a refused CONNECT must not open a tunnel:\n{response}"
    );
    assert!(seen.recv_timeout(Duration::from_millis(300)).is_err());
}

#[test]
fn the_host_header_does_not_decide_the_destination() {
    // Two names in one request, and only the one in the request target may
    // count. Consulting both is how a filter and a destination come to
    // disagree — the plain-HTTP form of the SNI-against-Host problem.
    let (address, seen) = origin("reached");
    let (proxy, _decisions) = proxy(&["localhost"]);

    // A granted target with a lying Host header still goes to the target.
    let response = through(
        &proxy,
        &format!(
            "GET http://localhost:{}/ HTTP/1.1\r\nhost: evil.test\r\n\r\n",
            address.port()
        ),
    );
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(seen.recv_timeout(Duration::from_secs(5)).is_ok());

    // And an ungranted target with a granted Host header is refused.
    let response = through(
        &proxy,
        "GET http://evil.test/ HTTP/1.1\r\nhost: localhost\r\n\r\n",
    );
    assert!(response.starts_with("HTTP/1.1 403"), "{response}");
}

#[test]
fn a_request_that_is_not_proxy_shaped_is_refused_rather_than_guessed_at() {
    let (proxy, _decisions) = proxy(&["localhost"]);

    // Origin form, which carries no host at all. A proxy that filled one in
    // from the Host header would be deciding on a field it must not read.
    let response = through(&proxy, "GET /paper HTTP/1.1\r\nhost: localhost\r\n\r\n");
    assert!(response.starts_with("HTTP/1.1 403"), "{response}");
}

#[test]
fn a_granted_name_that_points_inward_is_still_refused() {
    // The switch these tests run with is off in every real boundary, and this
    // is the proof that turning it off actually stops something: the same
    // request, the same granted name, refused.
    let (address, seen) = origin("must not be reached");
    let (tx, _rx) = mpsc::channel();
    let mut strict = Proxy::start(
        "127.0.0.1:0".parse().unwrap(),
        Allowlist::new(["localhost"]),
        move |decision| {
            let _ = tx.send(decision);
        },
    )
    .expect("starting the proxy");

    let response = through(
        &strict,
        &format!("GET http://localhost:{}/ HTTP/1.1\r\n\r\n", address.port()),
    );
    assert!(response.starts_with("HTTP/1.1 403"), "{response}");
    assert!(response.contains("not a public address"), "{response}");
    assert!(seen.recv_timeout(Duration::from_millis(300)).is_err());
    strict.shutdown();
}
