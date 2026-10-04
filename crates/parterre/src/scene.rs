//! A laid-out revision graph ready to draw: node contents and sizes, layout, and the physics
//! net that holds the current (possibly dragged) positions.

use std::sync::Arc;

use eframe::egui::{Color32, FontId, Pos2, Rect, Vec2, pos2, vec2};
use parterre_core::layout::{self, Layout, LayoutEdge, LayoutInput, LayoutOptions, Point};
use parterre_core::physics::{DragModel, Net};
use parterre_core::revgraph::{self, RevGraph};
use parterre_core::{Label, RefKind, Repo};
use parterre_forge::{PullRequest, PullRequests};

use crate::settings::Settings;

/// Node geometry at zoom 1, in logical pixels (TortoiseGit: Consolas 9 pt, margins 20 and 5).
pub const FONT_SIZE: f32 = 12.0;
pub const MARGIN_X: f32 = 20.0;
pub const MARGIN_Y: f32 = 5.0;
pub const CORNER_RADIUS: f32 = 6.0;
/// The room a worktree's folder glyph takes before its label, at 100%: the glyph and a gap.
pub const WORKTREE_GLYPH: f32 = FONT_SIZE + 4.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// The commit's short hash, on nodes that no ref labels.
    Hash,
    Ref {
        kind: RefKind,
        head: bool,
        /// The worktree that has this checked out (for HEAD, the open one), while worktrees
        /// are shown. Its name then follows the folder glyph.
        worktree: Option<Checkout>,
    },
    /// A worktree whose detached HEAD this commit is. Its label is the folder's name, drawn
    /// after the folder glyph, in italics.
    Worktree(Checkout),
    /// An open pull request whose head this commit is: [`Scene::pull_requests`]`[index]`. Its
    /// label is the number, drawn after the pull-request glyph.
    PullRequest { index: usize, draft: bool },
}

/// A worktree a row stands for: [`Repo::worktrees`]`[index]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Checkout {
    pub index: usize,
    /// Its folder is gone.
    pub missing: bool,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub label: String,
    pub kind: RowKind,
    /// The label's width at 100% (with the glyph before it, for a worktree).
    pub width: f32,
}

impl Row {
    /// A detached worktree's name, in italics: it is no branch.
    pub fn is_detached_worktree(&self) -> bool {
        matches!(
            self.kind,
            RowKind::Worktree(_)
                | RowKind::Ref {
                    kind: RefKind::DetachedHead,
                    worktree: Some(_),
                    ..
                }
        )
    }
}

#[derive(Clone, Debug)]
pub struct NodeVisual {
    pub rows: Vec<Row>,
    pub size: Vec2,
}

#[derive(Debug)]
pub struct Scene {
    /// The repository snapshot this scene was built from. Everything that maps nodes back to
    /// commits and refs must use this one (not a newer reload).
    pub repo: Arc<Repo>,
    pub graph: RevGraph,
    pub layout: Layout,
    pub visuals: Vec<NodeVisual>,
    pub net: Net,
    pub row_height: f32,
    /// The open pull requests that may label nodes (see [`RowKind::PullRequest`]).
    pub pull_requests: Vec<PullRequest>,
}

/// Everything needed to lay a scene out, prepared on the UI thread (which owns the fonts);
/// [`SceneInput::lay_out`] can then run on any thread.
#[derive(Debug)]
pub struct SceneInput {
    repo: Arc<Repo>,
    graph: RevGraph,
    visuals: Vec<NodeVisual>,
    input: LayoutInput,
    options: LayoutOptions,
    row_height: f32,
    pull_requests: Vec<PullRequest>,
}

impl SceneInput {
    pub fn lay_out(self) -> Scene {
        let layout = layout::layout(&self.input, &self.options);
        let net = Net::new(&layout, &self.input.sizes);
        Scene {
            repo: self.repo,
            graph: self.graph,
            layout,
            visuals: self.visuals,
            net,
            row_height: self.row_height,
            pull_requests: self.pull_requests,
        }
    }
}

