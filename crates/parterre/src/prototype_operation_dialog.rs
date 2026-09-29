//! PROTOTYPE — throwaway. How a running operation looks, and how an operation in progress
//! looks, for "The operation dialog and operations in progress: prototype"
//! (aquamoth/parterre#146). Nothing runs git: the scenarios in the ☰ menu play back scripted
//! output on a timer.
//!
//! Two things vary, each switched in the bar at the bottom of the main window:
//! - How an operation runs:
//!   - A, a modal dialog: the command, git's output as it comes, the result.
//!   - B, a window that doesn't block: move it aside and keep using the graph.
//!   - C, the status bar: a spinner and git's last line; failures open a dialog, and the whole
//!     output is a click away.
//! - How an operation in progress (a merge or rebase that stopped) shows in the graph:
//!   - 1, an extra row under the node, like a worktree's.
//!   - 2, a banner across the top of the graph, with Continue and Abort.
//!   - 3, a badge on the node's corner, as a shell prompt shows `main|MERGING`.

use std::cell::RefCell;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Painter, Rect, RichText, Stroke, Ui,
};
use parterre_core::Repo;

use crate::scene::{FONT_SIZE, Scene};
use crate::view::View;

const RUNNERS: [&str; 3] = [
    "A — modal dialog",
    "B — window that doesn't block",
    "C — status bar",
];
const SHOWS: [&str; 3] = ["1 — row under the node", "2 — banner", "3 — badge"];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Prompt {
    Username,
    Password,
    HostKey,
    Passphrase,
}

