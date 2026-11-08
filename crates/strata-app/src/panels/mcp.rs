//! Connect an AI assistant over MCP to edit this map live.

use egui::RichText;

use crate::mcp_link::{self, LinkStatus};
use crate::state::EditorState;
use crate::theme::{self, Tokens};

pub use crate::mcp_link::McpState;

const INSTALL: &str = "cargo install --path crates/strata-mcp";
const CLAUDE_CODE: &str = "claude mcp add strata -- strata-mcp";
const CLAUDE_DESKTOP: &str = r#"{
  "mcpServers": {
    "strata": { "command": "strata-mcp" }
  }
}"#;
const HTTP_RUN: &str = "strata-mcp --http 127.0.0.1:7341";
const HTTP_URL: &str = "http://127.0.0.1:7341/mcp";

pub fn show(ui: &mut egui::Ui, state: &mut EditorState) {
    let tk = theme::tokens(ui.ctx());
    let ctx = ui.ctx().clone();
    let now = ctx.input(|i| i.time);
    let m = &mut state.ui.mcp;

    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        status_pill(ui, &tk, &m.status);
        if let Some(s) = m.retry_in(now) {
            ui.label(RichText::new(format!("Retrying in {s:.0} s")).color(tk.text_dim));
        }
    });
    if let (Some(e), false) = (&m.last_error, m.is_connected()) {
        ui.label(RichText::new(e).color(tk.text_dim));
    }
    if m.is_connected() && !m.settings.allow_edits {
        ui.label(
            RichText::new(
                "AI edits are off. The assistant can read the map, but its changes are refused.",
            )
            .color(tk.warn),
        );
    }
    ui.label(
        RichText::new("Let an AI assistant read and edit this map through strata-mcp. Its changes are undoable and marked AI in History.")
            .color(tk.text_dim),
    );
    ui.add_space(4.0);

    egui::Grid::new("mcp-connection")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Server");
            ui.add(
                egui::TextEdit::singleline(&mut m.settings.url)
                    .hint_text(strata_core::ops::link::DEFAULT_WS_URL)
                    .desired_width(240.0),
            );
            ui.end_row();
            ui.label("Token");
            ui.horizontal(|ui| {
                let hint = if cfg!(target_arch = "wasm32") {
                    "Paste the token strata-mcp printed"
                } else {
                    "Read from strata-mcp automatically"
                };
                ui.add(
                    egui::TextEdit::singleline(&mut m.settings.token)
                        .password(!m.show_token)
                        .hint_text(hint)
                        .desired_width(240.0),
                );
                if ui
                    .small_button(if m.show_token { "Hide" } else { "Show" })
                    .clicked()
                {
                    m.show_token = !m.show_token;
                }
            });
            ui.end_row();
        });

    ui.horizontal(|ui| {
        match m.status {
            LinkStatus::Disconnected => {
                if ui.button("Connect").clicked() {
                    mcp_link::connect(&ctx, m);
                }
            }
            LinkStatus::Connecting => {
                if ui.button("Cancel").clicked() {
                    mcp_link::disconnect(m);
                }
            }
            LinkStatus::Connected { .. } => {
                if ui.button("Disconnect").clicked() {
                    mcp_link::disconnect(m);
                }
            }
        }
        ui.checkbox(&mut m.settings.auto_connect, "Connect automatically");
    });
    ui.checkbox(&mut m.settings.allow_edits, "Allow AI edits")
        .on_hover_text(
        "When off, the assistant can still read the map, but every change it asks for is refused.",
    );

    ui.separator();
    ui.horizontal(|ui| {
        ui.strong("Activity");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if !m.log.is_empty() && ui.small_button("Clear").clicked() {
                m.log.clear();
                m.selected = None;
            }
        });
    });
    if m.log.is_empty() {
        ui.label(
            RichText::new(
                "No AI activity yet. Once connected, ask your assistant to call editor_status.",
            )
            .color(tk.text_dim),
        );
    }
    let mut toggle = None;
    for e in m.log.iter().rev() {
        let open = m.selected == Some(e.seq);
        let row = ui
            .horizontal(|ui| {
                ui.add_sized(
                    [44.0, 18.0],
                    egui::Label::new(RichText::new(ago(now - e.time)).color(tk.text_dim).small()),
                );
                let (word, color) = if e.ok {
                    ("Done", tk.ok)
                } else {
                    ("Failed", tk.error)
                };
                ui.label(RichText::new(word).color(color).small());
                ui.label(RichText::new(&e.tool).monospace());
                if let (false, Some(err)) = (open, &e.error) {
                    ui.add(
                        egui::Label::new(RichText::new(err).color(tk.text_dim).small()).truncate(),
                    );
                }
            })
            .response
            .interact(egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if row.clicked() {
            toggle = Some(e.seq);
        }
        if open {
            egui::Frame::new()
                .fill(tk.bg_deep)
                .corner_radius(4.0)
                .inner_margin(6.0)
                .show(ui, |ui| {
                    if let Some(err) = &e.error {
                        ui.label(RichText::new(err).color(tk.error));
                    }
                    json_block(ui, "Request", &e.request);
                    json_block(ui, if e.ok { "Response" } else { "Error" }, &e.response);
                });
        }
    }
    if let Some(seq) = toggle {
        m.selected = if m.selected == Some(seq) {
            None
        } else {
            Some(seq)
        };
    }

    ui.separator();
    egui::CollapsingHeader::new("Set up an AI client")
        .default_open(!m.is_connected())
        .show(ui, |ui| {
            setup(ui, &tk);
        });
}

