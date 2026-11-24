use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, ensure};
use flecha_cef::{Engine, PermissionPrompt};
use flecha_permissions::{PermissionKind, PromptOutcome, RequestId, Snapshot};
use gpui::{
    ClickEvent, Context, Div, ParentElement as _, StatefulInteractiveElement as _, Styled as _,
    div, prelude::FluentBuilder as _, rgb,
};

use crate::view::{Shell, button};
use crate::{Action, Controller, dispatch};

pub(crate) struct PermissionStore {
    path: PathBuf,
    observed: Option<Vec<u8>>,
    error: Option<anyhow::Error>,
}

impl PermissionStore {
    pub(crate) fn open(profile: &Path) -> Self {
        Self {
            path: profile.join("permissions.json"),
            observed: None,
            error: None,
        }
    }

    pub(crate) fn restore(&mut self, engine: &mut Engine) {
        self.error = self.load(engine).context("Load saved permissions").err();
    }

    pub(crate) fn error(&self) -> Option<&anyhow::Error> {
        self.error.as_ref()
    }

    fn load(&mut self, engine: &mut Engine) -> Result<()> {
        self.observed = read_snapshot(&self.path)?;
        let snapshot = match self.observed.as_deref() {
            None => Snapshot::default(),
            Some(bytes) => {
                let decoded = Snapshot::decode(std::str::from_utf8(bytes)?)?;
                ensure!(
                    decoded.skipped_entries == 0,
                    "Permission snapshot has invalid entries"
                );
                decoded.snapshot
            }
        };
        engine.restore_permissions(snapshot)
    }

    fn save(&mut self, snapshot: &Snapshot) -> Result<()> {
        let directory = self
            .path
            .parent()
            .context("Permission snapshot has no parent")?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("permissions.json.lock"))?;
        lock.lock()?;
        ensure!(
            read_snapshot(&self.path)? == self.observed,
            "Permission snapshot changed on disk. Restart Flecha to load it"
        );
        let mut temp = tempfile::NamedTempFile::new_in(directory)?;
        let encoded = snapshot.encode().into_bytes();
        temp.write_all(&encoded)?;
        temp.as_file().sync_all()?;
        temp.persist(&self.path)?;
        self.observed = Some(encoded);
        #[cfg(unix)]
        File::open(directory)?.sync_all()?;
        Ok(())
    }
}

fn read_snapshot(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(info) => ensure!(info.is_file(), "Permission snapshot must be a regular file"),
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    Ok(Some(fs::read(path)?))
}

impl Controller {
    pub(crate) fn poll_permissions(&mut self) -> bool {
        let mut changed = false;
        while self.engine.next_permission_event().is_some() {
            changed = true;
        }
        if self.permissions.error().is_none()
            && let Err(error) = self.save_permissions()
        {
            self.status = error.to_string();
            changed = true;
        }
        changed
    }

    pub(crate) fn save_permissions(&mut self) -> Result<()> {
        if !self.engine.permissions_dirty() || self.permissions.error().is_some() {
            return Ok(());
        }
        if let Err(error) = self.permissions.save(&self.engine.permission_snapshot()) {
            let error = error.context("Could not save permissions. Restart Flecha to retry");
            let message = format!("{error:#}");
            self.permissions.error = Some(error);
            anyhow::bail!(message);
        }
        self.engine.mark_permissions_saved();
        Ok(())
    }

    pub(crate) fn writable_permissions(&self) -> Result<()> {
        ensure!(
            self.permissions.error().is_none(),
            "Saved permissions are read-only. See Site Controls for details"
        );
        Ok(())
    }

    pub(crate) fn resolve_permission(
        &mut self,
        request: RequestId,
        outcome: PromptOutcome,
    ) -> Result<()> {
        if matches!(
            outcome,
            PromptOutcome::AllowAlways | PromptOutcome::BlockAlways
        ) {
            self.writable_permissions()?;
        }
        self.engine.resolve_permission(request, outcome)?;
        self.save_permissions()
    }
}

pub(crate) fn permission_label(kind: PermissionKind) -> &'static str {
    match kind {
        PermissionKind::Geolocation => "Location",
        PermissionKind::Camera => "Camera",
        PermissionKind::Microphone => "Microphone",
        PermissionKind::Notifications => "Notifications",
        PermissionKind::ClipboardRead => "Clipboard",
    }
}

impl Shell {
    pub(crate) fn permission_banner(
        &self,
        prompt: PermissionPrompt,
        writable: bool,
        cx: &Context<Self>,
    ) -> Div {
        let kinds = prompt
            .ask
            .iter()
            .map(permission_label)
            .collect::<Vec<_>>()
            .join(", ");
        let mut banner = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap_1()
            .p_2()
            .text_sm()
            .bg(rgb(0x273b46))
            .child(div().child(format!("{} requests {kinds}", prompt.origin)))
            .when(!writable, |banner| {
                banner.child("Saved permissions are read-only.")
            });
        let mut buttons = div().flex().flex_wrap().gap_1();
        for (id, label, outcome, enabled) in [
            ("allow-once", "Allow once", PromptOutcome::AllowOnce, true),
            (
                "allow-always",
                "Always allow",
                PromptOutcome::AllowAlways,
                writable,
            ),
            ("block-once", "Block once", PromptOutcome::BlockOnce, true),
            (
                "block-always",
                "Always block",
                PromptOutcome::BlockAlways,
                writable,
            ),
            ("dismiss", "Dismiss", PromptOutcome::Dismiss, true),
        ] {
            let action = Action::ResolvePermission(prompt.request, outcome);
            buttons = buttons.child(
                button(format!("{id}-{:?}", prompt.request), label).when_else(
                    enabled,
                    |button| {
                        button.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            dispatch(&this.state, &this.id, action.clone(), cx);
                        }))
                    },
                    |button| button.opacity(0.5).cursor_default(),
                ),
            );
        }
        banner = banner.child(buttons);
        banner
    }
}