#[derive(Clone, Debug)]
enum Step {
    /// A line of output, after a pause (ms).
    Line(u32, &'static str),
    /// A progress line counting up to 100% over a time (ms), rewritten in place as git does.
    Progress(u32, &'static str),
    /// Git asks for something through parterre's askpass.
    Ask(Prompt),
    /// The end: how it went.
    End(Outcome),
}

#[derive(Clone, Debug)]
enum Outcome {
    Done(&'static str),
    Failed(&'static str, &'static str),
    /// Stopped partway, leaving an operation in progress.
    Stopped(&'static str, InProgress),
}

#[derive(Clone, Debug)]
struct InProgress {
    /// "merge", "rebase"…
    kind: &'static str,
    /// What it's doing, for the label: "feature/login into main".
    what: &'static str,
    conflicts: u32,
    /// For a rebase: "3/5".
    step: Option<&'static str>,
    /// In another worktree, not the open one.
    elsewhere: bool,
}

struct Scenario {
    title: &'static str,
    command: &'static str,
    network: bool,
    steps: Vec<Step>,
}

fn scenarios() -> Vec<Scenario> {
    use Outcome::*;
    use Step::*;
    vec![
        Scenario {
            title: "Fetch all remotes",
            command: "git fetch --all --prune --progress",
            network: true,
            steps: vec![
                Line(300, "Fetching origin"),
                Progress(1500, "Receiving objects"),
                Progress(600, "Resolving deltas"),
                Line(200, "From github.com:aquamoth/parterre"),
                Line(100, "   0ba98e9..ebbd435  main       -> origin/main"),
                Line(
                    100,
                    " * [new branch]      prototype/upstreams -> origin/prototype/upstreams",
                ),
                Line(
                    100,
                    " - [deleted]         (none)     -> origin/research/old-idea",
                ),
                End(Done("Fetched origin: 1 branch updated, 1 new, 1 deleted.")),
            ],
        },
        Scenario {
            title: "Push feature/search (rejected)",
            command: "git push --progress origin feature/search:feature/search",
            network: true,
            steps: vec![
                Line(800, "To github.com:aquamoth/parterre.git"),
                Line(
                    200,
                    " ! [rejected]        feature/search -> feature/search (fetch first)",
                ),
                Line(
                    50,
                    "error: failed to push some refs to 'github.com:aquamoth/parterre.git'",
                ),
                Line(
                    50,
                    "hint: Updates were rejected because the remote contains work that you do not",
                ),
                Line(
                    50,
                    "hint: have locally. Integrate the remote changes (e.g. 'git pull ...')",
                ),
                End(Failed(
                    "Push rejected",
                    "origin/feature/search has commits you don't have. Fetch, then pull or rebase, and push again.",
                )),
            ],
        },
        Scenario {
            title: "Switch to feature/login (quick, local)",
            command: "git switch feature/login",
            network: false,
            steps: vec![
                Line(250, "Switched to branch 'feature/login'"),
                End(Done("Switched to feature/login.")),
            ],
        },
        Scenario {
            title: "Merge feature/login into main (conflicts)",
            command: "git merge feature/login",
            network: false,
            steps: vec![
                Line(400, "Auto-merging file.txt"),
                Line(100, "CONFLICT (content): Merge conflict in file.txt"),
                Line(50, "Auto-merging README.md"),
                Line(50, "CONFLICT (content): Merge conflict in README.md"),
                Line(
                    50,
                    "Automatic merge failed; fix conflicts and then commit the result.",
                ),
                End(Stopped(
                    "The merge stopped on 2 conflicted files. Resolve them in your editor, then Continue — or Abort.",
                    InProgress {
                        kind: "merge",
                        what: "feature/login into main",
                        conflicts: 2,
                        step: None,
                        elsewhere: false,
                    },
                )),
            ],
        },
        Scenario {
            title: "Rebase main onto origin/main (stops at 3/5)",
            command: "git rebase origin/main",
            network: false,
            steps: vec![
                Line(300, "Rebasing (1/5)"),
                Line(200, "Rebasing (2/5)"),
                Line(200, "Rebasing (3/5)"),
                Line(100, "CONFLICT (content): Merge conflict in src/app.rs"),
                Line(50, "error: could not apply 1a2b3c4... Parse strings"),
                End(Stopped(
                    "The rebase stopped at commit 3 of 5 on 1 conflicted file. Resolve it, then Continue — or Skip this commit, or Abort.",
                    InProgress {
                        kind: "rebase",
                        what: "main onto origin/main",
                        conflicts: 1,
                        step: Some("3/5"),
                        elsewhere: false,
                    },
                )),
            ],
        },
        Scenario {
            title: "Pull over HTTPS (asks for credentials)",
            command: "git pull --progress",
            network: true,
            steps: vec![
                Line(400, "(git asks parterre's askpass)"),
                Ask(Prompt::Username),
                Ask(Prompt::Password),
                Progress(1200, "Receiving objects"),
                Line(100, "Updating 0ba98e9..ebbd435"),
                Line(50, "Fast-forward"),
                Line(50, " file.txt | 2 ++"),
                End(Done("Pulled 3 commits into main (fast-forward).")),
            ],
        },
        Scenario {
            title: "Fetch from a new SSH host (host key)",
            command: "git fetch --all --prune --progress",
            network: true,
            steps: vec![
                Line(400, "(ssh asks parterre's askpass)"),
                Ask(Prompt::HostKey),
                Ask(Prompt::Passphrase),
                Progress(1000, "Receiving objects"),
                End(Done("Fetched origin: nothing new.")),
            ],
        },
        Scenario {
            title: "Push with a slow pre-push hook",
            command: "git push --progress origin main",
            network: true,
            steps: vec![
                Line(300, "Running the pre-push hook…"),
                Line(1500, "   Compiling parterre v0.5.1"),
                Line(1500, "   Running tests… 312 passed"),
                Line(1500, "   All checks passed"),
                Progress(800, "Writing objects"),
                Line(100, "To github.com:aquamoth/parterre.git"),
                Line(50, "   0ba98e9..ebbd435  main -> main"),
                End(Done("Pushed main to origin/main.")),
            ],
        },
        Scenario {
            title: "Revert that fails to sign",
            command: "git revert --no-edit 1a2b3c4",
            network: false,
            steps: vec![
                Line(400, "error: gpg failed to sign the data"),
                Line(50, "fatal: failed to write commit object"),
                End(Failed(
                    "The revert wasn't committed",
                    "Git couldn't sign the commit. The revert's changes are staged in main but not committed, and there's no revert in progress to continue. Commit them yourself once signing works, or discard them.",
                )),
            ],
        },
        Scenario {
            title: "A merge stopped in another worktree",
            command: "",
            network: false,
            steps: vec![End(Stopped(
                "",
                InProgress {
                    kind: "merge",
                    what: "main into release",
                    conflicts: 1,
                    step: None,
                    elsewhere: true,
                },
            ))],
        },
    ]
}

/// A running (or finished) operation.
struct Run {
    scenario: Scenario,
    /// Index of the step being played, and when it started.
    at: usize,
    since: f64,
    output: Vec<String>,
    outcome: Option<Outcome>,
    /// When it ended, for closing a quick local one by itself.
    ended: Option<f64>,
    prompt: Option<(Prompt, String)>,
    /// Shown in the dialog (C: after a failure, or on request).
    open: bool,
}

struct State {
    runner: usize,
    show: usize,
    run: Option<Run>,
    in_progress: Option<InProgress>,
    /// C: the last operation's whole output, in a window.
    output_window: bool,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        runner: 0,
        show: 0,
        run: None,
        in_progress: None,
        output_window: false,
    });
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

fn start(scenario: Scenario, now: f64) {
    with(|s| {
        s.run = Some(Run {
            scenario,
            at: 0,
            since: now,
            output: Vec::new(),
            outcome: None,
            ended: None,
            prompt: None,
            open: s.runner != 2,
        });
    });
}

/// Plays the scenario forward to `now`.
fn advance(run: &mut Run, now: f64) {
    while run.outcome.is_none() && run.prompt.is_none() {
        let Some(step) = run.scenario.steps.get(run.at).cloned() else {
            return;
        };
        // PARTERRE_PROTO_FAST plays everything at once, for screenshots.
        let elapsed = if std::env::var_os("PARTERRE_PROTO_FAST").is_some() {
            1_000_000
        } else {
            ((now - run.since) * 1000.0) as u32
        };
        match step {
            Step::Line(ms, text) => {
                if elapsed < ms {
                    return;
                }
                run.output.push(text.to_owned());
            }
            Step::Progress(ms, what) => {
                let pct = (elapsed.min(ms) * 100 / ms.max(1)).min(100);
                let line = if pct < 100 {
                    format!("{what}: {pct:>3}% ({}/1432)", 1432 * pct / 100)
                } else {
                    format!("{what}: 100% (1432/1432), done.")
                };
                // Rewritten in place, as git does with a carriage return.
                if run.output.last().is_some_and(|l| l.starts_with(what)) {
                    *run.output.last_mut().unwrap() = line;
                } else {
                    run.output.push(line);
                }
                if elapsed < ms {
                    return;
                }
            }
            Step::Ask(p) => {
                run.prompt = Some((p, String::new()));
                run.at += 1;
                return;
            }
            Step::End(o) => {
                run.ended = Some(now);
                if matches!(o, Outcome::Failed(..)) {
                    run.open = true;
                }
                if let Outcome::Stopped(_, ip) = &o {
                    with_in_progress(ip.clone());
                }
                run.outcome = Some(o);
                return;
            }
        }
        run.at += 1;
        run.since = now;
    }
}

fn with_in_progress(ip: InProgress) {
    // Called while STATE is borrowed by the caller's `with`: defer through a second cell.
    PENDING_IP.with(|p| *p.borrow_mut() = Some(ip));
}

thread_local! {
    static PENDING_IP: RefCell<Option<InProgress>> = const { RefCell::new(None) };
}

// ---------------------------------------------------------------------------------------------
// Hooks called from the app.

/// The scenarios, in the ☰ menu.
pub fn main_menu(ui: &mut Ui) {
    crate::menu::submenu(ui, "PROTOTYPE: run an operation", |ui| {
        for (i, s) in scenarios().into_iter().enumerate() {
            if ui.button(s.title).clicked() {
                let now = ui.ctx().input(|i| i.time);
                let s = scenarios().swap_remove(i);
                start(s, now);
                ui.close();
            }
        }
        crate::menu::separator(ui);
        if ui.button("Resolve the conflicts in another tool").clicked() {
            with(|s| {
                if let Some(ip) = &mut s.in_progress {
                    ip.conflicts = 0;
                }
            });
            ui.close();
        }
        if ui.button("Clear the operation in progress").clicked() {
            with(|s| s.in_progress = None);
            ui.close();
        }
    });
}

/// Continue, Skip and Abort on the node with the operation in progress.
pub fn node_menu(ui: &mut Ui, scene: &Scene, node: usize) {
    let Some(ip) = with(|s| s.in_progress.clone()) else {
        return;
    };
    if in_progress_node(scene, &ip) != Some(node) {
        return;
    }
    let now = ui.ctx().input(|i| i.time);
    if ip.elsewhere {
        if ui.button("Go to worktree release").clicked() {
            ui.close();
        }
        ui.label(
            RichText::new(format!("A {} is in progress there", ip.kind))
                .weak()
                .small(),
        );
    } else {
        let kind = ip.kind;
        let cont = ui.add_enabled(
            ip.conflicts == 0,
            egui::Button::new(format!("Continue {kind}")),
        );
        let cont = cont.on_disabled_hover_text(format!(
            "{} conflicted file{} left",
            ip.conflicts,
            if ip.conflicts == 1 { "" } else { "s" }
        ));
        if cont.clicked() {
            start(
                Scenario {
                    title: "Continue",
                    command: if kind == "merge" {
                        "git merge --continue"
                    } else {
                        "git rebase --continue"
                    },
                    network: false,
                    steps: vec![
                        Step::Line(
                            300,
                            if kind == "merge" {
                                "[main 9f8e7d6] Merge branch 'feature/login'"
                            } else {
                                "Rebasing (4/5)"
                            },
                        ),
                        Step::End(Outcome::Done("Done.")),
                    ],
                },
                now,
            );
            with(|s| s.in_progress = None);
            ui.close();
        }
        if kind == "rebase" && ui.button("Skip this commit").clicked() {
            with(|s| {
                if let Some(ip) = &mut s.in_progress {
                    ip.step = Some("4/5");
                    ip.conflicts = 1;
                }
            });
            ui.close();
        }
        if ui.button(format!("Abort {kind}…")).clicked() {
            with(|s| s.in_progress = None);
            start(
                Scenario {
                    title: "Abort",
                    command: if kind == "merge" {
                        "git merge --abort"
                    } else {
                        "git rebase --abort"
                    },
                    network: false,
                    steps: vec![
                        Step::Line(200, ""),
                        Step::End(Outcome::Done("Aborted: main is back where it was.")),
                    ],
                },
                now,
            );
            ui.close();
        }
    }
    crate::menu::separator(ui);
}

fn in_progress_node(scene: &Scene, ip: &InProgress) -> Option<usize> {
    if ip.elsewhere {
        let others = || scene.repo.worktrees.iter().filter(|w| !w.open);
        let w = others()
            .find(|w| w.branch.as_deref().is_some_and(|b| b.ends_with("/release")))
            .or_else(|| others().next())?;
        return scene.graph.node_of(w.head?).map(|n| n as usize);
    }
    scene.head_node()
}

/// The operation in progress, drawn on the graph (1 and 3; 2 is a banner, in `show`).
pub fn paint(painter: &Painter, canvas: Rect, view: &View, scene: &Scene) {
    let Some(ip) = with(|s| s.in_progress.clone()) else {
        return;
    };
    let show = with(|s| s.show);
    let Some(node) = in_progress_node(scene, &ip) else {
        return;
    };
    let rect = view.rect_to_screen(canvas, scene.node_rect(node));
    let zoom = view.zoom;
    let fixed = |len: f32| view.fixed(len);
    let fill = if ip.conflicts > 0 {
        Color32::from_rgb(235, 140, 30)
    } else {
        Color32::from_rgb(90, 170, 90)
    };
    let label = label(&ip);
    let font = FontId::monospace(fixed(FONT_SIZE * zoom));
    match show {
        0 => {
            let h = fixed(scene.row_height * zoom);
            let galley = painter.layout_no_wrap(format!("⚠ {label}"), font, Color32::BLACK);
            let pad = fixed(8.0 * zoom);
            let width = rect.width().max(galley.size().x + 2.0 * pad);
            let row = Rect::from_min_size(rect.left_bottom(), egui::vec2(width, h));
            painter.rect_filled(row, CornerRadius::same(fixed(4.0 * zoom) as u8), fill);
            let at = row.left_center() + egui::vec2(pad, -galley.size().y / 2.0);
            painter.galley(at, galley, Color32::BLACK);
        }
        2 => {
            let text = if ip.conflicts > 0 {
                format!(
                    "{}ING {}",
                    ip.kind.to_uppercase().trim_end_matches('E'),
                    ip.conflicts
                )
            } else {
                format!("{}ING ✓", ip.kind.to_uppercase().trim_end_matches('E'))
            };
            let small = FontId::monospace(fixed(FONT_SIZE * 0.8 * zoom));
            let galley = painter.layout_no_wrap(text, small, Color32::BLACK);
            let size = galley.size() + egui::vec2(fixed(10.0 * zoom), fixed(4.0 * zoom));
            let pill =
                Rect::from_center_size(rect.right_top() + egui::vec2(-size.x / 2.0, 0.0), size);
            painter.rect(
                pill,
                CornerRadius::same((size.y / 2.0) as u8),
                fill,
                Stroke::new(1.0, Color32::WHITE),
                egui::StrokeKind::Outside,
            );
            painter.galley(pill.center() - galley.size() / 2.0, galley, Color32::BLACK);
        }
        _ => {}
    }
}

fn label(ip: &InProgress) -> String {
    let files = if ip.conflicts == 0 {
        "conflicts resolved: ready to continue".to_owned()
    } else {
        format!(
            "{} conflicted file{}",
            ip.conflicts,
            if ip.conflicts == 1 { "" } else { "s" }
        )
    };
    let step = ip.step.map(|s| format!(" at {s}")).unwrap_or_default();
    format!("{} {}{step}: {files}", capital(ip.kind), ip.what)
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

/// C: the running operation in the status bar.
pub fn status_bar(ui: &mut Ui) {
    let line = with(|s| {
        if s.runner != 2 {
            return None;
        }
        let run = s.run.as_ref()?;
        Some((
            run.outcome.is_none(),
            run.scenario.title,
            match &run.outcome {
                None => run.output.last().cloned().unwrap_or_default(),
                Some(Outcome::Done(t)) => (*t).to_owned(),
                Some(Outcome::Failed(t, _)) => (*t).to_owned(),
                Some(Outcome::Stopped(t, _)) => (*t).to_owned(),
            },
        ))
    });
    let Some((running, title, last)) = line else {
        return;
    };
    if running {
        ui.spinner();
    }
    ui.label(RichText::new(title).strong());
    ui.label(last);
    if ui.small_button("Output").clicked() {
        with(|s| s.output_window = true);
    }
    if running && ui.small_button("Cancel").clicked() {
        with(|s| s.run = None);
    }
    ui.separator();
}

/// Plays the operation forward and shows it, the prompts, the banner and the variant bar.
pub fn show(ctx: &egui::Context, repo: Option<&Repo>) {
    let now = ctx.input(|i| i.time);
    with(|s| {
        if let Some(run) = &mut s.run {
            advance(run, now);
        }
    });
    if let Some(ip) = PENDING_IP.with(|p| p.borrow_mut().take()) {
        with(|s| s.in_progress = Some(ip));
    }
    let running = with(|s| {
        s.run
            .as_ref()
            .is_some_and(|r| r.outcome.is_none() && r.prompt.is_none())
    });
    if running {
        ctx.request_repaint();
    }
    prompt(ctx);
    dialog(ctx, now);
    output_window(ctx);
    banner(ctx, repo);
    variant_bar(ctx);
}

fn prompt(ctx: &egui::Context) {
    let Some((p, mut text)) = with(|s| s.run.as_ref().and_then(|r| r.prompt.clone())) else {
        return;
    };
    let mut answer: Option<bool> = None;
    egui::Modal::new(egui::Id::new("prototype-askpass")).show(ctx, |ui| {
        ui.set_width(460.0);
        ui.label(RichText::new("PROTOTYPE: parterre's askpass").small().color(Color32::from_rgb(200, 120, 0)));
        match p {
            Prompt::Username => {
                ui.heading("Sign in to github.com");
                ui.label("Username for 'https://github.com':");
                ui.text_edit_singleline(&mut text);
            }
            Prompt::Password => {
                ui.heading("Sign in to github.com");
                ui.label("Password (or token) for 'https://aquamoth@github.com':");
                ui.add(egui::TextEdit::singleline(&mut text).password(true));
            }
            Prompt::HostKey => {
                ui.heading("Unknown host");
                ui.label("The authenticity of host 'git.example.com (203.0.113.7)' can't be established.");
                ui.label(RichText::new("ED25519 key fingerprint is SHA256:x3Jm0Kq3vL6tWc2YhX9pZs1eRb8uNf4aTg7dQo5iEk.").monospace());
                ui.label("Trust it and connect? Only if you know this fingerprint is right.");
            }
            Prompt::Passphrase => {
                ui.heading("SSH key passphrase");
                ui.label("Enter passphrase for key '~/.ssh/id_ed25519':");
                ui.add(egui::TextEdit::singleline(&mut text).password(true));
            }
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let ok = if p == Prompt::HostKey { "Trust and connect" } else { "OK" };
            if ui.button(ok).clicked() {
                answer = Some(true);
            }
            if ui.button("Cancel").clicked() {
                answer = Some(false);
            }
        });
    });
    with(|s| {
        let Some(run) = &mut s.run else { return };
        match answer {
            None => run.prompt = Some((p, text)),
            Some(true) => {
                run.prompt = None;
                run.since = ctx.input(|i| i.time);
            }
            Some(false) => {
                run.prompt = None;
                run.open = true;
                run.output
                    .push("fatal: could not read Username: terminal prompts disabled".to_owned());
                run.outcome = Some(Outcome::Failed(
                    "Cancelled",
                    "You cancelled the sign-in, so git stopped.",
                ));
                run.ended = Some(ctx.input(|i| i.time));
            }
        }
    });
}

fn dialog(ctx: &egui::Context, now: f64) {
    let runner = with(|s| s.runner);
    let Some((title, command, network, output, outcome, ended, open)) = with(|s| {
        s.run.as_ref().map(|r| {
            (
                r.scenario.title,
                r.scenario.command,
                r.scenario.network,
                r.output.clone(),
                r.outcome.clone(),
                r.ended,
                r.open,
            )
        })
    }) else {
        return;
    };
    if !open || command.is_empty() {
        if command.is_empty() {
            with(|s| s.run = None);
        }
        return;
    }
    // Quick local operations that worked close by themselves; network ones wait for Close.
    if let (Some(Outcome::Done(_)), Some(t), false) = (&outcome, ended, network)
        && now - t > 0.8
    {
        with(|s| s.run = None);
        return;
    }
    if matches!(outcome, Some(Outcome::Done(_))) && !network {
        ctx.request_repaint();
    }
    let mut close = false;
    let mut cancel = false;
    let body = |ui: &mut Ui, close: &mut bool, cancel: &mut bool| {
        ui.set_width(620.0);
        ui.label(
            RichText::new("PROTOTYPE: nothing runs")
                .small()
                .color(Color32::from_rgb(200, 120, 0)),
        );
        ui.heading(title);
        ui.label(RichText::new(command).monospace().weak());
        ui.add_space(6.0);
        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .inner_margin(6)
            .corner_radius(4)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(200.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.set_min_width(600.0);
                        for line in &output {
                            let colour = if line.starts_with("CONFLICT")
                                || line.starts_with("error")
                                || line.starts_with("fatal")
                                || line.contains("[rejected]")
                            {
                                Color32::from_rgb(210, 80, 60)
                            } else {
                                ui.visuals().text_color()
                            };
                            ui.label(RichText::new(line).monospace().color(colour));
                        }
                    });
            });
        ui.add_space(6.0);
        match &outcome {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Running…");
                });
            }
            Some(Outcome::Done(t)) => {
                ui.label(RichText::new(format!("✔ {t}")).color(Color32::from_rgb(60, 150, 60)));
            }
            Some(Outcome::Failed(t, why)) => {
                ui.label(
                    RichText::new(format!("✖ {t}"))
                        .strong()
                        .color(Color32::from_rgb(210, 80, 60)),
                );
                ui.label(*why);
            }
            Some(Outcome::Stopped(t, _)) => {
                ui.label(
                    RichText::new("⚠ Stopped on conflicts")
                        .strong()
                        .color(Color32::from_rgb(220, 130, 20)),
                );
                ui.label(*t);
            }
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if outcome.is_none() {
                if network && ui.button("Cancel").clicked() {
                    *cancel = true;
                }
            } else if ui.button("Close").clicked() {
                *close = true;
            }
        });
    };
    match runner {
        0 => {
            egui::Modal::new(egui::Id::new("prototype-operation-dialog"))
                .show(ctx, |ui| body(ui, &mut close, &mut cancel));
        }
        _ => {
            egui::Window::new("Operation")
                .id(egui::Id::new("prototype-operation-window"))
                .collapsible(true)
                .resizable(false)
                .default_pos(ctx.content_rect().right_top() + egui::vec2(-680.0, 60.0))
                .show(ctx, |ui| body(ui, &mut close, &mut cancel));
        }
    }
    if cancel {
        with(|s| {
            if let Some(run) = &mut s.run {
                run.output
                    .push("(parterre stopped git and everything it started)".to_owned());
                run.outcome = Some(Outcome::Failed(
                    "Cancelled",
                    "Nothing was changed on the remote.",
                ));
                run.ended = Some(now);
            }
        });
    }
    if close {
        with(|s| {
            if runner == 2 {
                if let Some(run) = &mut s.run {
                    run.open = false;
                }
            } else {
                s.run = None;
            }
        });
    }
}

