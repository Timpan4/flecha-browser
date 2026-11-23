//! The real transport: blocking ureq on the updater's own thread.
//!
//! ureq is built without its `gzip` and `brotli` features, so it neither adds Accept-Encoding
//! nor decodes the body: [`crate::fetch`] sees the wire bytes and checks Content-Length against
//! them, as build-adblock.mjs does. `tests/transport.rs` fails if a feature change ever turns
//! transparent decompression back on.

use std::{io, time::Instant};

use crate::fetch::{Download, DownloadRequest, WireResponse};

#[derive(Clone, Debug)]
pub struct UreqDownload {
    agent: ureq::Agent,
}

impl Default for UreqDownload {
    fn default() -> Self {
        let agent = ureq::Agent::config_builder()
            // fetch::follow_redirects follows redirects itself, HTTPS only.
            .max_redirects(0)
            .http_status_as_error(false)
            .build()
            .into();
        Self { agent }
    }
}

impl Download for UreqDownload {
    fn get(&self, url: &str, request: &DownloadRequest) -> io::Result<WireResponse> {
        let remaining = request.deadline.saturating_duration_since(Instant::now());
        let response = self
            .agent
            .get(url)
            .config()
            .timeout_global(Some(remaining))
            .build()
            .header("Accept-Encoding", request.accept_encoding)
            .call()
            .map_err(io::Error::other)?;
        let (parts, body) = response.into_parts();
        let headers = parts
            .headers
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_owned(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect();
        Ok(WireResponse {
            status: parts.status.as_u16(),
            headers,
            body: Box::new(body.into_reader()),
        })
    }
}
