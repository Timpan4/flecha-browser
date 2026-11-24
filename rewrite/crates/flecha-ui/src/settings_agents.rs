use std::time::Duration;

use async_io::Timer;
use flecha_core::AgentPreference;
use gpui::{
    ClickEvent, ClipboardItem, Context, Div, FontWeight, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _, px, relative,
};

use super::{SettingsPanel, card, muted, row, settings_button};
use crate::theme::palette;
use crate::tokens;
use crate::view::{SETTINGS_SWITCH, Shell, switch};

/// How long a copy button says "Copied" (ui/settings.ts:557).
const COPIED: Duration = Duration::from_millis(1200);

/// The pane's opening paragraph (settings.html:270-273).
pub(super) const LEAD: &str = "Let Claude Code, Cursor and other MCP clients see and drive your tabs: read pages and their console, take screenshots, click, type and run JavaScript. You'll see what an agent is doing in the sidebar.";

/// The pane's rows and their descriptions (settings.html:277-305), shared with Settings search.
pub(super) const SWITCH: (&str, &str) = (
    "Let coding agents use Flecha",
    "Full access. Only programs on this computer that have the token can connect.",
);
pub(super) const CLAUDE_CODE: &str = "Claude Code";
pub(super) const OTHER_CLIENTS: (&str, &str) =
    ("Other MCP clients", "JSON for Cursor, Windsurf and others.");
pub(super) const TOKEN: (&str, &str) = (
    "Token",
    "A new token disconnects every agent using the old one.",
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Setup {
    Command,
    Config,
}

/// The command that adds Flecha to Claude Code (ui/settings.ts:541-543).
fn claude_command(endpoint: &str, token: &str) -> String {
    format!(
        "claude mcp add --transport http flecha {endpoint} --header \"Authorization: Bearer {token}\""
    )
}

/// The `mcpServers` entry other clients read (ui/settings.ts:545-551), indented by two spaces as
/// `JSON.stringify(…, null, 2)` writes it.
fn agent_config(endpoint: &str, token: &str) -> String {
    #[derive(serde::Serialize)]
    struct Config<'a> {
        #[serde(rename = "mcpServers")]
        servers: Servers<'a>,
    }
    #[derive(serde::Serialize)]
    struct Servers<'a> {
        flecha: Server<'a>,
    }
    #[derive(serde::Serialize)]
    struct Server<'a> {
        r#type: &'a str,
        url: &'a str,
        headers: Headers,
    }
    #[derive(serde::Serialize)]
    struct Headers {
        #[serde(rename = "Authorization")]
        authorization: String,
    }
    let config = Config {
        servers: Servers {
            flecha: Server {
                r#type: "http",
                url: endpoint,
                headers: Headers {
                    authorization: format!("Bearer {token}"),
                },
            },
        },
    };
    serde_json::to_string_pretty(&config).expect("the config serializes")
}

/// A `.row-text`: the setting's name over its description (ui/settings.css:250-270).
fn row_text(title: &'static str, detail: Div) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .gap_0p5()
        .child(div().font_weight(FontWeight::MEDIUM).child(title))
        .child(detail)
}

/// `.button:disabled` (ui/settings.css:302-304).
fn disabled(button: gpui::Stateful<Div>, disabled: bool) -> gpui::Stateful<Div> {
    button.when(disabled, |button| {
        button
            .opacity(tokens::SETTINGS_BUTTON_DISABLED_OPACITY)
            .cursor_default()
    })
}

