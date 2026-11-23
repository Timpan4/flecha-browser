//! Loading and refreshing the lists on the updater's own thread, never on GPUI's foreground
//! thread or a CEF thread. It needs no GPUI timer: one thread sleeps until the next list is due.
//!
//! The schedule is build-adblock.mjs's: a list is downloaded again once its cached copy is
//! [`MAX_AGE`] old. A launch first installs the serialized engine for the cached lists, whatever
//! their age, so blocking starts without parsing; then any list that is due is refreshed.

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::mpsc::{self, RecvTimeoutError, Sender},
    thread::JoinHandle,
    time::{Duration, SystemTime},
};

use crate::{
    blocker::AdBlocker,
    engine::{
        CachedEngineError, EngineKey, SkippedLine, build_engine, engine_key, load_engine,
        save_engine,
    },
    fetch::{Download, ListFailure, ListSource, ListWarning, cache_age, cache_file, fetch_lists},
    lists::{LISTS, MAX_AGE},
};

pub struct UpdaterConfig {
    /// Where the lists and the serialized engine are kept. The app owns the path.
    pub cache_dir: PathBuf,
    /// When to try again after a download failed. build-adblock.mjs has no retry: it runs
    /// again at the next build. The app chooses this value.
    pub retry_after_failure: Duration,
}

/// What happened to the engine in one pass.
#[derive(Debug)]
pub enum EngineOutcome {
    /// The installed engine already matches the lists.
    Unchanged,
    /// Read from `engine.dat`.
    Deserialized,
    /// Parsed from the lists, because the serialized copy was missing, stale or refused.
    Parsed {
        cached: CachedEngineError,
        skipped_lines: Vec<SkippedLine>,
        /// Writing the new `engine.dat` failed; the next launch parses again.
        save_error: Option<String>,
    },
    /// The lists could not be loaded; the previous engine, if any, stays.
    NotBuilt,
}

/// One pass of the updater, for the app's log.
#[derive(Debug)]
pub struct UpdateReport {
    pub warnings: Vec<ListWarning>,
    pub failures: Vec<ListFailure>,
    pub engine: EngineOutcome,
    /// How long until the next pass.
    pub next_check: Duration,
}

/// Installs the serialized engine for the cached lists, if both exist and match. No network.
pub fn warm_start(blocker: &AdBlocker, cache: &Path) -> Result<EngineKey, CachedEngineError> {
    let mut texts = Vec::new();
    for list in LISTS {
        match fs::read_to_string(cache_file(cache, list.id)) {
            Ok(text) => texts.push((list.id, text)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(CachedEngineError::Missing);
            }
            Err(error) => return Err(CachedEngineError::Read(error)),
        }
    }
    let key = engine_key(texts.iter().map(|(id, text)| (*id, text.as_str())));
    blocker.install(load_engine(cache, &key)?);
    Ok(key)
}

/// Fetches what is due, and installs a new engine when the list text changed.
pub fn update_once(
    blocker: &AdBlocker,
    cache: &Path,
    refresh: bool,
    download: &dyn Download,
    installed: &mut Option<EngineKey>,
    retry_after_failure: Duration,
) -> UpdateReport {
    let fetched = fetch_lists(cache, refresh, download);
    let lists = match fetched.lists {
        Ok(lists) => lists,
        Err(failures) => {
            return UpdateReport {
                warnings: fetched.warnings,
                failures,
                engine: EngineOutcome::NotBuilt,
                next_check: retry_after_failure,
            };
        }
    };
    let texts = || lists.iter().map(|list| (list.id, list.text.as_str()));
    let key = engine_key(texts());
    let engine = if *installed == Some(key) {
        EngineOutcome::Unchanged
    } else {
        let outcome = match load_engine(cache, &key) {
            Ok(engine) => {
                blocker.install(engine);
                EngineOutcome::Deserialized
            }
            Err(cached) => {
                let (engine, skipped_lines) = build_engine(texts());
                let save_error = save_engine(cache, &key, &engine)
                    .err()
                    .map(|e| e.to_string());
                blocker.install(engine);
                EngineOutcome::Parsed {
                    cached,
                    skipped_lines,
                    save_error,
                }
            }
        };
        *installed = Some(key);
        outcome
    };
    // A failed refresh leaves the old copy's time, which is already due: wait instead.
    let failed_refresh = lists
        .iter()
        .any(|list| list.source == ListSource::CacheAfterFailedRefresh);
    let now = SystemTime::now();
    let due = lists
        .iter()
        .map(|list| {
            cache_age(&cache_file(cache, list.id), now).map(|age| MAX_AGE.saturating_sub(age))
        })
        .collect::<Option<Vec<_>>>()
        .and_then(|waits| waits.into_iter().min());
    let next_check = match due {
        Some(due) if !failed_refresh => due,
        _ => retry_after_failure,
    };
    UpdateReport {
        warnings: fetched.warnings,
        failures: Vec::new(),
        engine,
        next_check,
    }
}

enum Command {
    RefreshNow,
}

/// The updater thread. Dropping it stops the thread after its current pass.
pub struct Updater {
    commands: Sender<Command>,
    thread: JoinHandle<()>,
}

impl Updater {
    /// Starts the thread and returns at once. `on_report` runs on the updater thread after the
    /// warm start and after every pass.
    pub fn spawn(
        blocker: AdBlocker,
        config: UpdaterConfig,
        download: Box<dyn Download>,
        mut on_report: impl FnMut(UpdateReport) + Send + 'static,
    ) -> io::Result<Self> {
        let (commands, received) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("flecha-adblock-updater".into())
            .spawn(move || {
                let cache = config.cache_dir.as_path();
                let mut installed = warm_start(&blocker, cache).ok();
                if installed.is_some() {
                    on_report(UpdateReport {
                        warnings: Vec::new(),
                        failures: Vec::new(),
                        engine: EngineOutcome::Deserialized,
                        next_check: Duration::ZERO,
                    });
                }
                let mut refresh = false;
                loop {
                    let report = update_once(
                        &blocker,
                        cache,
                        refresh,
                        download.as_ref(),
                        &mut installed,
                        config.retry_after_failure,
                    );
                    let wait = report.next_check;
                    on_report(report);
                    refresh = match received.recv_timeout(wait) {
                        Ok(Command::RefreshNow) => true,
                        Err(RecvTimeoutError::Timeout) => false,
                        Err(RecvTimeoutError::Disconnected) => break,
                    };
                }
            })?;
        Ok(Self { commands, thread })
    }

    /// Downloads both lists now, whatever their age.
    pub fn refresh_now(&self) {
        let _ = self.commands.send(Command::RefreshNow);
    }

    /// Stops the thread and waits for its current pass. A pass can take up to
    /// [`crate::DOWNLOAD_TIMEOUT`] plus parsing, so don't call this from a UI thread.
    pub fn shutdown(self) {
        let Self { commands, thread } = self;
        drop(commands);
        let _ = thread.join();
    }
}
