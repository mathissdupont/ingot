//! Just enough HTTP/1.1 to talk to one browser on the loopback interface.
//!
//! Not a general server. It reads a request, writes a response, and closes the
//! connection — no keep-alive, no chunked bodies, no ranges, no compression. A
//! browser handles all of that being absent; a general server would be a larger
//! thing to get right for no gain here.

use std::io::{self, BufReader, Read, Write};
use std::net::TcpStream;

/// The longest request line and header block this will read.
///
/// Bounded because the alternative is a client that sends header bytes forever
/// and a server that buffers them forever.
const MAX_HEADER_BYTES: usize = 16 * 1024;

/// The largest body this will accept.
///
/// Every route here takes its arguments in the query string; a body exists so
/// that a future one can, not because anything sends a large one.
const MAX_BODY_BYTES: usize = 256 * 1024;

/// The methods the studio answers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Delete,
}

impl Method {
    fn parse(word: &str) -> Option<Method> {
        match word {
            "GET" => Some(Method::Get),
            "POST" => Some(Method::Post),
            "DELETE" => Some(Method::Delete),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Delete => "DELETE",
        }
    }
}

/// A parsed request, before any route has looked at it.
#[derive(Debug, Clone)]
pub struct Head {
    pub method: Method,
    /// The path, percent-decoded, always beginning with `/`.
    pub path: String,
    /// Query parameters, percent-decoded, in the order they were sent.
    pub query: Vec<(String, String)>,
    /// The `Host` header, which the guard checks before anything else runs.
    pub host: Option<String>,
    /// The `Origin` header, present when a browser thinks this is cross-site.
    pub origin: Option<String>,
    /// The `X-Ingot-Token` header, the way a fetch carries the session token.
    pub header_token: Option<String>,
    content_length: usize,
}

impl Head {
    /// The first value sent for `name`.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// Read a request head, or `None` if the peer hung up without sending one.
pub fn read_head(reader: &mut BufReader<TcpStream>) -> io::Result<Option<Head>> {
    let mut budget = MAX_HEADER_BYTES;
    let Some(line) = read_line(reader, &mut budget)? else {
        return Ok(None);
    };

    let mut words = line.split(' ');
    let (Some(method), Some(target), Some(version)) = (words.next(), words.next(), words.next())
    else {
        return Err(malformed("the request line has fewer than three words"));
    };
    if !version.starts_with("HTTP/1.") {
        return Err(malformed("only HTTP/1.x is spoken here"));
    }
    let Some(method) = Method::parse(method) else {
        return Err(malformed("unsupported method"));
    };

    let (path, query) = split_target(target)?;

    let mut host = None;
    let mut origin = None;
    let mut header_token = None;
    let mut content_length = 0usize;
    loop {
        let Some(line) = read_line(reader, &mut budget)? else {
            return Err(malformed("the header block ended without a blank line"));
        };
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(malformed("a header line has no colon"));
        };
        let value = value.trim().to_string();
        match name.trim().to_ascii_lowercase().as_str() {
            "host" => host = Some(value),
            "origin" => origin = Some(value),
            "x-ingot-token" => header_token = Some(value),
            "content-length" => {
                content_length = value
                    .parse::<usize>()
                    .map_err(|_| malformed("Content-Length is not a number"))?;
                if content_length > MAX_BODY_BYTES {
                    return Err(malformed("the body is larger than this server accepts"));
                }
            }
            // A body this server cannot measure is a body it will not read: the
            // alternative is guessing where the request ends.
            "transfer-encoding" => {
                return Err(malformed("transfer codings are not supported"));
            }
            _ => {}
        }
    }

    Ok(Some(Head {
        method,
        path,
        query,
        host,
        origin,
        header_token,
        content_length,
    }))
}

/// Read exactly the body the head declared.
pub fn read_body(reader: &mut BufReader<TcpStream>, head: &Head) -> io::Result<Vec<u8>> {
    let mut body = vec![0u8; head.content_length];
    reader.read_exact(&mut body)?;
    Ok(body)
}