impl Scene {
    /// Builds the graph for the current settings, with `pull_requests` if they are shown, and
    /// measures its nodes. `text_width` measures a string at [`FONT_SIZE`]; `text_height` is the
    /// height of one line of text.
    pub fn prepare(
        repo: &Arc<Repo>,
        settings: &Settings,
        pull_requests: Option<&PullRequests>,
        text_width: &mut dyn FnMut(&str) -> f32,
        text_height: f32,
    ) -> SceneInput {
        let (heads, pull_requests): (Vec<_>, Vec<_>) = pull_requests
            .map(|p| p.heads(repo))
            .unwrap_or_default()
            .into_iter()
            .map(|(head, pr)| (head, pr.clone()))
            .unzip();
        let graph = revgraph::build_with_pull_requests(repo, &settings.graph, &heads);

        let row_height = text_height + 2.0 * MARGIN_Y;
        // Commits without refs show their hash as long as git abbreviates it in this
        // repository (`Repo::abbrev_len`), like the log window. Deliberate deviation:
        // TortoiseGit always shows 8 digits.
        let hash_width = text_width(&"8".repeat(repo.abbrev_len));
        let checkout = |index: usize| Checkout {
            index,
            missing: repo.worktrees[index].missing,
        };
        let worktrees_shown = settings.graph.show_worktrees;
        let visuals: Vec<NodeVisual> = graph
            .nodes
            .iter()
            .map(|node| {
                let labels = repo.labels(&node.refs, &node.worktrees, worktrees_shown);
                let mut rows: Vec<Row> = labels
                    .iter()
                    .map(|&label| match label {
                        Label::Ref { index, worktree } => {
                            let r = &repo.refs[index];
                            Row {
                                label: repo.label_name(label),
                                kind: RowKind::Ref {
                                    kind: r.kind,
                                    head: r.is_head,
                                    worktree: worktree.map(checkout),
                                },
                                width: 0.0,
                            }
                        }
                        Label::Worktree(index) => Row {
                            label: repo.label_name(label),
                            kind: RowKind::Worktree(checkout(index)),
                            width: 0.0,
                        },
                    })
                    .collect();
                // Only refs stand in for the hash. Worktrees go above it, pull requests below,
                // and the open worktree's detached HEAD is a worktree like any other.
                if rows.iter().all(Row::is_detached_worktree) {
                    rows.push(Row {
                        label: repo.commit(node.commit).oid.short(repo.abbrev_len),
                        kind: RowKind::Hash,
                        width: 0.0,
                    });
                }
                rows.extend(node.pull_requests.iter().map(|&index| Row {
                    label: pull_requests[index].number.to_string(),
                    kind: RowKind::PullRequest {
                        index,
                        draft: pull_requests[index].draft,
                    },
                    width: 0.0,
                }));
                for row in &mut rows {
                    row.width = text_width(&row.label);
                    if let RowKind::Worktree(_)
                    | RowKind::Ref {
                        worktree: Some(_), ..
                    } = row.kind
                    {
                        row.width += WORKTREE_GLYPH;
                    }
                }
                let widest = rows.iter().map(|r| r.width).fold(hash_width, f32::max);
                let size = vec2(widest + 2.0 * MARGIN_X, row_height * rows.len() as f32);
                NodeVisual { rows, size }
            })
            .collect();

        let sizes: Vec<Point> = visuals
            .iter()
            .map(|v| Point::new(v.size.x, v.size.y))
            .collect();
        let anchor = graph.layout_anchor(repo);
        let input = LayoutInput {
            sizes: sizes.clone(),
            times: graph
                .nodes
                .iter()
                .map(|n| repo.commit(n.commit).commit_time)
                .collect(),
            edges: graph
                .edges
                .iter()
                .map(|e| LayoutEdge {
                    child: e.child,
                    parent: e.parent,
                    first_parent: e.first_parent,
                })
                .collect(),
            priority: anchor.into_iter().collect(),
        };
        SceneInput {
            repo: Arc::clone(repo),
            graph,
            visuals,
            input,
            options: settings.layout.clone(),
            row_height,
            pull_requests,
        }
    }

