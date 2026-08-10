//! The arrangement that makes a host allowlist true.
//!
//! A filter only bounds what is routed through it, and [`crate::invocation`]
//! cannot route anything by itself: a `docker run` flag can join a network but
//! cannot create one. This module owns the two objects that have to exist
//! before a bounded server starts, and have to be gone after it stops.
//!
//! # The topology, and why it is this one
//!
//! ```text
//!   ┌─────────────────────┐        ┌──────────────────┐
//!   │  tool server        │        │  ingot-egress    │──── the internet
//!   │  internal only      │───────▶│  internal+bridge │
//!   └─────────────────────┘        └──────────────────┘
//! ```
//!
//! The tool server joins a network created `--internal`, which has no route
//! out. The proxy joins that network *and* an ordinary one, so it is the only
//! thing on the internal side that can reach anything.
//!
//! **The network is the bound; the proxy variables are only a signpost.** A
//! server that reads `HTTP_PROXY` goes through the filter. A server that
//! ignores it — or that never heard of it — reaches nothing at all, because
//! there is nowhere for its packets to go. That distinction is what separates
//! this from setting an environment variable and hoping: the enforcement does
//! not depend on the contained process cooperating.
//!
//! # Why the proxy is its own image
//!
//! It is the one process here that can reach both the contained server and the
//! internet. Everything that does not need to be in that position is a reason
//! for it to be somewhere else, so the image holds one dependency-free binary:
//! no interpreter, no model provider, no TLS stack, no tool server, and no
//! credential.

use std::fmt;
use std::process::Command;

use crate::executor::ExecutorError;
use crate::Runtime;

/// The default image the proxy runs from.
///
/// Built from this checkout — `tools/egress.Dockerfile` — for the same reason
/// the run image is: what enforces a policy should be what you can read.
pub const DEFAULT_EGRESS_IMAGE: &str = concat!("ingot/egress:", env!("CARGO_PKG_VERSION"));

/// The port the proxy listens on inside its container.
const PROXY_PORT: u16 = 8080;

/// Where a bounded container joins, and what it talks to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressRoute {
    /// A container network with no route out.
    pub network: String,
    /// `name:port` of the proxy on that network.
    pub proxy: String,
}

/// A live network and proxy, removed when this is dropped.
///
/// Dropped rather than closed explicitly, because the paths out of a run
/// include panics and early returns, and a leaked internal network is a leaked
/// container that can still reach the internet.
pub struct EgressBoundary {
    runtime: Runtime,
    route: EgressRoute,
    container: String,
    torn_down: bool,
}

impl fmt::Debug for EgressBoundary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EgressBoundary")
            .field("network", &self.route.network)
            .field("container", &self.container)
            .finish()
    }
}

impl EgressBoundary {
    /// Create the network, start the proxy, and give back the route.
    ///
    /// `name` distinguishes one run's objects from another's; a caller that
    /// reuses one would have two runs sharing a filter built from one policy.
    pub fn start(
        runtime: &Runtime,
        name: &str,
        hosts: &[String],
        image: &str,
    ) -> Result<EgressBoundary, ExecutorError> {
        let network = format!("ingot-egress-{name}");
        let container = format!("ingot-egress-{name}");

        run(runtime, &["network", "create", "--internal", &network])?;

        // From here on every failure has to take the network with it, or a
        // refused run leaves an object behind that the next one trips over.
        let mut boundary = EgressBoundary {
            runtime: runtime.clone(),
            route: EgressRoute {
                network: network.clone(),
                proxy: format!("{container}:{PROXY_PORT}"),
            },
            container: container.clone(),
            torn_down: false,
        };

        let mut start = vec![
            "run".to_string(),
            "-d".to_string(),
            "--rm".to_string(),
            "--name".to_string(),
            container.clone(),
            "--network".to_string(),
            network.clone(),
            // The same hardening the tool container gets. This one holds no
            // credential and touches no filesystem, so there is even less for
            // it to need.
            "--security-opt".to_string(),
            "no-new-privileges".to_string(),
            "--cap-drop".to_string(),
            "ALL".to_string(),
            "--read-only".to_string(),
            image.to_string(),
            "--bind".to_string(),
            format!("0.0.0.0:{PROXY_PORT}"),
        ];
        for host in hosts {
            start.push("--allow".to_string());
            start.push(host.clone());
        }
        if let Err(error) = run(
            runtime,
            &start.iter().map(String::as_str).collect::<Vec<_>>(),
        ) {
            boundary.tear_down();
            return Err(error);
        }

        // Second network, so the proxy — and only the proxy — has a way out.
        // Done after the container exists because a network is attached to a
        // container rather than promised to one.
        if let Err(error) = run(runtime, &["network", "connect", "bridge", &container]) {
            boundary.tear_down();
            return Err(error);
        }

        Ok(boundary)
    }

    pub fn route(&self) -> &EgressRoute {
        &self.route
    }

    /// Remove the proxy and the network, ignoring what is already gone.
    ///
    /// Errors are swallowed on purpose: this runs on the way out of a failure
    /// as often as a success, and a teardown that reported its own problems
    /// would bury the one the caller is trying to read.
    pub fn tear_down(&mut self) {
        if self.torn_down {
            return;
        }
        self.torn_down = true;
        let _ = run(&self.runtime, &["rm", "-f", &self.container]);
        let _ = run(&self.runtime, &["network", "rm", &self.route.network]);
    }
}

impl Drop for EgressBoundary {
    fn drop(&mut self) {
        self.tear_down();
    }
}

fn run(runtime: &Runtime, args: &[&str]) -> Result<String, ExecutorError> {
    let output = Command::new(&runtime.program)
        .args(args)
        .output()
        .map_err(|error| ExecutorError::RuntimeUnavailable {
            runtime: runtime.program.clone(),
            reason: error.to_string(),
        })?;
    if !output.status.success() {
        return Err(ExecutorError::RuntimeUnavailable {
            runtime: runtime.program.clone(),
            reason: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_route_names_the_proxy_on_the_network_the_container_joins() {
        // A container on the internal network reaches the proxy by container
        // name, which only resolves on a user-defined network — the reason the
        // network is created rather than the default bridge being reused.
        let route = EgressRoute {
            network: "ingot-egress-abc".into(),
            proxy: "ingot-egress-abc:8080".into(),
        };
        assert!(route.proxy.starts_with(&route.network));
        assert!(route.proxy.ends_with(":8080"));
    }

    #[test]
    fn the_default_image_is_tagged_with_this_build() {
        // The proxy and the toolchain that starts it are built from one
        // checkout, so a stale image cannot quietly enforce an older rule.
        assert!(DEFAULT_EGRESS_IMAGE.starts_with("ingot/egress:"));
        assert!(DEFAULT_EGRESS_IMAGE.ends_with(env!("CARGO_PKG_VERSION")));
    }
}
