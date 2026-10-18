//! Parterre keeps no merge tool setting of its own: git's config names the tool, and one picked
//! without *Remember this choice* lasts until parterre closes.

use std::path::PathBuf;
use std::sync::mpsc;

use eframe::egui::{self, RichText, ViewportId};
use parterre_core::branches::command_text;
use parterre_core::git::Git;
use parterre_core::merge_tool::{self, Configured, Detected, Tool};

use crate::dialogs::{self, Answer, Dialog};
use crate::worker;

#[derive(Clone, Debug)]
struct File {
    root: PathBuf,
    path: String,
    opener: ViewportId,
}

#[derive(Debug, Default)]
pub struct MergeTools {
    picked_for_session: Option<Tool>,
    detecting: Option<(File, mpsc::Receiver<Result<Detected, String>>)>,
    picker: Option<Picker>,
    pub error: Option<String>,
}

#[derive(Debug)]
struct Picker {
    file: File,
    detected: Detected,
    chosen: usize,
    remember: bool,
    just_opened: bool,
    terminal_error: Option<String>,
}

impl MergeTools {
    pub fn open(&mut self, ctx: &egui::Context, root: PathBuf, path: String, opener: ViewportId) {
        crate::usage::action(crate::usage::Action::OpenMergeTool);
        let file = File { root, path, opener };
        if let Some(tool) = self.picked_for_session.clone() {
            self.start(&file, &tool);
            return;
        }
        let root = file.root.clone();
        let rx = worker::spawn(ctx, move || {
            merge_tool::detect(&Git::new(root)).map_err(|e| e.to_string())
        });
        self.detecting = Some((file, rx));
    }

    fn start(&mut self, file: &File, tool: &Tool) {
        if let Err(e) = merge_tool::open(&file.root, tool, &file.path) {
            self.error = Some(format!("Could not open {}: {e}", file.path));
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        if let Some((file, rx)) = self.detecting.take() {
            match rx.try_recv() {
                Ok(Ok(detected)) => match detected.usable() {
                    Some(tool) => self.start(&file, &tool),
                    None => self.picker = Some(Picker::new(file, detected)),
                },
                Ok(Err(e)) => self.error = Some(format!("Could not find a merge tool: {e}")),
                Err(mpsc::TryRecvError::Empty) => self.detecting = Some((file, rx)),
                Err(mpsc::TryRecvError::Disconnected) => {}
            }
        }
        let Some(mut picker) = self.picker.take() else {
            return;
        };
        match picker.show(ctx) {
            Answer::Open => self.picker = Some(picker),
            Answer::Cancel => {}
            Answer::Primary => self.pick(picker),
        }
    }

    fn pick(&mut self, picker: Picker) {
        let Some(tool) = picker.detected.installed.get(picker.chosen).cloned() else {
            return;
        };
        if picker.remember {
            let git = Git::new(&picker.file.root);
            if let Err(e) = merge_tool::remember(&git, &tool, picker.terminal_tool_configured()) {
                self.error = Some(format!("Could not remember {}: {e}", tool.name));
            }
        }
        self.start(&picker.file, &tool);
        self.picked_for_session = Some(tool);
    }
}

impl Picker {
    fn new(file: File, detected: Detected) -> Picker {
        Picker {
            file,
            detected,
            chosen: 0,
            remember: false,
            just_opened: true,
            terminal_error: None,
        }
    }

    fn terminal_tool_configured(&self) -> bool {
        matches!(
            self.detected.configured,
            Configured::Unusable { terminal: true, .. }
        )
    }

    fn show(&mut self, ctx: &egui::Context) -> Answer {
        let why = match &self.detected.configured {
            Configured::None => "No merge tool configured".to_owned(),
            Configured::Unusable { why, .. } => why.clone(),
            Configured::Usable(name) => format!("{name} is configured"),
        };
        let terminal = self.terminal_tool_configured();
        let tools = &self.detected.installed;
        let chosen = &mut self.chosen;
        let remember = &mut self.remember;
        let root = &self.file.root;
        let terminal_error = &mut self.terminal_error;
        let shown = Dialog::new("merge-tool", "Merge tool")
            .screen(crate::usage::Screen::MergeTool)
            .modal()
            .opener(self.file.opener)
            .raise(self.just_opened)
            .show(ctx, |ui| {
                ui.label(format!("{why}."));
                ui.add_space(6.0);
                if tools.is_empty() {
                    ui.label("No merge tool found. Install one, then:");
                    ui.label(RichText::new("git config --global merge.tool <tool>").monospace());
                    ui.add_space(6.0);
                    if ui.button("Open terminal").clicked() {
                        crate::usage::action(crate::usage::Action::OpenTerminal);
                        *terminal_error = crate::file_manager::open_terminal(root).err();
                    }
                    if let Some(e) = terminal_error {
                        ui.label(RichText::new(e.as_str()).color(ui.visuals().error_fg_color));
                    }
                    return dialogs::close_only(ui);
                }
                dialogs::fields(ui, |ui| {
                    for (i, tool) in tools.iter().enumerate() {
                        ui.radio_value(chosen, i, RichText::new(&tool.name).monospace());
                    }
                });
                ui.add_space(4.0);
                let commands: Vec<String> =
                    merge_tool::remember_commands(&tools[*chosen], terminal)
                        .iter()
                        .map(|args| command_text(args))
                        .collect();
                ui.checkbox(remember, "Remember this choice")
                    .on_hover_text(commands.join("\n"));
                dialogs::actions(ui, dialogs::Primary::new("Open"))
            });
        self.just_opened = false;
        if shown.should_close() {
            Answer::Cancel
        } else {
            shown.inner
        }
    }
}
