//! `tempest open`: what the operating system runs for a `tempest:` link (D-263). It makes sure
//! a daemon runs and opens the web UI there, which asks before downloading.

use anyhow::{Context, Result};
use tempest_assets::show;

use crate::engine::{EngineArgs, Target};
use crate::get::parse_link;
use crate::handoff::Daemon;

/// What the page's address starts with when it offers a link; the web UI reads it.
const OFFER: &str = "/#/add?link=";

#[derive(Debug, clap::Args)]
pub struct OpenArgs {
    /// The bundle's tempest: link. Quote it: it contains `&`.
    link: String,
    /// Print the web UI's address rather than opening it in a browser.
    #[arg(long)]
    no_open: bool,
    #[command(flatten)]
    engine: EngineArgs,
}

/// `tempest open`: opens the web UI on the daemon, starting one if none runs, with a question
/// whether to download the link's bundle. Nothing downloads until it is answered.
pub fn open(args: OpenArgs) -> Result<()> {
    let link = parse_link(&args.link)?;
    let config = args.engine.load()?;
    let runtime = tokio::runtime::Runtime::new().context("cannot start the async runtime")?;
    let url = runtime.block_on(async {
        match Target::daemon(&config).await? {
            Some(client) => Ok(client.url().to_owned()),
            None => {
                let daemon = Daemon {
                    engine: args.engine.clone(),
                    set: Vec::new(),
                };
                daemon.start(&config).await
            }
        }
    })?;
    let url = format!(
        "{}{OFFER}{}",
        url.trim_end_matches('/'),
        encode(&link.to_string())
    );
    show(&url, !args.no_open);
    Ok(())
}

/// `text` percent-encoded as JavaScript's `encodeURIComponent` does.
fn encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            true => encoded.push(byte as char),
            false => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_is_encoded_for_the_address() {
        assert_eq!(
            encode("tempest:?bundle=ab&name=Holiday Photos"),
            "tempest%3A%3Fbundle%3Dab%26name%3DHoliday%20Photos"
        );
    }
}
