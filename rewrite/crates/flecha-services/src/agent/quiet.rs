//! Keeps page contents and tokens out of the logs. rmcp logs whole JSON-RPC messages at its
//! DEBUG and TRACE levels, and tool results carry page text, HTML and addresses, so on the server
//! thread rmcp's events below WARN go nowhere. Everything else reaches the app's subscriber.

use tracing::dispatcher::{self, DefaultGuard, Dispatch};
use tracing::span::{Attributes, Id, Record};
use tracing::subscriber::Interest;
use tracing::{Event, Level, Metadata, Subscriber};
use tracing_core::span::Current;

/// Wraps the thread's current subscriber for as long as the guard lives.
pub(crate) fn install() -> DefaultGuard {
    let quiet = dispatcher::get_default(|inner| {
        Dispatch::new(Quiet {
            inner: inner.clone(),
        })
    });
    dispatcher::set_default(&quiet)
}

fn muted(metadata: &Metadata<'_>) -> bool {
    metadata.target().starts_with("rmcp") && *metadata.level() > Level::WARN
}

struct Quiet {
    inner: Dispatch,
}

impl Subscriber for Quiet {
    fn register_callsite(&self, _: &'static Metadata<'static>) -> Interest {
        // Ask `enabled` every time: other threads share each callsite's cached interest.
        Interest::sometimes()
    }

    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        !muted(metadata) && self.inner.enabled(metadata)
    }

    fn event_enabled(&self, event: &Event<'_>) -> bool {
        !muted(event.metadata())
    }

    fn new_span(&self, span: &Attributes<'_>) -> Id {
        self.inner.new_span(span)
    }

    fn record(&self, span: &Id, values: &Record<'_>) {
        self.inner.record(span, values);
    }

    fn record_follows_from(&self, span: &Id, follows: &Id) {
        self.inner.record_follows_from(span, follows);
    }

    fn event(&self, event: &Event<'_>) {
        if !muted(event.metadata()) {
            self.inner.event(event);
        }
    }

    fn enter(&self, span: &Id) {
        self.inner.enter(span);
    }

    fn exit(&self, span: &Id) {
        self.inner.exit(span);
    }

    fn clone_span(&self, id: &Id) -> Id {
        self.inner.clone_span(id)
    }

    fn try_close(&self, id: Id) -> bool {
        self.inner.try_close(id)
    }

    fn current_span(&self) -> Current {
        self.inner.current_span()
    }
}
