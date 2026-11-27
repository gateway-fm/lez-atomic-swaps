//! The LEZ node endpoints a role may connect to, checked against the
//! configured route before anything is sent.
//!
//! The role sidecar (`compat/lez-v0_2-sidecar`, `runtime.rs`) accepts a
//! sequencer or indexer URL only under its route profile: literal-loopback
//! HTTP for `local`, the one official HTTPS origin for `official_public`.
//! The Node reads the same two services itself (`getAccount`,
//! `getLastBlockId`, `getTransaction`, `getLastFinalizedBlockId`), so it
//! applies the same rule when the configuration is loaded. The sidecar is its
//! own Cargo workspace; the rule is mirrored here and a test pins the origin
//! to the sidecar's source.
//!
//! [`LezNodeEndpoint`] has no public constructor other than the two checks,
//! and every reader in the Nodes takes one, so a URL that was not checked
//! cannot reach a client.

use std::net::IpAddr;

use anyhow::{Result, bail};
use url::{Host, Url};

use crate::config::LezSequencerRoute;

/// The one official public LEZ origin, exactly as the sidecar allowlists it
/// (`OFFICIAL_PUBLIC_NODE_ENDPOINT`).
pub const OFFICIAL_PUBLIC_LEZ_ENDPOINT: &str = "https://testnet.lez.logos.co/";

/// A LEZ sequencer or indexer URL that passed its route's check.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LezNodeEndpoint(String);

impl LezNodeEndpoint {
    /// The sequencer URL under `route`: literal-loopback HTTP for `local`,
    /// the exact official origin for `official_public`.
    ///
    /// # Errors
    ///
    /// Refuses every other URL; the message names the route's rule and never
    /// repeats the URL, which may carry credentials.
    pub fn sequencer(route: LezSequencerRoute, url: &str) -> Result<Self> {
        match route {
            LezSequencerRoute::Local => {
                if !is_literal_loopback_http(url) {
                    bail!(
                        "lez.sequencer_url is not a literal-loopback HTTP URL with a port (http://127.0.0.1:<port>/), which the local sequencer route requires"
                    );
                }
            }
            LezSequencerRoute::OfficialPublic => {
                if !is_official_public_origin(url) {
                    bail!(
                        "lez.sequencer_url is not {OFFICIAL_PUBLIC_LEZ_ENDPOINT}, the only URL the official_public sequencer route accepts"
                    );
                }
            }
        }
        Ok(Self(url.to_owned()))
    }

    /// The indexer URL: literal-loopback HTTP under every route, since the
    /// official origin serves no indexer.
    ///
    /// # Errors
    ///
    /// Refuses every other URL, without repeating it.
    pub fn indexer(url: &str) -> Result<Self> {
        if !is_literal_loopback_http(url) {
            bail!(
                "lez.indexer_url is not a literal-loopback HTTP URL with a port (http://127.0.0.1:<port>/), which every sequencer route requires of the indexer"
            );
        }
        Ok(Self(url.to_owned()))
    }

    /// The checked URL, as configured.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The sidecar's `validate_local_node_endpoint`: HTTP, a literal loopback
/// address (no name, `localhost` included), an explicit port, no credentials,
/// query or fragment, and the root path.
fn is_literal_loopback_http(endpoint: &str) -> bool {
    let Ok(parsed) = Url::parse(endpoint) else {
        return false;
    };
    let loopback = match parsed.host() {
        Some(Host::Ipv4(address)) => IpAddr::V4(address).is_loopback(),
        Some(Host::Ipv6(address)) => IpAddr::V6(address).is_loopback(),
        Some(Host::Domain(_)) | None => false,
    };
    parsed.scheme() == "http"
        && loopback
        && parsed.port().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.query().is_none()
        && parsed.fragment().is_none()
        && parsed.path() == "/"
}

/// The sidecar's `validate_official_public_endpoint`: the exact text of the
/// official origin, and every part of it as parsed.
fn is_official_public_origin(endpoint: &str) -> bool {
    let Ok(parsed) = Url::parse(endpoint) else {
        return false;
    };
    endpoint == OFFICIAL_PUBLIC_LEZ_ENDPOINT
        && parsed.scheme() == "https"
        && parsed.host_str() == Some("testnet.lez.logos.co")
        && parsed.port().is_none()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.path() == "/"
        && parsed.query().is_none()
        && parsed.fragment().is_none()
}

#[cfg(test)]
mod tests {
    use super::{LezNodeEndpoint, OFFICIAL_PUBLIC_LEZ_ENDPOINT};
    use crate::config::LezSequencerRoute::{Local, OfficialPublic};

