//! Colours, following TortoiseGit's defaults (`src/TortoiseProc/Colors.cpp`), plus colours
//! chosen per branch name.

use eframe::egui::Color32;
use parterre_core::RefKind;
use parterre_core::pattern::BranchPatterns;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeChoice {
    /// Follow the operating system.
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 3] = [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark];

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::System => "Follow system",
            ThemeChoice::Light => "Light",
            ThemeChoice::Dark => "Dark",
        }
    }
}

/// A colour for branches whose names match `patterns` (e.g. `feature/*`). Not in TortoiseGit,
/// which colours by ref kind only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BranchColor {
    /// Wildcards separated by commas or spaces; see [`BranchPatterns`].
    pub patterns: String,
    pub color: Color32,
}

impl BranchColor {
    /// Colours offered for new rules, in turn: distinct from the ref-kind colours and legible
    /// with either black or white text.
    pub const SUGGESTED: [Color32; 6] = [
        Color32::from_rgb(0x9B, 0x59, 0xB6),
        Color32::from_rgb(0x29, 0x80, 0xB9),
        Color32::from_rgb(0x1A, 0xBC, 0x9C),
        Color32::from_rgb(0xE6, 0x7E, 0x22),
        Color32::from_rgb(0xE8, 0x43, 0x93),
        Color32::from_rgb(0x7F, 0x8C, 0x8D),
    ];

    /// Parses `PATTERNS=COLOR`, e.g. `feature/*,bugfix/*=#8e44ad` (the `#` is optional).
    pub fn parse(s: &str) -> Result<BranchColor, String> {
        let (patterns, color) = s
            .rsplit_once('=')
            .ok_or_else(|| format!("expected PATTERNS=COLOR, got `{s}`"))?;
        let hex = color.trim();
        let hex = if hex.starts_with('#') {
            hex.to_owned()
        } else {
            format!("#{hex}")
        };
        let color = Color32::from_hex(&hex)
            .map_err(|_| format!("`{color}` is not a colour like #8e44ad"))?;
        Ok(BranchColor {
            patterns: patterns.trim().to_owned(),
            color: color.to_opaque(),
        })
    }
}

/// Resolved colours for drawing the graph.
#[derive(Clone, Debug)]
pub struct Palette {
    /// Whether this is the dark palette, drawn with egui's dark visuals (which draw text
    /// bolder).
    pub dark: bool,
    pub background: Color32,
    pub edge: Color32,
    pub plain_fill: Color32,
    pub plain_border: Color32,
    pub plain_text: Color32,
    pub current_branch: Color32,
    pub local_branch: Color32,
    pub remote_branch: Color32,
    pub tag: Color32,
    pub stash: Color32,
    pub other_ref: Color32,
    /// Open pull requests, and draft ones. Not in TortoiseGit.
    pub pull_request: Color32,
    pub draft_pull_request: Color32,
    pub selection: Color32,
    pub search_hit: Color32,
    /// Colours by branch name, first match first; they override the ref-kind colours.
    pub branch_colors: Vec<(BranchPatterns, Color32)>,
}

impl Palette {
    /// The light or dark palette with `branch_colors` applied.
    pub fn new(dark: bool, branch_colors: &[BranchColor]) -> Palette {
        let mut p = if dark {
            Palette::dark()
        } else {
            Palette::light()
        };
        // Taken as chosen in both themes (not lightness-inverted like the ref-kind colours),
        // so the graph shows what the colour button shows.
        p.branch_colors = branch_colors
            .iter()
            .map(|b| (BranchPatterns::parse(&b.patterns), b.color))
            .collect();
        p
    }

    pub fn light() -> Palette {
        Palette {
            dark: false,
            background: Color32::WHITE,
            edge: Color32::BLACK,
            // TortoiseGit's "brightColor", which works out to a pale lavender on white.
            plain_fill: Color32::from_rgb(0xE8, 0xE8, 0xFF),
            plain_border: Color32::from_rgb(0xB8, 0xB8, 0xD8),
            plain_text: Color32::BLACK,
            current_branch: Color32::from_rgb(200, 0, 0),
            local_branch: Color32::from_rgb(0, 195, 0),
            remote_branch: Color32::from_rgb(255, 221, 170),
            tag: Color32::from_rgb(255, 255, 0),
            stash: Color32::from_rgb(128, 128, 128),
            other_ref: Color32::from_rgb(224, 224, 224),
            // Pale blue next to TortoiseGit's pale ref colours; drafts greyer.
            pull_request: Color32::from_rgb(0xB6, 0xD9, 0xFF),
            draft_pull_request: Color32::from_rgb(0xD2, 0xDA, 0xE2),
            selection: Color32::from_rgb(0, 120, 215),
            search_hit: Color32::from_rgb(255, 140, 0),
            branch_colors: Vec::new(),
        }
    }

