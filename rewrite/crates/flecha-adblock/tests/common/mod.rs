#![allow(dead_code)]

use std::{
    io::{self, Cursor},
    time::Duration,
};

use flecha_adblock::{
    AdBlocker, Download, DownloadRequest, LISTS, UpdateReport, WireResponse, update_once,
};

pub const EASYLIST: &str = include_str!("../fixtures/lists/easylist.txt");
pub const EASYPRIVACY: &str = include_str!("../fixtures/lists/easyprivacy.txt");
/// Only for calls whose outcome doesn't depend on it.
pub const RETRY: Duration = Duration::from_secs(60);

/// A list for `url` with the title the checks expect (build-adblock.test.mjs `subscription`).
pub fn subscription(url: &str, rules: &str) -> String {
    let title = if url == LISTS[0].url {
        "EasyList"
    } else {
        "EasyPrivacy"
    };
    format!("[Adblock Plus 2.0]\n! Title: {title}\n{rules}\n")
}

/// A 200 response with a matching Content-Length.
pub fn response(body: impl Into<Vec<u8>>) -> WireResponse {
    let body = body.into();
    let length = body.len().to_string();
    response_with(body, &[("content-length", &length)])
}

pub fn response_with(body: impl Into<Vec<u8>>, headers: &[(&str, &str)]) -> WireResponse {
    WireResponse {
        status: 200,
        headers: headers
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
        body: Box::new(Cursor::new(body.into())),
    }
}

/// A transport from a closure.
pub struct FnDownload<F>(pub F);

impl<F> Download for FnDownload<F>
where
    F: Fn(&str, &DownloadRequest) -> io::Result<WireResponse> + Send + Sync,
{
    fn get(&self, url: &str, request: &DownloadRequest) -> io::Result<WireResponse> {
        (self.0)(url, request)
    }
}

pub fn serve(
    easylist: &'static str,
    easyprivacy: &'static str,
) -> FnDownload<impl Fn(&str, &DownloadRequest) -> io::Result<WireResponse> + Send + Sync> {
    FnDownload(move |url: &str, _: &DownloadRequest| {
        Ok(response(if url == LISTS[0].url {
            easylist
        } else {
            easyprivacy
        }))
    })
}

pub fn offline()
-> FnDownload<impl Fn(&str, &DownloadRequest) -> io::Result<WireResponse> + Send + Sync> {
    FnDownload(|_: &str, _: &DownloadRequest| Err(io::Error::other("offline")))
}

/// An `AdBlocker` with the fixture lists installed, and its cache directory.
pub fn loaded(enabled: bool, allowed: &[&str]) -> (AdBlocker, tempfile::TempDir) {
    let allowed: Vec<String> = allowed.iter().map(|host| (*host).to_owned()).collect();
    let (blocker, dropped) = AdBlocker::new(enabled, &allowed);
    assert_eq!(dropped, 0);
    let cache = tempfile::tempdir().unwrap();
    let report = update(&blocker, cache.path(), &serve(EASYLIST, EASYPRIVACY));
    assert!(report.failures.is_empty(), "{report:?}");
    assert!(blocker.has_engine());
    (blocker, cache)
}

pub fn update(
    blocker: &AdBlocker,
    cache: &std::path::Path,
    download: &dyn Download,
) -> UpdateReport {
    update_once(blocker, cache, false, download, &mut None, RETRY)
}