fn status_pill(ui: &mut egui::Ui, tk: &Tokens, status: &LinkStatus) {
    let (text, color) = match status {
        LinkStatus::Disconnected => ("Not connected".to_owned(), tk.text_dim),
        LinkStatus::Connecting => ("Connecting…".to_owned(), tk.text_dim),
        LinkStatus::Connected { server, version } => {
            (format!("Connected to {server} v{version}"), tk.accent)
        }
    };
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, color))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.0, color);
                ui.label(RichText::new(text).color(color));
            });
        });
}

fn json_block(ui: &mut egui::Ui, title: &str, text: &str) {
    if text.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).small().strong());
        if ui.small_button("Copy").clicked() {
            ui.ctx().copy_text(text.to_owned());
        }
    });
    egui::ScrollArea::vertical()
        .id_salt(("mcp-json", title, text.len()))
        .max_height(180.0)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut &*text)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(2),
            );
        });
}

fn copy_row(ui: &mut egui::Ui, tk: &Tokens, label: &str, text: &str) {
    ui.label(label);
    ui.horizontal(|ui| {
        egui::Frame::new()
            .fill(tk.bg_deep)
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(6, 3))
            .show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(text).monospace()).wrap());
            });
        if ui.small_button("Copy").clicked() {
            ui.ctx().copy_text(text.to_owned());
        }
    });
    ui.add_space(4.0);
}

fn setup(ui: &mut egui::Ui, tk: &Tokens) {
    let dim = |s: &str| RichText::new(s).color(tk.text_dim);
    copy_row(
        ui,
        tk,
        "1. Install strata-mcp from the Strata source folder:",
        INSTALL,
    );
    copy_row(ui, tk, "2. Add it to Claude Code:", CLAUDE_CODE);
    copy_row(
        ui,
        tk,
        "Or to Claude Desktop, in claude_desktop_config.json:",
        CLAUDE_DESKTOP,
    );
    copy_row(
        ui,
        tk,
        "Other MCP clients can use Streamable HTTP. Run:",
        HTTP_RUN,
    );
    copy_row(ui, tk, "and connect them to:", HTTP_URL);
    ui.label("3. Start your AI client; it starts strata-mcp, which listens for this editor.");
    #[cfg(target_arch = "wasm32")]
    {
        ui.label(dim(
            "strata-mcp prints a token in its log. Paste it into Token above and click Connect.              Opening Strata with ?mcp=TOKEN in the address fills it in for you.",
        ));
        ui.label(dim(
            "Keep this tab in front while the assistant works; browsers pause background tabs.",
        ));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let path = strata_core::ops::link::token_file_path()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        ui.label(dim(&format!(
            "Strata reads the token strata-mcp saves at {path}, so you can leave Token empty and click Connect."
        )));
    }
    ui.label(dim("Without a connected editor, the assistant edits its own copy of a map and saves it to disk."));
}

fn ago(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    match s {
        0..=4 => "now".into(),
        5..=59 => format!("{s} s"),
        60..=3599 => format!("{} min", s / 60),
        _ => format!("{} h", s / 3600),
    }
}