    /// URLs no route accepts for the sequencer: a name instead of a literal
    /// address, another scheme, credentials, a path, a query, a fragment,
    /// lookalike hosts of the official origin, and text that is not a URL.
    const NEVER: &[&str] = &[
        "",
        "127.0.0.1:3040",
        "http://localhost:3040/",
        "http://sequencer:3040/",
        "http://sequencer:3040",
        "http://127.0.0.1.example.com:3040/",
        "http://192.0.2.1:3040/",
        "http://0.0.0.0:3040/",
        "http://[::ffff:127.0.0.1]:3040/",
        "http://[2001:db8::1]:3040/",
        "https://127.0.0.1:3040/",
        "ws://127.0.0.1:3040/",
        "file:///etc/passwd",
        "http://127.0.0.1/",
        "http://127.0.0.1:80/",
        "http://user@127.0.0.1:3040/",
        "http://user:secret@127.0.0.1:3040/",
        "http://127.0.0.1:3040/path",
        "http://127.0.0.1:3040//",
        "http://127.0.0.1:3040/?query=1",
        "http://127.0.0.1:3040/#fragment",
        "http://127.0.0.1:3040@example.com/",
        "http://testnet.lez.logos.co/",
        "https://testnet.lez.logos.co:443/",
        "https://testnet.lez.logos.co:8443/",
        "https://user@testnet.lez.logos.co/",
        "https://user:secret@testnet.lez.logos.co/",
        "https://testnet.lez.logos.co/path",
        "https://testnet.lez.logos.co/?query=1",
        "https://testnet.lez.logos.co/#fragment",
        "https://testnet.lez.logos.co./",
        "https://TESTNET.lez.logos.co/",
        "https://testnet.lez.logos.co.example.com/",
        "https://testnet.lez.logos.co@example.com/",
        "https://example.com/#https://testnet.lez.logos.co/",
        "https://xn--testnet-lez-logos-co.example/",
        "https://testnet-lez.logos.co/",
        "https://example.com/",
        " https://testnet.lez.logos.co/",
        "https://testnet.lez.logos.co/ ",
    ];

    /// Literal loopback as the local route accepts it.
    const LOOPBACK: &[&str] = &[
        "http://127.0.0.1:3040/",
        "http://127.0.0.1:3040",
        "http://127.0.0.1:1/",
        "http://127.8.9.10:3040/",
        "http://[::1]:3040/",
    ];

    #[test]
    fn the_local_route_accepts_only_literal_loopback_http_with_a_port() {
        for url in LOOPBACK {
            let endpoint = LezNodeEndpoint::sequencer(Local, url)
                .unwrap_or_else(|error| panic!("{url}: {error:#}"));
            assert_eq!(endpoint.as_str(), *url);
        }
        for url in NEVER
            .iter()
            .chain(&[OFFICIAL_PUBLIC_LEZ_ENDPOINT, "https://testnet.lez.logos.co"])
        {
            let refusal = LezNodeEndpoint::sequencer(Local, url)
                .expect_err(url)
                .to_string();
            assert!(
                refusal.contains("local sequencer route"),
                "{url}: {refusal}"
            );
        }
    }