    /// Builds and lays out the scene on this thread, without a window (for `--export`).
    pub fn headless(repo: &Arc<Repo>, settings: &Settings) -> Scene {
        let ctx = eframe::egui::Context::default();
        // One pass initialises the fonts used to measure labels.
        // Nothing is rendered, so the texture updates are discarded.
        ctx.run_ui(eframe::egui::RawInput::default(), |_| {})
            .textures_delta
            .clear();
        let font = FontId::monospace(FONT_SIZE);
        let text_height = ctx.fonts_mut(|f| f.row_height(&font));
        let input = ctx.fonts_mut(|f| {
            let mut width = |s: &str| {
                f.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE)
                    .size()
                    .x
            };
            Scene::prepare(repo, settings, None, &mut width, text_height)
        });
        input.lay_out()
    }

    pub fn node_count(&self) -> usize {
        self.visuals.len()
    }

    /// Current centre of a node in world coordinates.
    pub fn node_center(&self, node: usize) -> Pos2 {
        to_pos(self.net.node_pos(node))
    }

    /// Current box of a node in world coordinates.
    pub fn node_rect(&self, node: usize) -> Rect {
        Rect::from_center_size(self.node_center(node), self.visuals[node].size)
    }

    /// The row of `node` at a world position inside its box.
    pub fn row_at(&self, node: usize, world: Pos2) -> Option<&Row> {
        let row = (world.y - self.node_rect(node).min.y) / self.row_height;
        self.visuals[node].rows.get(row.max(0.0) as usize)
    }

    /// The pull request (an index into [`Scene::pull_requests`]) whose label is at a world
    /// position, if any.
    pub fn pull_request_at(&self, world: Pos2) -> Option<usize> {
        let node = self.node_at(world)?;
        match self.row_at(node, world)?.kind {
            RowKind::PullRequest { index, .. } => Some(index),
            _ => None,
        }
    }

    /// The worktrees shown on `node`, in row order: those with its branches checked out,
    /// then the detached ones.
    pub fn worktrees_on(&self, node: usize) -> Vec<usize> {
        self.visuals[node]
            .rows
            .iter()
            .filter_map(|row| match row.kind {
                RowKind::Worktree(c)
                | RowKind::Ref {
                    worktree: Some(c), ..
                } => Some(c.index),
                _ => None,
            })
            .collect()
    }

    /// Topmost node under a world position.
    pub fn node_at(&self, world: Pos2) -> Option<usize> {
        (0..self.node_count())
            .rev()
            .find(|&i| self.node_rect(i).contains(world))
    }

    /// The nodes that move along when `roots` are dragged in `model`: everything growing out
    /// of them in Subtree mode, nothing otherwise.
    pub fn carried_nodes(&self, roots: &[usize], model: DragModel) -> Vec<usize> {
        match model {
            DragModel::Subtree => self.graph.subtree(roots),
            DragModel::Adapt | DragModel::Free => Vec::new(),
        }
    }

    /// Nodes whose boxes touch `rect` (world coordinates).
    pub fn nodes_in(&self, rect: Rect) -> Vec<usize> {
        (0..self.node_count())
            .filter(|&i| rect.intersects(self.node_rect(i)))
            .collect()
    }

    /// Bounding box of the nodes where they are now, in world coordinates. Not the layout's
    /// box: after nodes have been dragged in from its edges, fitting that would leave empty
    /// margins.
    pub fn bounds(&self) -> Rect {
        (0..self.node_count())
            .map(|i| self.node_rect(i))
            .reduce(|a, b| a.union(b))
            .unwrap_or_else(|| Rect::from_min_max(to_pos(self.layout.min), to_pos(self.layout.max)))
    }

    pub fn head_node(&self) -> Option<usize> {
        self.graph.nodes.iter().position(|n| n.is_head)
    }
}

pub fn to_pos(p: Point) -> Pos2 {
    pos2(p.x, p.y)
}

pub fn to_point(p: Pos2) -> Point {
    Point::new(p.x, p.y)
}

#[cfg(test)]
mod tests {
    use parterre_core::{Commit, CommitIx, Head, Oid};
    use parterre_forge::{PullRequest, PullRequests, Remote};

    use super::*;