/// Write one response and say nothing else on this connection.
///
/// The header block is the same for every reply because every reply is private
/// to one person on one machine: nothing here may be cached, referred to, or
/// loaded from anywhere but itself.
pub fn respond(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Referrer-Policy: no-referrer\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Content-Security-Policy: {CSP}\r\n\
         Connection: close\r\n\
         \r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

/// What the page is allowed to load, which is nothing it did not arrive with.
///
/// The whole studio is one document with its style and script inline, so
/// `'unsafe-inline'` is what inline means here rather than a relaxation: no
/// external origin can be reached even if a report somehow carried a URL into
/// the page.
const CSP: &str = "default-src 'none'; \
                   script-src 'unsafe-inline'; \
                   style-src 'unsafe-inline'; \
                   connect-src 'self'; \
                   img-src data:; \
                   form-action 'none'; \
                   base-uri 'none'; \
                   frame-ancestors 'none'";

fn read_line(reader: &mut BufReader<TcpStream>, budget: &mut usize) -> io::Result<Option<String>> {
    let mut line = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        match reader.read(&mut byte) {
            Ok(0) if line.is_empty() => return Ok(None),
            Ok(0) => return Err(malformed("the connection ended mid-line")),
            Ok(_) => {}
            Err(error) => return Err(error),
        }
        if *budget == 0 {
            return Err(malformed(
                "the header block is larger than this server reads",
            ));
        }
        *budget -= 1;
        if byte[0] == b'\n' {
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return String::from_utf8(line)
                .map(Some)
                .map_err(|_| malformed("a header line is not UTF-8"));
        }
        line.push(byte[0]);
    }
}

/// Split `/path?a=b` into a decoded path and decoded parameters.
fn split_target(target: &str) -> io::Result<(String, Vec<(String, String)>)> {
    let (raw_path, raw_query) = match target.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (target, None),
    };
    if !raw_path.starts_with('/') {
        return Err(malformed("the request target is not a path"));
    }

    let path = percent_decode(raw_path)?;
    // A decoded path is what routes are matched against, so a `%2e%2e` cannot
    // become a `..` after the match has already happened.
    if path.contains('\0') {
        return Err(malformed("the path contains a null byte"));
    }

    let mut query = Vec::new();
    if let Some(raw_query) = raw_query {
        for pair in raw_query.split('&').filter(|pair| !pair.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            query.push((percent_decode(key)?, percent_decode(value)?));
        }
    }
    Ok((path, query))
}

/// `%XX` and `+`, the two escapes a browser produces.
fn percent_decode(text: &str) -> io::Result<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' => {
                let (Some(high), Some(low)) = (
                    bytes.get(index + 1).and_then(|byte| hex(*byte)),
                    bytes.get(index + 2).and_then(|byte| hex(*byte)),
                ) else {
                    return Err(malformed("a percent escape is incomplete"));
                };
                out.push(high * 16 + low);
                index += 3;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| malformed("a decoded value is not UTF-8"))
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn malformed(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_windows_path_survives_the_query_string() {
        // Project paths are the studio's main parameter and they contain
        // drive letters, backslashes and spaces on the platform most likely to
        // be running this.
        let (path, query) =
            split_target("/api/project?path=C%3A%5CUsers%5Csam%5CMy%20Agent").expect("must parse");
        assert_eq!(path, "/api/project");
        assert_eq!(query[0].1, r"C:\Users\sam\My Agent");
    }

    #[test]
    fn a_plus_is_a_space_and_a_percent_is_a_byte() {
        assert_eq!(percent_decode("a+b%2Fc").expect("must decode"), "a b/c");
    }

    #[test]
    fn an_incomplete_escape_is_refused_rather_than_passed_through() {
        assert!(percent_decode("%2").is_err());
        assert!(percent_decode("%zz").is_err());
    }

    #[test]
    fn a_target_without_a_leading_slash_is_not_a_path() {
        assert!(split_target("api/projects").is_err());
    }

    #[test]
    fn a_missing_parameter_is_absent_rather_than_empty() {
        let (_, query) = split_target("/api/run?id=7").expect("must parse");
        let head = Head {
            method: Method::Get,
            path: "/api/run".into(),
            query,
            host: None,
            origin: None,
            header_token: None,
            content_length: 0,
        };
        assert_eq!(head.param("id"), Some("7"));
        assert_eq!(head.param("path"), None);
    }
}
