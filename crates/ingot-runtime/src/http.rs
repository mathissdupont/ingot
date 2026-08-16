//! The HTTP call every network provider makes.
//!
//! Shared so that "how Ingot talks to a model service" is decided once: the
//! same retry rule, the same default wait, the same mapping from a status code
//! to a [`ProviderError`]. A second provider that retried differently would
//! make two artifacts behave differently for reasons the artifact never
//! mentions.
//!
//! The wait itself is per-endpoint, because it is the one of those that is a
//! fact about the service rather than about Ingot — see `timeout-seconds` on a
//! [`crate::catalogue::ProviderConfig`].
//!
//! Compiled only when a network provider is.

use std::io::{BufRead, BufReader};
use std::time::Duration;

use serde_json::Value;

use crate::provider::ProviderError;

/// The wait a provider gets when its declaration does not state one.
///
/// Defined beside the field that overrides it rather than here, because a build
/// with no HTTP support still reads and prints manifests — and one number with
/// two definitions is how they come to disagree.
pub use crate::catalogue::DEFAULT_TIMEOUT;

pub const DEFAULT_MAX_RETRIES: u32 = 3;

/// POST a JSON body and read a JSON reply, retrying what is worth retrying.
///
/// `timeout` bounds each attempt, and running out of it ends the call — see
/// [`is_timeout`]. `None` is no bound at all, which is what an operator asks
/// for with `timeout-seconds = 0`.
///
/// `headers` carries authentication, so it never appears in an error message.
pub fn post_json(
    url: &str,
    headers: &[(&str, &str)],
    body: &Value,
    timeout: Option<Duration>,
    max_retries: u32,
) -> Result<Value, ProviderError> {
    let mut attempt = 0;

    loop {
        attempt += 1;
        let mut request = ureq::post(url)
            .config()
            .timeout_global(timeout)
            .build()
            .header("content-type", "application/json");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }

        match request.send_json(body) {
            Ok(mut ok) => {
                return ok
                    .body_mut()
                    .read_json()
                    .map_err(|error| ProviderError::Transport(error.to_string()));
            }
            Err(ureq::Error::StatusCode(status)) => {
                let retryable = status == 429 || status >= 500;
                if retryable && attempt <= max_retries {
                    // No jitter: runs should be reproducible, and the retry
                    // count is small enough that thundering herds are not the
                    // failure mode a reference interpreter guards against.
                    std::thread::sleep(backoff(attempt));
                    continue;
                }
                if status == 429 {
                    return Err(ProviderError::RateLimited {
                        retry_after_seconds: None,
                    });
                }
                return Err(ProviderError::Request {
                    status,
                    message: describe_status(status).to_string(),
                });
            }
            Err(error) => {
                if attempt <= max_retries && !is_timeout(&error) {
                    std::thread::sleep(backoff(attempt));
                    continue;
                }
                return Err(ProviderError::Transport(error.to_string()));
            }
        }
    }
}

/// Whether an attempt ran out of the time it was given.
///
/// **Not retried, unlike every other transport failure.** A refused connection
/// or a reset is a machine that might answer if asked again; a request that ran
/// out of time is an endpoint that is there and is slow, and asking it the same
/// question again is slow again. Retrying would also multiply the ceiling by
/// the retry count, so `timeout-seconds = 900` would mean an hour — and the
/// number an operator writes has to be the wait they get. It is the rule
/// `[mcp] timeout-seconds` already follows for a tool call.
fn is_timeout(error: &ureq::Error) -> bool {
    matches!(error, ureq::Error::Timeout(_))
}

/// POST a JSON body and read a `text/event-stream` reply, one event at a time.
///
/// `on_event` is called with each event's name and its parsed `data` payload,
/// in arrival order. Events the caller does not recognise are still delivered;
/// deciding what is meaningful is the provider's job, not the transport's.
///
/// **Retries stop once anything has been delivered.** A stream that fails
/// half-way cannot be started again: the caller has already shown that text to
/// somebody, and a second attempt would repeat it from the beginning. So a
/// retry here only covers the window before the first event, where nothing has
/// been observed and the attempt is genuinely repeatable.
pub fn post_sse(
    url: &str,
    headers: &[(&str, &str)],
    body: &Value,
    timeout: Option<Duration>,
    max_retries: u32,
    on_event: &mut dyn FnMut(&str, &Value),
) -> Result<(), ProviderError> {
    let mut attempt = 0;

    loop {
        attempt += 1;
        let mut request = ureq::post(url)
            .config()
            .timeout_global(timeout)
            .build()
            .header("content-type", "application/json")
            .header("accept", "text/event-stream");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }

        match request.send_json(body) {
            Ok(mut ok) => {
                let mut delivered = false;
                let reader = BufReader::new(ok.body_mut().as_reader());
                let outcome = read_events(reader, &mut delivered, on_event);
                return match outcome {
                    Ok(()) => Ok(()),
                    Err(error) if delivered || attempt > max_retries => Err(error),
                    // Nothing was observed, so nothing would be repeated.
                    Err(_) => {
                        std::thread::sleep(backoff(attempt));
                        continue;
                    }
                };
            }
            Err(ureq::Error::StatusCode(status)) => {
                let retryable = status == 429 || status >= 500;
                if retryable && attempt <= max_retries {
                    std::thread::sleep(backoff(attempt));
                    continue;
                }
                if status == 429 {
                    return Err(ProviderError::RateLimited {
                        retry_after_seconds: None,
                    });
                }
                return Err(ProviderError::Request {
                    status,
                    message: describe_status(status).to_string(),
                });
            }
            Err(error) => {
                if attempt <= max_retries && !is_timeout(&error) {
                    std::thread::sleep(backoff(attempt));
                    continue;
                }
                return Err(ProviderError::Transport(error.to_string()));
            }
        }
    }
}