    #[test]
    fn hash_rows_are_as_long_as_git_abbreviates() {
        let commit = Commit {
            oid: Oid::from_hex(&"abcdef0123".repeat(4)).unwrap(),
            parents: Vec::new(),
            truncated: false,
            empty_tree: false,
            author_name: String::new(),
            author_email: String::new(),
            author_time: 0,
            author_date: String::new(),
            commit_time: 0,
            subject: String::new(),
        };
        let mut repo = Repo::new(
            "/x".into(),
            vec![commit],
            Vec::new(),
            Head::Detached(CommitIx(0)),
        );
        repo.abbrev_len = 12;
        let input = Scene::prepare(
            &Arc::new(repo),
            &Settings::default(),
            None,
            &mut |s| s.len() as f32,
            10.0,
        );
        let visual = &input.visuals[0];
        assert_eq!(visual.rows[0].kind, RowKind::Hash);
        assert_eq!(visual.rows[0].label, "abcdef0123ab");
        // The box is sized for that many digits.
        assert_eq!(visual.size.x, 12.0 + 2.0 * MARGIN_X);
    }

    #[test]
    fn pull_requests_are_rows_below_the_refs() {
        let commit = |n: u8, parents: Vec<CommitIx>| Commit {
            oid: Oid::from_hex(&format!("{n:02x}").repeat(20)).unwrap(),
            parents,
            truncated: false,
            empty_tree: false,
            author_name: String::new(),
            author_email: String::new(),
            author_time: 0,
            author_date: String::new(),
            commit_time: n.into(),
            subject: String::new(),
        };
        let git_ref = |full_name: &str, target: u32| {
            let (kind, name) = parterre_core::git::classify_ref(full_name);
            parterre_core::GitRef {
                full_name: full_name.into(),
                name,
                kind,
                target: CommitIx(target),
                annotated: false,
                is_head: false,
            }
        };
        // Commit 2 (origin/main) is the child of 1, the child of 0.
        let repo = Arc::new(Repo::new(
            "/x".into(),
            vec![
                commit(2, vec![CommitIx(1)]),
                commit(1, vec![CommitIx(2)]),
                commit(0, Vec::new()),
            ],
            vec![git_ref("refs/remotes/origin/main", 0)],
            Head::Detached(CommitIx(0)),
        ));
        let pull_request = |number: u64, head: u8, draft: bool| PullRequest {
            number,
            title: String::new(),
            author: String::new(),
            draft,
            head: Oid::from_hex(&format!("{head:02x}").repeat(20)).unwrap(),
            head_branch: "topic".into(),
            head_repo: None,
            base_branch: "main".into(),
            base_repo: "o/r".into(),
            url: String::new(),
        };
        let prs = PullRequests {
            list: vec![pull_request(12, 2, false), pull_request(9, 1, true)],
            remotes: vec![Remote {
                name: "origin".into(),
                repo: "o/r".into(),
            }],
            upstreams: Default::default(),
        };
        let mut settings = Settings::default();
        settings.graph.show_pull_requests = true;
        let input = Scene::prepare(&repo, &settings, Some(&prs), &mut |s| s.len() as f32, 10.0);
        let rows = |i: usize| -> Vec<(String, RowKind)> {
            input.visuals[i]
                .rows
                .iter()
                .map(|r| (r.label.clone(), r.kind.clone()))
                .collect()
        };
        let pr_row =
            |number: &str, index, draft| (number.to_owned(), RowKind::PullRequest { index, draft });
        // The ref first, then the pull request.
        assert_eq!(
            rows(0),
            [
                (
                    "origin/main".to_owned(),
                    RowKind::Ref {
                        kind: RefKind::RemoteBranch,
                        head: false,
                        worktree: None,
                    }
                ),
                pr_row("12", 0, false)
            ]
        );
        // A commit with no refs shows its hash, then the pull request.
        let hash = "01".repeat(20)[..repo.abbrev_len].to_owned();
        assert_eq!(rows(1), [(hash, RowKind::Hash), pr_row("9", 1, true)]);
        assert_eq!(input.visuals[1].size.y, 2.0 * input.row_height);
        assert_eq!(input.pull_requests.len(), 2);

        // Its hash row is the node's, its pull request's row the pull request's.
        let scene = input.lay_out();
        let rect = scene.node_rect(1);
        let hash_row = pos2(rect.center().x, rect.min.y + 0.5 * scene.row_height);
        let pull_request_row = pos2(rect.center().x, rect.min.y + 1.5 * scene.row_height);
        assert_eq!(scene.node_at(hash_row), Some(1));
        assert_eq!(scene.pull_request_at(hash_row), None);
        assert_eq!(scene.node_at(pull_request_row), Some(1));
        assert_eq!(scene.pull_request_at(pull_request_row), Some(1));

        // Turned off, nothing of them shows.
        settings.graph.show_pull_requests = false;
        let input = Scene::prepare(&repo, &settings, Some(&prs), &mut |s| s.len() as f32, 10.0);
        assert_eq!(input.visuals.len(), 2);
        assert!(input.visuals.iter().all(|v| {
            v.rows
                .iter()
                .all(|r| !matches!(r.kind, RowKind::PullRequest { .. }))
        }));
    }

