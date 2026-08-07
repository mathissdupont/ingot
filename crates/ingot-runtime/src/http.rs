//! The HTTP call every network provider makes.
//!
//! Shared so that "how Ingot talks to a model service" is decided once: the
//! same timeout, the same retry rule, the same mapping from a status code to a
//! [`ProviderError`]. A second provider that retried differently would make two
//! artifacts behave differently for reasons the artifact never mentions.
//!
//! Compiled only when a network provider is.

use std::time::Duration;

use serde_json::Value;

use crate::provider::ProviderError;

/// Long enough for a slow reasoning model, short enough that a wedged
/// connection does not hold a run open indefinitely.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(180);
pub const DEFAULT_MAX_RETRIES: u32 = 3;

/// POST a JSON body and read a JSON reply, retrying what is worth retrying.
///
/// `headers` carries authentication, so it never appears in an error message.
pub fn post_json(
    url: &str,
    headers: &[(&str, &str)],
    body: &Value,
    timeout: Duration,
    max_retries: u32,
) -> Result<Value, ProviderError> {
    let mut attempt = 0;

    loop {
        attempt += 1;
        let mut request = ureq::post(url)
            .config()
            .timeout_global(Some(timeout))
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
                if attempt <= max_retries {
                    std::thread::sleep(backoff(attempt));
                    continue;
                }
                return Err(ProviderError::Transport(error.to_string()));
            }
        }
    }
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
}