fn output_window(ctx: &egui::Context) {
    let mut open = with(|s| s.output_window);
    if !open {
        return;
    }
    let output = with(|s| {
        s.run
            .as_ref()
            .map(|r| (r.scenario.command, r.output.clone()))
    });
    egui::Window::new("Output of the last operation")
        .open(&mut open)
        .default_width(620.0)
        .show(ctx, |ui| match &output {
            Some((command, lines)) => {
                ui.label(RichText::new(*command).monospace().weak());
                for l in lines {
                    ui.label(RichText::new(l).monospace());
                }
            }
            None => {
                ui.label("No operation has run.");
            }
        });
    with(|s| s.output_window = open);
}

/// 2: a banner across the top of the graph.
fn banner(ctx: &egui::Context, _repo: Option<&Repo>) {
    let Some(ip) = with(|s| s.in_progress.clone().filter(|_| s.show == 1)) else {
        return;
    };
    if ip.elsewhere {
        return;
    }
    egui::Area::new(egui::Id::new("prototype-in-progress-banner"))
        .anchor(Align2::CENTER_TOP, egui::vec2(0.0, 52.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            let fill = if ip.conflicts > 0 {
                Color32::from_rgb(250, 215, 160)
            } else {
                Color32::from_rgb(200, 235, 200)
            };
            egui::Frame::new()
                .fill(fill)
                .corner_radius(8)
                .inner_margin(egui::Margin::symmetric(14, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("⚠ {}", label(&ip))).color(Color32::BLACK));
                        let now = ui.ctx().input(|i| i.time);
                        if ui
                            .add_enabled(ip.conflicts == 0, egui::Button::new("Continue"))
                            .clicked()
                        {
                            with(|s| s.in_progress = None);
                            start(
                                Scenario {
                                    title: "Continue",
                                    command: "git merge --continue",
                                    network: false,
                                    steps: vec![
                                        Step::Line(
                                            300,
                                            "[main 9f8e7d6] Merge branch 'feature/login'",
                                        ),
                                        Step::End(Outcome::Done("Done.")),
                                    ],
                                },
                                now,
                            );
                        }
                        if ui.button("Abort…").clicked() {
                            with(|s| s.in_progress = None);
                        }
                    });
                });
        });
}