    #[test]
    fn worktrees_colour_their_branches_and_label_their_detached_heads() {
        let commit = |n: u8, parents: Vec<CommitIx>| parterre_core::Commit {
            oid: Oid::from_hex(&format!("{n:02x}").repeat(20)).unwrap(),
            parents,
            truncated: false,
            empty_tree: false,
            author_name: String::new(),
            author_email: String::new(),
            author_time: 0,
            author_date: String::new(),
            commit_time: n.into(),
            subject: String::new(),
        };
        let git_ref = |full_name: &str, target: u32, is_head: bool| {
            let (kind, name) = parterre_core::git::classify_ref(full_name);
            parterre_core::GitRef {
                full_name: full_name.into(),
                name,
                kind,
                target: CommitIx(target),
                annotated: false,
                is_head,
            }
        };
        let worktree =
            |path: &str, head: u32, branch: Option<&str>, missing, open| parterre_core::Worktree {
                path: path.into(),
                head: Some(CommitIx(head)),
                branch: branch.map(str::to_owned),
                locked: false,
                missing,
                open,
            };
        // main (0, HEAD) - topic (1, also `a-first`) - the root (2), with topic and the root
        // checked out elsewhere, the root in a worktree whose folder is gone.
        let mut repo = Repo::new(
            "/src/main".into(),
            vec![
                commit(2, vec![CommitIx(1)]),
                commit(1, vec![CommitIx(2)]),
                commit(0, Vec::new()),
            ],
            vec![
                git_ref("refs/heads/main", 0, true),
                git_ref("refs/heads/a-first", 1, false),
                git_ref("refs/heads/topic", 1, false),
            ],
            Head::Branch {
                name: "refs/heads/main".into(),
                target: Some(CommitIx(0)),
            },
        );
        repo.worktrees = vec![
            worktree("/src/main", 0, Some("refs/heads/main"), false, true),
            worktree("/src/wt-topic", 1, Some("refs/heads/topic"), false, false),
            worktree("/src/wt-gone", 2, None, true, false),
        ];
        let repo = Arc::new(repo);
        let mut settings = Settings::default();
        settings.graph.show_worktrees = true;
        let prepare = |settings: &Settings| {
            Scene::prepare(&repo, settings, None, &mut |s| s.len() as f32, 10.0)
        };
        let input = prepare(&settings);
        let rows = |i: usize| -> Vec<(String, RowKind)> {
            input.visuals[i]
                .rows
                .iter()
                .map(|r| (r.label.clone(), r.kind.clone()))
                .collect()
        };
        let branch = |name: &str, head, worktree| {
            let kind = RefKind::LocalBranch;
            (
                name.to_owned(),
                RowKind::Ref {
                    kind,
                    head,
                    worktree,
                },
            )
        };
        // The open worktree's branch is HEAD's, and red as ever.
        let open = Checkout {
            index: 0,
            missing: false,
        };
        assert_eq!(rows(0), [branch("main", true, Some(open))]);
        let topic = Checkout {
            index: 1,
            missing: false,
        };
        // Worktrees first.
        assert_eq!(
            rows(1),
            [
                branch("topic", false, Some(topic)),
                branch("a-first", false, None)
            ]
        );
        // A detached worktree labels its commit above the hash, with its folder's name.
        let gone = Checkout {
            index: 2,
            missing: true,
        };
        let hash = "00".repeat(20)[..repo.abbrev_len].to_owned();
        assert_eq!(
            rows(2),
            [
                ("wt-gone".to_owned(), RowKind::Worktree(gone)),
                (hash, RowKind::Hash)
            ]
        );
        assert_eq!(input.visuals[2].rows[0].width, 7.0 + WORKTREE_GLYPH);

        let scene = input.lay_out();
        assert_eq!(scene.worktrees_on(0), [0]);
        assert_eq!(scene.worktrees_on(1), [1]);
        assert_eq!(scene.worktrees_on(2), [2]);

        // Turned off, the branch is a plain one and the detached worktree isn't shown.
        settings.graph.show_worktrees = false;
        let input = prepare(&settings);
        let kinds: Vec<RowKind> = input.visuals[1]
            .rows
            .iter()
            .map(|r| r.kind.clone())
            .collect();
        assert_eq!(
            kinds,
            [
                branch("a-first", false, None).1,
                branch("topic", false, None).1
            ]
        );
        assert_eq!(input.visuals[2].rows.len(), 1);
    }