    #[test]
    fn the_official_public_route_accepts_only_the_exact_official_origin() {
        let endpoint = LezNodeEndpoint::sequencer(OfficialPublic, OFFICIAL_PUBLIC_LEZ_ENDPOINT)
            .expect("the official origin");
        assert_eq!(endpoint.as_str(), "https://testnet.lez.logos.co/");
        // The origin without its trailing slash is another text and refused,
        // as the sidecar refuses it; so is every loopback URL.
        for url in NEVER
            .iter()
            .chain(LOOPBACK)
            .chain(&["https://testnet.lez.logos.co"])
        {
            let refusal = LezNodeEndpoint::sequencer(OfficialPublic, url)
                .expect_err(url)
                .to_string();
            assert!(
                refusal.contains("official_public sequencer route"),
                "{url}: {refusal}"
            );
        }
    }

    #[test]
    fn the_indexer_is_literal_loopback_under_every_route() {
        for url in LOOPBACK {
            LezNodeEndpoint::indexer(url).unwrap_or_else(|error| panic!("{url}: {error:#}"));
        }
        for url in NEVER.iter().chain(&[OFFICIAL_PUBLIC_LEZ_ENDPOINT]) {
            LezNodeEndpoint::indexer(url).expect_err(url);
        }
    }

    #[test]
    fn a_refusal_never_repeats_the_url() {
        let secret = "http://user:hunter2@192.0.2.1:3040/";
        for refusal in [
            LezNodeEndpoint::sequencer(Local, secret).expect_err("local"),
            LezNodeEndpoint::sequencer(OfficialPublic, secret).expect_err("official"),
            LezNodeEndpoint::indexer(secret).expect_err("indexer"),
        ] {
            let text = format!("{refusal:#}");
            assert!(
                !text.contains("hunter2") && !text.contains("192.0.2.1"),
                "{text}"
            );
        }
    }

    /// The sidecar is a separate Cargo workspace, so the origin cannot be one
    /// shared constant: this reads the sidecar's source and fails when its
    /// allowlisted origin and the Node's differ.
    #[test]
    fn the_official_origin_is_the_one_the_sidecar_allowlists() {
        let sidecar = include_str!("../../../compat/lez-v0_2-sidecar/src/runtime.rs");
        let pinned = format!(
            "const OFFICIAL_PUBLIC_NODE_ENDPOINT: &str = \"{OFFICIAL_PUBLIC_LEZ_ENDPOINT}\";"
        );
        assert!(
            sidecar.contains(&pinned),
            "the sidecar's official origin is no longer {OFFICIAL_PUBLIC_LEZ_ENDPOINT}"
        );
        // The sidecar's own refused and accepted examples
        // (tests/runtime_boundary.rs) get the same answer here.
        let examples = include_str!("../../../compat/lez-v0_2-sidecar/tests/runtime_boundary.rs");
        for refused in [
            "http://localhost:3040/",
            "https://127.0.0.1:3040/",
            "http://127.0.0.1/",
            "http://user@127.0.0.1:3040/",
            "http://127.0.0.1:3040/path",
        ] {
            assert!(examples.contains(&format!("\"{refused}\"")), "{refused}");
            LezNodeEndpoint::sequencer(Local, refused).expect_err(refused);
        }
        for refused in [
            "https://testnet.lez.logos.co",
            "http://testnet.lez.logos.co/",
            "https://user@testnet.lez.logos.co/",
            "https://testnet.lez.logos.co/path",
            "https://testnet.lez.logos.co/?query=1",
            "https://testnet.lez.logos.co/#fragment",
            "https://example.com/",
            "http://127.0.0.1:3040/",
        ] {
            assert!(examples.contains(&format!("\"{refused}\"")), "{refused}");
            LezNodeEndpoint::sequencer(OfficialPublic, refused).expect_err(refused);
        }
        assert!(examples.contains("connect_local(\"http://127.0.0.1:1/\")"));
        LezNodeEndpoint::sequencer(Local, "http://127.0.0.1:1/").expect("the sidecar accepts it");
    }
}
