//! `ingot-egress` — the proxy, on its own.
//!
//! A separate binary from `ingot` on purpose. This is the process that sits on
//! the network edge of a boundary, so its image should contain the filter and
//! nothing else: no interpreter, no model providers, no TLS stack, no tool
//! server. The crate has no dependencies and neither does this.
//!
//! Argument parsing is written out rather than delegated for the same reason.
//! It is twenty lines, and the alternative is a dependency in the one place
//! where a dependency has to be trusted.
//!
//!     ingot-egress --bind 0.0.0.0:8080 --allow arxiv.org --allow github.com
//!
//! `ingot egress` in the main CLI is the same proxy, for watching one locally.

use std::net::SocketAddr;
use std::process::ExitCode;

use ingot_egress::{Allowlist, Proxy};

const USAGE: &str = "\
ingot-egress — bound a container's egress to the hosts a policy names

    ingot-egress [--bind ADDR] [--allow HOST]...

    --bind ADDR    where to listen (default 0.0.0.0:8080)
    --allow HOST   a host that may be reached; repeatable, matched exactly

With no --allow, every request is refused. That is the honest default for a
component whose job is to say no.
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("ingot-egress: {message}");
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut bind = "0.0.0.0:8080".to_string();
    let mut allow: Vec<String> = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--bind" => bind = args.next().ok_or("`--bind` needs an address")?,
            "--allow" => allow.push(args.next().ok_or("`--allow` needs a host")?),
            "--help" | "-h" => {
                print!("{USAGE}");
                return Ok(());
            }
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }

    let bind: SocketAddr = bind
        .parse()
        .map_err(|_| format!("`{bind}` is not an address:port"))?;
    let allow = Allowlist::new(&allow);

    // Said at startup so a log shows what this was told, not only what it did.
    // A proxy allowing nothing and a proxy nobody used look identical
    // afterwards.
    if allow.is_empty() {
        eprintln!("egress: no hosts allowed; every request will be refused");
    } else {
        eprintln!("egress: allowing {}", allow.hosts().join(", "));
    }

    let proxy = Proxy::start(bind, allow, |decision| eprintln!("egress: {decision}"))
        .map_err(|error| format!("could not listen on {bind}: {error}"))?;
    println!("{}", proxy.address());

    loop {
        std::thread::park();
    }
}