    #[test]
    fn the_layout_is_the_same_whichever_worktree_is_open() {
        let commit = |n: u8, parents: Vec<CommitIx>| parterre_core::Commit {
            oid: Oid::from_hex(&format!("{n:02x}").repeat(20)).unwrap(),
            parents,
            truncated: false,
            empty_tree: false,
            author_name: String::new(),
            author_email: String::new(),
            author_time: 0,
            author_date: String::new(),
            commit_time: n.into(),
            subject: String::new(),
        };
        let git_ref = |full_name: &str, target: u32, is_head: bool| {
            let (kind, name) = parterre_core::git::classify_ref(full_name);
            parterre_core::GitRef {
                full_name: full_name.into(),
                name,
                kind,
                target: CommitIx(target),
                annotated: false,
                is_head,
            }
        };
        let worktree =
            |path: &str, head: u32, branch: Option<&str>, open| parterre_core::Worktree {
                path: path.into(),
                head: Some(CommitIx(head)),
                branch: branch.map(str::to_owned),
                locked: false,
                missing: false,
                open,
            };
        // main (0) and a detached worktree (1), both on the root (2). The detached one is
        // newer, so it would get the leftmost column whenever it is HEAD.
        let open = |detached_open: bool| {
            let mut refs = vec![git_ref("refs/heads/main", 0, !detached_open)];
            let head = if detached_open {
                refs.push(parterre_core::GitRef {
                    kind: RefKind::DetachedHead,
                    ..git_ref("HEAD", 1, true)
                });
                Head::Detached(CommitIx(1))
            } else {
                Head::Branch {
                    name: "refs/heads/main".into(),
                    target: Some(CommitIx(0)),
                }
            };
            let commits = vec![
                commit(1, vec![CommitIx(2)]),
                commit(2, vec![CommitIx(2)]),
                commit(0, Vec::new()),
            ];
            let mut repo = Repo::new("/src/main".into(), commits, refs, head);
            repo.worktrees = vec![
                worktree("/src/main", 0, Some("refs/heads/main"), !detached_open),
                worktree("/src/wt-detached", 1, None, detached_open),
            ];
            Arc::new(repo)
        };
        let mut settings = Settings::default();
        settings.graph.show_worktrees = true;
        let prepare =
            |repo: &Arc<Repo>| Scene::prepare(repo, &settings, None, &mut |s| s.len() as f32, 10.0);
        let from_main = prepare(&open(false));
        let from_detached = prepare(&open(true));
        assert_eq!(from_main.input, from_detached.input);
        // The main worktree's HEAD is the leftmost line, from both.
        assert_eq!(from_main.input.priority, [0]);

        // The open detached worktree is named after its folder, and red.
        let row = &from_detached.visuals[1].rows[0];
        assert_eq!(row.label, "wt-detached");
        assert!(row.is_detached_worktree());
        assert_eq!(from_detached.visuals[1].rows[1].kind, RowKind::Hash);
        assert!(matches!(
            row.kind,
            RowKind::Ref {
                kind: RefKind::DetachedHead,
                head: true,
                ..
            }
        ));
    }
}
