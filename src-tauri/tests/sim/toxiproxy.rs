//! Fault injection through Toxiproxy, which sits between each adapter under
//! test and its simulator (`sim/toxiproxy/toxiproxy.json` names the
//! proxies). The faults are the network states that are hard to reach on
//! purpose with a real printer: a host that disappears, a slow link, a
//! response cut off part way, and a connection that goes silent.

use serde_json::json;

use super::http::{self, Url};
use super::{sim_url, Skip};

pub struct Toxiproxy {
    api: Url,
}

/// One proxy, named as in `toxiproxy.json`.
#[derive(Clone, Copy, Debug)]
pub enum Proxy {
    Moonraker,
    MoonrakerMulti,
    OctoPrint,
}

impl Proxy {
    pub const ALL: [Proxy; 3] = [Proxy::Moonraker, Proxy::MoonrakerMulti, Proxy::OctoPrint];

    fn name(self) -> &'static str {
        match self {
            Proxy::Moonraker => "moonraker",
            Proxy::MoonrakerMulti => "moonraker-multi",
            Proxy::OctoPrint => "octoprint",
        }
    }
}

impl Toxiproxy {
    pub fn discover() -> Result<Self, Skip> {
        let api = sim_url("FARM3D_SIM_TOXIPROXY")?;
        http::get_json(&api, "/version", &[]).map_err(|error| {
            Skip(format!(
                "the fault proxy at {} is not answering: {error}",
                api.authority()
            ))
        })?;
        Ok(Self { api })
    }

    /// Re-enables every proxy and removes every toxic.
    ///
    /// Proxies are disabled first. That closes every connection at once;
    /// otherwise removing a latency toxic waits for the data it is still
    /// holding back, which can take as long as the latency itself.
    pub fn reset(&self) {
        for proxy in Proxy::ALL {
            self.set_enabled(proxy, false);
        }
        self.call("POST", "/reset", None);
    }

    /// `false` closes every open connection and refuses new ones, which is
    /// what a host that went away looks like to a client.
    pub fn set_enabled(&self, proxy: Proxy, enabled: bool) {
        self.call(
            "POST",
            &format!("/proxies/{}", proxy.name()),
            Some(json!({"enabled": enabled})),
        );
    }

    /// Delays every response chunk by `ms`. Keep it below the adapter's
    /// timeouts: Toxiproxy cannot drop a latency toxic quickly while it is
    /// still holding data back. Use [`Toxiproxy::hang`] for a timeout.
    pub fn slow(&self, proxy: Proxy, ms: u64) {
        self.toxic(proxy, "slow", "latency", json!({"latency": ms}));
    }

    /// Closes each connection after `bytes` of response.
    pub fn cut_after(&self, proxy: Proxy, bytes: u64) {
        self.toxic(proxy, "cut", "limit_data", json!({"bytes": bytes}));
    }

    /// Closes each connection after `bytes` of the **request**: an upload
    /// cut mid-body, before the host has the whole file (spike Gate D). The
    /// other toxics act on responses only.
    pub fn cut_request_after(&self, proxy: Proxy, bytes: u64) {
        self.toxic_on(
            proxy,
            "cut-request",
            "limit_data",
            "upstream",
            json!({"bytes": bytes}),
        );
    }

    /// Stops delivering responses but keeps connections open, like a pulled
    /// cable: no FIN, no RST, just silence.
    pub fn hang(&self, proxy: Proxy) {
        self.toxic(proxy, "hang", "timeout", json!({"timeout": 0}));
    }

    fn toxic(&self, proxy: Proxy, name: &str, kind: &str, attributes: serde_json::Value) {
        self.toxic_on(proxy, name, kind, "downstream", attributes);
    }

    /// `stream` is Toxiproxy's direction: `downstream` acts on what the
    /// host sends back, `upstream` on what the client sends.
    fn toxic_on(
        &self,
        proxy: Proxy,
        name: &str,
        kind: &str,
        stream: &str,
        attributes: serde_json::Value,
    ) {
        self.call(
            "POST",
            &format!("/proxies/{}/toxics", proxy.name()),
            Some(json!({
                "name": name,
                "type": kind,
                "stream": stream,
                "attributes": attributes,
            })),
        );
    }

    fn call(&self, method: &str, path: &str, body: Option<serde_json::Value>) {
        let response = http::request(method, &self.api, path, &[], body.as_ref())
            .unwrap_or_else(|error| panic!("toxiproxy {method} {path}: {error}"));
        assert!(
            (200..300).contains(&response.status),
            "toxiproxy {method} {path} answered HTTP {}: {}",
            response.status,
            String::from_utf8_lossy(&response.body)
        );
    }
}