    /// TortoiseGit's dark mode inverts the HSL lightness of every ref colour.
    pub fn dark() -> Palette {
        let l = Palette::light();
        Palette {
            dark: true,
            background: Color32::from_rgb(0x1E, 0x1E, 0x1E),
            edge: Color32::from_rgb(0xD0, 0xD0, 0xD0),
            plain_fill: Color32::from_rgb(0x2E, 0x2E, 0x48),
            plain_border: Color32::from_rgb(0x50, 0x50, 0x70),
            plain_text: Color32::from_rgb(0xE8, 0xE8, 0xE8),
            current_branch: invert_lightness(l.current_branch),
            local_branch: invert_lightness(l.local_branch),
            remote_branch: invert_lightness(l.remote_branch),
            tag: invert_lightness(l.tag),
            stash: invert_lightness(l.stash),
            other_ref: invert_lightness(l.other_ref),
            pull_request: invert_lightness(l.pull_request),
            draft_pull_request: invert_lightness(l.draft_pull_request),
            selection: Color32::from_rgb(80, 170, 255),
            search_hit: Color32::from_rgb(255, 160, 40),
            branch_colors: Vec::new(),
        }
    }

    /// Fill of a row for the ref `name`. The current branch stays red whatever its name.
    pub fn ref_fill(&self, kind: RefKind, is_head: bool, name: &str) -> Color32 {
        if !is_head
            && let Some(&(_, color)) = self
                .branch_colors
                .iter()
                .find(|(patterns, _)| patterns.matches(kind, name))
        {
            return color;
        }
        match kind {
            RefKind::LocalBranch if is_head => self.current_branch,
            RefKind::LocalBranch => self.local_branch,
            RefKind::RemoteBranch => self.remote_branch,
            RefKind::Tag => self.tag,
            RefKind::Stash => self.stash,
            RefKind::DetachedHead => self.current_branch,
            RefKind::Other => self.other_ref,
        }
    }
}

/// Black or white, whichever reads better on `fill` (TortoiseGit: WCAG luminance > 0.5).
pub fn text_on(fill: Color32) -> Color32 {
    fn channel(c: u8) -> f32 {
        let c = c as f32 / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    let l = 0.2126 * channel(fill.r()) + 0.7152 * channel(fill.g()) + 0.0722 * channel(fill.b());
    if l > 0.5 {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

/// Inverts HSL lightness (`l = 1 - l`, clamped to [0.05, 0.9]) keeping hue and saturation.
fn invert_lightness(c: Color32) -> Color32 {
    let (r, g, b) = (
        c.r() as f32 / 255.0,
        c.g() as f32 / 255.0,
        c.b() as f32 / 255.0,
    );
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    let (h, s) = if d == 0.0 {
        (0.0, 0.0)
    } else {
        let s = if l > 0.5 {
            d / (2.0 - max - min)
        } else {
            d / (max + min)
        };
        let h = if max == r {
            (g - b) / d + if g < b { 6.0 } else { 0.0 }
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        (h / 6.0, s)
    };
    let l = (1.0 - l).clamp(0.05, 0.9);
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let hue = |mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    let (r, g, b) = if s == 0.0 {
        (l, l, l)
    } else {
        (hue(h + 1.0 / 3.0), hue(h), hue(h - 1.0 / 3.0))
    };
    Color32::from_rgb(
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_colour_matches_tortoisegit_table() {
        let p = Palette::light();
        assert_eq!(text_on(p.current_branch), Color32::WHITE);
        assert_eq!(text_on(p.local_branch), Color32::WHITE);
        assert_eq!(text_on(p.remote_branch), Color32::BLACK);
        assert_eq!(text_on(p.tag), Color32::BLACK);
        assert_eq!(text_on(p.other_ref), Color32::BLACK);
    }

    #[test]
    fn branch_colours_override_kind_colours_but_not_head() {
        let purple = Color32::from_rgb(0x9B, 0x59, 0xB6);
        let rules = [
            BranchColor {
                patterns: "feature/*".into(),
                color: purple,
            },
            BranchColor {
                patterns: "*".into(),
                color: Color32::GRAY,
            },
        ];
        let p = Palette::new(true, &rules);
        assert_eq!(p.ref_fill(RefKind::LocalBranch, false, "feature/x"), purple);
        assert_eq!(
            p.ref_fill(RefKind::RemoteBranch, false, "origin/feature/x"),
            purple
        );
        assert_eq!(
            p.ref_fill(RefKind::LocalBranch, false, "dev/x"),
            Color32::GRAY
        );
        assert_eq!(
            p.ref_fill(RefKind::LocalBranch, true, "feature/x"),
            p.current_branch
        );
        assert_eq!(p.ref_fill(RefKind::Tag, false, "feature/x"), p.tag);
    }

    #[test]
    fn parses_branch_colours() {
        let b = BranchColor::parse("feature/*, bugfix/*=#8e44ad").unwrap();
        assert_eq!(b.patterns, "feature/*, bugfix/*");
        assert_eq!(b.color, Color32::from_rgb(0x8E, 0x44, 0xAD));
        assert_eq!(
            BranchColor::parse("dev/*=0af").unwrap().color,
            Color32::from_rgb(0, 0xAA, 0xFF)
        );
        assert!(BranchColor::parse("dev/*").is_err());
        assert!(BranchColor::parse("dev/*=purple").is_err());
    }

    #[test]
    fn inverting_lightness_keeps_grey_grey() {
        assert_eq!(
            invert_lightness(Color32::from_rgb(224, 224, 224)),
            Color32::from_rgb(31, 31, 31)
        );
        let yellow = invert_lightness(Color32::from_rgb(255, 255, 0));
        assert_eq!(yellow.r(), yellow.g());
        assert!(yellow.b() < yellow.r());
    }
}
