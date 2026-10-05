//! *Open in merge tool*: starts `git mergetool` on a conflicted file and leaves it to the user.
//! The tool is the one git's config names; when none is usable, a picker lists the installed
//! ones, and *Remember this choice* writes it to the global git config. Without it, the choice
//! lasts until parterre closes. Parterre has no setting of its own.

use std::path::PathBuf;
use std::sync::mpsc;

use eframe::egui::{self, RichText, ViewportId};
use parterre_core::branches::command_text;
use parterre_core::git::Git;
use parterre_core::merge_tool::{self, Configured, Detected, Tool};

use crate::dialogs::{self, Answer, Dialog};

/// A file to open, and the window it was asked from.
#[derive(Clone, Debug)]
struct File {
    root: PathBuf,
    path: String,
    opener: ViewportId,
}

#[derive(Debug, Default)]
pub struct MergeTools {
    /// Picked without remembering it: used until parterre closes.
    session: Option<Tool>,
    /// Asking git which tool, for a file.
    detecting: Option<(File, mpsc::Receiver<Result<Detected, String>>)>,
    picker: Option<Picker>,
    /// Why the last file didn't open, for the app to show.
    pub error: Option<String>,
}

#[derive(Debug)]
struct Picker {
    file: File,
    detected: Detected,
    chosen: usize,
    remember: bool,
    fresh: bool,
    /// Why *Open terminal* failed.
    failed: Option<String>,
}

impl MergeTools {
    /// Opens `path` of the worktree at `root` in the merge tool, asking which one first if
    /// none is usable.
    pub fn open(&mut self, ctx: &egui::Context, root: PathBuf, path: String, opener: ViewportId) {
        crate::usage::action(crate::usage::Action::OpenMergeTool);
        let file = File { root, path, opener };
        if let Some(tool) = self.session.clone() {
            self.start(&file, &tool);
            return;
        }
        let (tx, rx) = mpsc::channel();
        let worker_root = file.root.clone();
        let worker_ctx = ctx.clone();
        std::thread::spawn(move || {
            let detected = merge_tool::detect(&Git::new(worker_root)).map_err(|e| e.to_string());
            let _ = tx.send(detected);
            worker_ctx.request_repaint();
        });
        self.detecting = Some((file, rx));
    }

    fn start(&mut self, file: &File, tool: &Tool) {
        if let Err(e) = merge_tool::open(&file.root, tool, &file.path) {
            self.error = Some(format!("Could not open {}: {e}", file.path));
        }
    }

    /// Takes what git said, and shows the picker while it is needed.
    pub fn show(&mut self, ctx: &egui::Context) {
        if let Some((_, rx)) = &self.detecting {
            match rx.try_recv() {
                Ok(result) => {
                    let (file, _) = self.detecting.take().expect("detecting");
                    match result {
                        Ok(detected) => match detected.usable() {
                            Some(tool) => self.start(&file, &tool),
                            None => {
                                self.picker = Some(Picker {
                                    file,
                                    detected,
                                    chosen: 0,
                                    remember: false,
                                    fresh: true,
                                    failed: None,
                                })
                            }
                        },
                        Err(e) => self.error = Some(format!("Could not find a merge tool: {e}")),
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => self.detecting = None,
            }
        }
        let Some(picker) = &mut self.picker else {
            return;
        };
        match picker.show(ctx) {
            Answer::Open => {}
            Answer::Cancel => self.picker = None,
            Answer::Primary => {
                let picker = self.picker.take().expect("shown");
                let Some(tool) = picker.detected.installed.get(picker.chosen).cloned() else {
                    return;
                };
                if picker.remember {
                    let terminal = matches!(
                        picker.detected.configured,
                        Configured::Unusable { terminal: true, .. }
                    );
                    if let Err(e) =
                        merge_tool::remember(&Git::new(&picker.file.root), &tool, terminal)
                    {
                        self.error = Some(format!("Could not remember {}: {e}", tool.name));
                    }
                }
                self.session = Some(tool.clone());
                self.start(&picker.file, &tool);
            }
        }
    }
}

impl Picker {
    fn show(&mut self, ctx: &egui::Context) -> Answer {
        let why = match &self.detected.configured {
            Configured::None => "No merge tool configured".to_owned(),
            Configured::Unusable { why, .. } => why.clone(),
            Configured::Usable(name) => format!("{name} is configured"),
        };
        let terminal = matches!(
            self.detected.configured,
            Configured::Unusable { terminal: true, .. }
        );
        let tools = &self.detected.installed;
        let chosen = &mut self.chosen;
        let remember = &mut self.remember;
        let root = &self.file.root;
        let failed = &mut self.failed;
        let shown = Dialog::new("merge-tool", "Merge tool")
            .screen(crate::usage::Screen::MergeTool)
            .modal()
            .opener(self.file.opener)
            .raise(self.fresh)
            .show(ctx, |ui| {
                ui.label(format!("{why}."));
                ui.add_space(6.0);
                if tools.is_empty() {
                    ui.label("No merge tool found. Install one, then:");
                    ui.label(RichText::new("git config --global merge.tool <tool>").monospace());
                    ui.add_space(6.0);
                    if ui.button("Open terminal").clicked() {
                        crate::usage::action(crate::usage::Action::OpenTerminal);
                        *failed = crate::file_manager::open_terminal(root).err();
                    }
                    if let Some(e) = failed {
                        ui.label(RichText::new(e.as_str()).color(ui.visuals().error_fg_color));
                    }
                    return dialogs::actions(ui, "", false, false, false);
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
                dialogs::actions(ui, "Open", true, false, false)
            });
        self.fresh = false;
        if shown.should_close() {
            Answer::Cancel
        } else {
            shown.inner
        }
    }
}