fn variant_bar(ctx: &egui::Context) {
    egui::Area::new(egui::Id::new("prototype-dialog-bar"))
        .anchor(Align2::CENTER_BOTTOM, egui::vec2(0.0, -40.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgb(30, 30, 30))
                .corner_radius(20)
                .inner_margin(egui::Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let text = |s: &str| RichText::new(s).color(Color32::WHITE);
                        let (runner, show) = with(|s| (s.runner, s.show));
                        if ui.add(egui::Button::new(text("◀")).frame(false)).clicked() {
                            with(|s| s.runner = (runner + 2) % 3);
                        }
                        ui.label(text(&format!("Running: {}", RUNNERS[runner])));
                        if ui.add(egui::Button::new(text("▶")).frame(false)).clicked() {
                            with(|s| s.runner = (runner + 1) % 3);
                        }
                        ui.label(text("   |   "));
                        if ui.add(egui::Button::new(text("◀")).frame(false)).clicked() {
                            with(|s| s.show = (show + 2) % 3);
                        }
                        ui.label(text(&format!("In progress: {}", SHOWS[show])));
                        if ui.add(egui::Button::new(text("▶")).frame(false)).clicked() {
                            with(|s| s.show = (show + 1) % 3);
                        }
                    });
                });
        });
}

/// For scripted screenshots: `PARTERRE_PROTO=<runner><show>:<scenario>`, e.g. `A1:3`.
pub fn from_env(now: f64) {
    thread_local! { static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
    if DONE.with(|d| d.replace(true)) {
        return;
    }
    let Ok(v) = std::env::var("PARTERRE_PROTO") else {
        return;
    };
    let mut chars = v.chars();
    let runner = match chars.next() {
        Some('B') => 1,
        Some('C') => 2,
        _ => 0,
    };
    let show = match chars.next() {
        Some('2') => 1,
        Some('3') => 2,
        _ => 0,
    };
    with(|s| {
        s.runner = runner;
        s.show = show;
    });
    if let Some((_, n)) = v.split_once(':')
        && let Ok(n) = n.parse::<usize>()
        && n < scenarios().len()
    {
        start(scenarios().swap_remove(n), now);
    }
}