/// Read `text/event-stream` framing off a reader.
///
/// Split out so the framing can be tested without a socket.
pub(crate) fn read_events(
    reader: impl BufRead,
    delivered: &mut bool,
    on_event: &mut dyn FnMut(&str, &Value),
) -> Result<(), ProviderError> {
    let mut name = String::new();
    let mut data = String::new();

    for line in reader.lines() {
        let line = line.map_err(|error| ProviderError::Transport(error.to_string()))?;
        let line = line.strip_suffix('\r').unwrap_or(&line);

        // A blank line ends an event. A comment (`:` first) is a keep-alive.
        if line.is_empty() {
            if !data.is_empty() {
                // `[DONE]` is not JSON. It is how an OpenAI-compatible stream
                // says it is over, and parsing it would fail the whole call at
                // the last possible moment.
                if data.trim() != "[DONE]" {
                    let payload: Value = serde_json::from_str(&data).map_err(|error| {
                        ProviderError::Transport(format!("malformed event data: {error}"))
                    })?;
                    *delivered = true;
                    on_event(&name, &payload);
                }
            }
            name.clear();
            data.clear();
            continue;
        }
        if line.starts_with(':') {
            continue;
        }

        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "event" => {
                name.clear();
                name.push_str(value);
            }
            // Multiple `data:` lines in one event concatenate with newlines,
            // per the event-stream format.
            "data" => {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(value);
            }
            _ => {}
        }
    }

    Ok(())
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_millis(500 * u64::from(attempt))
}

/// What a status code means, in words an operator can act on.
pub fn describe_status(status: u16) -> &'static str {
    match status {
        400 => "the request was malformed or used an unsupported parameter",
        401 => "the API key is missing or invalid",
        403 => "the API key lacks permission for this model",
        404 => "no such model or endpoint",
        413 => "the request is too large",
        422 => "the request was well-formed but the service refused it",
        529 => "the service is temporarily overloaded",
        _ => "the provider rejected the request",
    }
}

/// Read a key from the environment, with advice attached to its absence.
pub fn key_from_env(variable: &str) -> Result<String, ProviderError> {
    let key = std::env::var(variable).map_err(|_| {
        ProviderError::Configuration(format!(
            "{variable} is not set. Export it, or run with `--provider replay` \
             against a recorded cassette."
        ))
    })?;
    if key.trim().is_empty() {
        return Err(ProviderError::Configuration(format!(
            "{variable} is set but empty"
        )));
    }
    Ok(key)
}

/// An endpoint override, for a gateway, a proxy, or a stub server in a test.
pub fn base_url_from_env(variable: &str) -> Option<String> {
    std::env::var(variable)
        .ok()
        .filter(|url| !url.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_key_says_what_to_do_about_it() {
        let error = key_from_env("INGOT_A_VARIABLE_NOBODY_EXPORTS").unwrap_err();
        let text = error.to_string();
        assert!(text.contains("INGOT_A_VARIABLE_NOBODY_EXPORTS"), "{text}");
        assert!(text.contains("--provider replay"), "{text}");
    }

    #[test]
    fn an_empty_override_is_treated_as_absent() {
        // An exported-but-empty variable is a shell accident, not a request to
        // talk to the empty string.
        std::env::set_var("INGOT_TEST_BASE_URL", "   ");
        assert_eq!(base_url_from_env("INGOT_TEST_BASE_URL"), None);
        std::env::set_var("INGOT_TEST_BASE_URL", "http://127.0.0.1:1/v1");
        assert_eq!(
            base_url_from_env("INGOT_TEST_BASE_URL").as_deref(),
            Some("http://127.0.0.1:1/v1")
        );
        std::env::remove_var("INGOT_TEST_BASE_URL");
    }

    #[test]
    fn every_status_gets_words_rather_than_a_number() {
        for status in [400, 401, 403, 404, 413, 422, 529, 418] {
            assert!(!describe_status(status).is_empty());
        }
    }

    #[test]
    fn backoff_grows_with_the_attempt() {
        assert!(backoff(2) > backoff(1));
    }

    fn events(stream: &str) -> Vec<(String, Value)> {
        let mut seen = Vec::new();
        let mut delivered = false;
        read_events(stream.as_bytes(), &mut delivered, &mut |name, data| {
            seen.push((name.to_string(), data.clone()))
        })
        .unwrap();
        assert_eq!(delivered, !seen.is_empty());
        seen
    }

    #[test]
    fn a_blank_line_ends_an_event() {
        let seen = events("event: one\ndata: {\"a\":1}\n\nevent: two\ndata: {\"a\":2}\n\n");
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].0, "one");
        assert_eq!(seen[1].1["a"], 2);
    }

    #[test]
    fn keep_alive_comments_and_crlf_are_tolerated() {
        // A gateway that pads the stream with comments, and a server that ends
        // its lines the other way, are both ordinary and neither is an event.
        let seen = events(": ping\r\nevent: one\r\ndata: {\"a\":1}\r\n\r\n");
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].1["a"], 1);
    }

    #[test]
    fn the_done_sentinel_is_not_parsed_as_json() {
        let seen = events("data: {\"a\":1}\n\ndata: [DONE]\n\n");
        assert_eq!(seen.len(), 1, "[DONE] is framing, not an event: {seen:?}");
    }

    #[test]
    fn malformed_event_data_fails_rather_than_being_skipped() {
        let mut delivered = false;
        let error = read_events(
            "data: not json\n\n".as_bytes(),
            &mut delivered,
            &mut |_, _| {},
        )
        .unwrap_err();
        assert!(error.to_string().contains("malformed"), "{error}");
    }
}
