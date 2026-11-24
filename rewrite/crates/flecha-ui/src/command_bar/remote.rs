use std::sync::OnceLock;

use async_compat::Compat;
use flecha_core::{SearchEngine, Settings, input_text::trim_input, suggestions};
use reqwest::Client;

pub(super) struct SuggestionRequest {
    pub(super) engine: SearchEngine,
    query: String,
    url: String,
}

/// Whether the window may ask a search provider at all. An incognito window never does, whatever
/// the setting: Chromium's omnibox sends no suggestion requests in incognito (`flechaApi.ts`
/// `searchSuggestions`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RemoteSuggestions {
    Allowed,
    Refused,
}

impl SuggestionRequest {
    pub(super) fn new(settings: &Settings, remote: RemoteSuggestions, text: &str) -> Option<Self> {
        let engine = settings.search_engine;
        if remote == RemoteSuggestions::Refused || !settings.search_suggestions_enabled() {
            return None;
        }
        let query = trim_input(text);
        Some(Self {
            engine,
            url: engine.suggestion_url(query)?,
            query: query.into(),
        })
    }

    pub(super) fn matches_settings(&self, settings: &Settings) -> bool {
        settings.search_suggestions_enabled() && self.engine == settings.search_engine
    }
}

#[derive(Default)]
pub(crate) struct SuggestionClient {
    http: OnceLock<Result<Client, reqwest::Error>>,
}

impl SuggestionClient {
    pub(super) async fn load(&self, request: &SuggestionRequest) -> anyhow::Result<Vec<String>> {
        self.load_url(request, &request.url).await
    }

    async fn load_url(
        &self,
        request: &SuggestionRequest,
        url: &str,
    ) -> anyhow::Result<Vec<String>> {
        Compat::new(async {
            let client = self.http.get_or_init(|| {
                // No cookie store, browser session, or referrer on suggestion requests.
                Client::builder().https_only(true).referer(false).build()
            });
            let client = client
                .as_ref()
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            let response = client.get(url).send().await?;
            if !response.status().is_success() {
                return Ok(Vec::new());
            }
            let body = response.text().await?;
            Ok(suggestions::parse_suggestions(&body, &request.query))
        })
        .await
    }
}

#[cfg(test)]
impl SuggestionClient {
    pub(super) fn fixture() -> Self {
        Self {
            http: OnceLock::from(Client::builder().no_proxy().referer(false).build()),
        }
    }

    pub(super) async fn load_fixture(
        &self,
        request: &SuggestionRequest,
        address: std::net::SocketAddr,
    ) -> anyhow::Result<Vec<String>> {
        let mut url = reqwest::Url::parse(&request.url)?;
        url.set_scheme("http").unwrap();
        url.set_ip_host(address.ip()).unwrap();
        url.set_port(Some(address.port())).unwrap();
        self.load_url(request, url.as_str()).await
    }
}