impl Shell {
    /// The Coding Agents pane (settings.html:269-306).
    pub(super) fn agent_settings(&self, panel: &SettingsPanel, cx: &Context<Self>) -> Div {
        let palette = palette();
        let state = self.state.borrow();
        let enabled = state.model.core.settings.agents_enabled();
        let endpoint = state.agents.endpoint();
        let lead = div()
            .mb(px(tokens::LEAD_BOTTOM))
            .text_color(palette.text_2)
            .line_height(relative(tokens::LEAD_LINE_HEIGHT))
            .child(LEAD);
        let switch_row = row()
            .justify_between()
            .flex_nowrap()
            .child(row_text(SWITCH.0, muted(SWITCH.1)))
            .child(
                switch("agent-enabled", enabled, SWITCH.0, SETTINGS_SWITCH).on_click(cx.listener(
                    move |this, _: &ClickEvent, _, cx| {
                        let result = this.state.borrow_mut().edit_settings(|settings| {
                            settings.agent = Some(AgentPreference { enabled: !enabled });
                            Ok(String::new())
                        });
                        if let Some(panel) = this.settings_mut() {
                            panel.result(result);
                        }
                        cx.refresh_windows();
                    },
                )),
            );
        let content = div()
            .flex()
            .flex_col()
            .child(lead)
            .child(card().mb(px(tokens::SETTINGS_CARD_GAP)).child(switch_row));
        if !enabled {
            return content;
        }
        // The endpoint, or why there is none (ui/settings.ts:605-607).
        let (status, failed): (SharedString, bool) = match (&endpoint, state.agents.error()) {
            (Some(endpoint), _) => (endpoint.clone().into(), false),
            (None, Some(error)) => (error.to_owned().into(), true),
            (None, None) if state.isolated => (
                "This launch keeps coding agents out. Start Flecha normally to connect one.".into(),
                true,
            ),
            (None, None) => ("Starting…".into(), false),
        };
        let status = muted(status)
            .font_family(tokens::MONO)
            .text_size(px(tokens::TEXT_SM * tokens::MONO_SHARE))
            .map(|status| {
                if failed {
                    status.text_color(palette.red)
                } else {
                    status.truncate()
                }
            });
        let copy_button = |kind: Setup, id: &'static str, label: &'static str| {
            let shown = if panel
                .copied
                .as_ref()
                .is_some_and(|(copied, _)| *copied == kind)
            {
                "Copied"
            } else {
                label
            };
            disabled(settings_button(id, shown), endpoint.is_none()).on_click(
                cx.listener(move |this, _: &ClickEvent, _, cx| this.copy_agent_setup(kind, cx)),
            )
        };
        let new_token =
            settings_button("agent-new-token", "New Token").on_click(cx.listener(
                |this, _: &ClickEvent, _, cx| {
                    let result = this.state.borrow().agents.renew_token();
                    if let Some(panel) = this.settings_mut() {
                        panel.result(result.map(|()| String::new()).map_err(|error| {
                            anyhow::Error::new(error).context("Make a new token")
                        }));
                    }
                    cx.notify();
                },
            ));
        content.child(
            card()
                .child(
                    row()
                        .justify_between()
                        .flex_nowrap()
                        .child(row_text(CLAUDE_CODE, status))
                        .child(copy_button(
                            Setup::Command,
                            "agent-copy-command",
                            "Copy Command",
                        )),
                )
                .child(
                    row()
                        .justify_between()
                        .flex_nowrap()
                        .child(row_text(OTHER_CLIENTS.0, muted(OTHER_CLIENTS.1)))
                        .child(copy_button(
                            Setup::Config,
                            "agent-copy-config",
                            "Copy Config",
                        )),
                )
                .child(
                    row()
                        .justify_between()
                        .flex_nowrap()
                        .child(row_text(TOKEN.0, muted(TOKEN.1)))
                        .child(new_token),
                ),
        )
    }

    fn copy_agent_setup(&mut self, kind: Setup, cx: &mut Context<Self>) {
        let text = {
            let state = self.state.borrow();
            let (Some(endpoint), Some(token)) = (state.agents.endpoint(), state.agents.token())
            else {
                return;
            };
            match kind {
                Setup::Command => claude_command(&endpoint, &token),
                Setup::Config => agent_config(&endpoint, &token),
            }
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        let reset = cx.spawn(async move |shell, cx| {
            Timer::after(COPIED).await;
            let _ = shell.update(cx, |this, cx| {
                if let Some(panel) = this.settings_mut() {
                    panel.copied = None;
                    cx.notify();
                }
            });
        });
        if let Some(panel) = self.settings_mut() {
            panel.copied = Some((kind, reset));
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_copied_setup_matches_what_the_shell_copies() {
        let endpoint = "http://127.0.0.1:7390/mcp";
        assert_eq!(
            claude_command(endpoint, "t0k"),
            "claude mcp add --transport http flecha http://127.0.0.1:7390/mcp --header \"Authorization: Bearer t0k\""
        );
        assert_eq!(
            agent_config(endpoint, "t0k"),
            r#"{
  "mcpServers": {
    "flecha": {
      "type": "http",
      "url": "http://127.0.0.1:7390/mcp",
      "headers": {
        "Authorization": "Bearer t0k"
      }
    }
  }
}"#
        );
    }
}
