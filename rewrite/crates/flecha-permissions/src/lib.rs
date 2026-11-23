//! Per-origin site permissions and the host-owned permission prompt queue.
//!
//! CEF Alloy with windowless rendering shows no permission UI of its own: it denies media
//! requests and ignores the rest unless the host answers `OnRequestMediaAccessPermission` and
//! `OnShowPermissionPrompt`. [`PermissionManager`] decides each request from saved and one-time
//! decisions or queues a prompt for the GPUI shell, and answers every queued request exactly once.
//!
//! The site panel's names and semantics follow `chromium/shell/siteControls.ts`: `media` covers
//! the camera and the microphone, and setting a permission back to "ask" forgets the decision.
//!
//! The host maps CEF's request bitmasks onto [`PermissionSet`] and denies any bit with no
//! [`PermissionKind`], such as desktop capture. A [`Verdict::Allow`] grants every requested kind;
//! a [`Verdict::Block`] grants none.

mod kind;
mod manager;
mod origin;
mod snapshot;

pub use kind::{Decision, PermissionKind, PermissionSet, SiteControl, Verdict};
pub use manager::{
    Completion, Evaluation, Partition, PermissionError, PermissionManager, PromptOutcome,
    PromptView, RequestId, SiteSettings, TabId,
};
pub use origin::{Origin, OriginError};
pub use snapshot::{
    DecodeError, Decoded, Loaded, SCHEMA_VERSION, Snapshot, SnapshotError, SnapshotFile,
};
