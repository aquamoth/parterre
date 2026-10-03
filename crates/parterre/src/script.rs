//! Workflow scripts (`--script`): what a person would do in the window, one step per line, for
//! screenshots and recordings of any window, dialog or menu. See `docs/automation.md`.
//!
//! ```text
//! # Create a branch from the context menu of v0.2.0.
//! right-click node:v0.2.0
//! click "Create branch…"
//! type "feature/demo"
//! screenshot branch.png window
//! key Enter
//! ```

use std::path::PathBuf;

use eframe::egui::{Key, Modifiers, Vec2, vec2};

/// What a pointer step aims at.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// Text on screen: the topmost exact match, else the topmost containing it.
    Text(String),
    /// The node of a ref name or hash prefix.
    Node(String),
    /// The empty spot of the canvas farthest from any node.
    Canvas,
    /// A toolbar button: `menu`, `filter`, `zoom` or `drag`.
    Toolbar(String),
    /// A point of the window, in points from its top left.
    Point(Vec2),
}

/// What part of the window a screenshot keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Crop {
    Full,
    /// The topmost window (a dialog, embedded in the main window in scripted runs).
    Window,
    /// The open menus and popovers.
    Popup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Primary,
    Secondary,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Do nothing for this many seconds.
    Wait(f32),
    /// Wait for the target to show.
    WaitFor(Target),
    Click {
        target: Target,
        button: Button,
        count: u32,
    },
    Hover(Target),
    /// Press on the target, move by this much and let go.
    Drag(Target, Vec2),
    /// Turn the mouse wheel by this many points where the pointer is.
    Scroll(Vec2),
    Key(Modifiers, Key),
    Type(String),
    /// Open a window or dialog directly, e.g. `about` or `log:main`; see `docs/automation.md`.
    Open(String),
    Screenshot(PathBuf, Crop),
}

/// A step and the line it came from, for error messages.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub number: usize,
    pub step: Step,
}

pub fn parse(text: &str) -> Result<Vec<Line>, String> {
    text.lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let words = match split(line) {
                Ok(words) => words,
                Err(e) => return Some(Err(format!("line {}: {e}", i + 1))),
            };
            (!words.is_empty()).then(|| {
                step(&words)
                    .map(|step| Line {
                        number: i + 1,
                        step,
                    })
                    .map_err(|e| format!("line {}: {e}", i + 1))
            })
        })
        .collect()
}

/// The words of a line: separated by spaces, `"quoted"` ones may hold spaces (`\"` and `\\`
/// inside), and `#` outside quotes starts a comment.
fn split(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '#' {
            break;
        } else if c == '"' {
            chars.next();
            let mut word = String::new();
            loop {
                match chars.next() {
                    Some('"') => break,
                    Some('\\') => word.extend(chars.next()),
                    Some(c) => word.push(c),
                    None => return Err("unclosed quote".into()),
                }
            }
            // Marked as quoted, so `"canvas"` is text rather than the canvas.
            words.push(format!("\"{word}"));
        } else {
            let mut word = String::new();
            while let Some(&c) = chars.peek()
                && !c.is_whitespace()
            {
                word.push(c);
                chars.next();
            }
            words.push(word);
        }
    }
    Ok(words)
}

fn step(words: &[String]) -> Result<Step, String> {
    let command = words[0].as_str();
    let args = &words[1..];
    let count = |n: usize| {
        if args.len() == n {
            Ok(())
        } else {
            Err(format!(
                "{command} takes {n} argument(s), not {}",
                args.len()
            ))
        }
    };
    let click = |button, count| -> Result<Step, String> {
        Ok(Step::Click {
            target: target(args.first().ok_or("missing target")?)?,
            button,
            count,
        })
    };
    let step = match command {
        "wait" => {
            count(1)?;
            let seconds: f32 = args[0].parse().map_err(|_| "expected seconds")?;
            Step::Wait(seconds)
        }
        "wait-for" => {
            count(1)?;
            Step::WaitFor(target(&args[0])?)
        }
        "click" => {
            count(1)?;
            click(Button::Primary, 1)?
        }
        "double-click" => {
            count(1)?;
            click(Button::Primary, 2)?
        }
        "right-click" => {
            count(1)?;
            click(Button::Secondary, 1)?
        }
        "hover" => {
            count(1)?;
            Step::Hover(target(&args[0])?)
        }
        "drag" => {
            count(2)?;
            Step::Drag(target(&args[0])?, pair(&args[1])?)
        }
        "scroll" => {
            count(1)?;
            Step::Scroll(pair(&args[0])?)
        }
        "key" => {
            count(1)?;
            let (modifiers, key) = key(&args[0])?;
            Step::Key(modifiers, key)
        }
        "type" => {
            count(1)?;
            Step::Type(quoted(&args[0]).ok_or("type takes quoted text")?.to_owned())
        }
        "open" => {
            count(1)?;
            Step::Open(args[0].clone())
        }
        "screenshot" => {
            if args.is_empty() || args.len() > 2 {
                return Err("screenshot takes a file and optionally window or popup".into());
            }
            let file = quoted(&args[0]).unwrap_or(&args[0]);
            let crop = match args.get(1).map(String::as_str) {
                None | Some("full") => Crop::Full,
                Some("window") => Crop::Window,
                Some("popup") => Crop::Popup,
                Some(other) => return Err(format!("unknown crop {other}: full, window or popup")),
            };
            Step::Screenshot(PathBuf::from(file), crop)
        }
        _ => return Err(format!("unknown step {command}")),
    };
    Ok(step)
}

fn quoted(word: &str) -> Option<&str> {
    word.strip_prefix('"')
}

fn target(word: &str) -> Result<Target, String> {
    if let Some(text) = quoted(word) {
        return Ok(Target::Text(text.to_owned()));
    }
    if word == "canvas" {
        return Ok(Target::Canvas);
    }
    if let Some(name) = word.strip_prefix("node:") {
        return Ok(Target::Node(name.to_owned()));
    }
    if let Some(name) = word.strip_prefix("toolbar:") {
        return match name {
            "menu" | "filter" | "zoom" | "drag" => Ok(Target::Toolbar(name.to_owned())),
            _ => Err(format!(
                "unknown toolbar button {name}: menu, filter, zoom or drag"
            )),
        };
    }
    pair(word).map(Target::Point).map_err(|_| {
        format!("unknown target {word}: \"text\", node:REF, canvas, toolbar:BUTTON or X,Y")
    })
}

fn pair(word: &str) -> Result<Vec2, String> {
    let (x, y) = word.split_once(',').ok_or("expected X,Y")?;
    match (x.trim().parse(), y.trim().parse()) {
        (Ok(x), Ok(y)) => Ok(vec2(x, y)),
        _ => Err(format!("expected X,Y, not {word}")),
    }
}

/// `Enter`, `Ctrl+O`, `Ctrl+Shift+Z`: modifiers, then one of egui's key names.
fn key(word: &str) -> Result<(Modifiers, Key), String> {
    let mut parts: Vec<&str> = word.split('+').collect();
    // `Ctrl++` and `+` name the plus key.
    if word.ends_with("++") || word == "+" {
        parts.retain(|p| !p.is_empty());
        parts.push("+");
    }
    let name = parts.pop().unwrap_or_default();
    let mut modifiers = Modifiers::NONE;
    for m in parts {
        modifiers |= match m.to_ascii_lowercase().as_str() {
            "ctrl" => Modifiers::CTRL,
            "shift" => Modifiers::SHIFT,
            "alt" => Modifiers::ALT,
            "cmd" | "command" => Modifiers::COMMAND,
            _ => return Err(format!("unknown modifier {m}")),
        };
    }
    let key = Key::from_name(name)
        .or_else(|| Key::from_name(&name.to_ascii_uppercase()))
        .or_else(|| {
            let mut c = name.chars();
            let first = c.next()?.to_ascii_uppercase();
            Key::from_name(&format!("{first}{}", c.as_str().to_ascii_lowercase()))
        })
        .ok_or_else(|| format!("unknown key {name}"))?;
    Ok((modifiers, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(line: &str) -> Step {
        parse(line).unwrap().remove(0).step
    }

    #[test]
    fn reads_steps_and_skips_comments() {
        let lines =
            parse("# a comment\n\nright-click node:v0.2.0  # the tag\nclick \"Create branch…\"\n")
                .unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].number, 3);
        assert_eq!(
            lines[0].step,
            Step::Click {
                target: Target::Node("v0.2.0".into()),
                button: Button::Secondary,
                count: 1,
            }
        );
        assert_eq!(
            lines[1].step,
            Step::Click {
                target: Target::Text("Create branch…".into()),
                button: Button::Primary,
                count: 1,
            }
        );
    }

    #[test]
    fn reads_targets() {
        assert_eq!(one("hover canvas"), Step::Hover(Target::Canvas));
        assert_eq!(
            one("hover \"canvas\""),
            Step::Hover(Target::Text("canvas".into()))
        );
        assert_eq!(
            one("hover toolbar:menu"),
            Step::Hover(Target::Toolbar("menu".into()))
        );
        assert_eq!(
            one("drag 10,20 -30,4.5"),
            Step::Drag(Target::Point(vec2(10.0, 20.0)), vec2(-30.0, 4.5))
        );
        assert_eq!(
            one(r#"type "say \"hi\" # not a comment""#),
            Step::Type(r#"say "hi" # not a comment"#.into())
        );
    }

    #[test]
    fn reads_keys() {
        assert_eq!(one("key Enter"), Step::Key(Modifiers::NONE, Key::Enter));
        assert_eq!(one("key escape"), Step::Key(Modifiers::NONE, Key::Escape));
        assert_eq!(
            one("key Ctrl+Shift+z"),
            Step::Key(Modifiers::CTRL | Modifiers::SHIFT, Key::Z)
        );
        assert_eq!(one("key Ctrl++"), Step::Key(Modifiers::CTRL, Key::Plus));
        assert_eq!(one("key F5"), Step::Key(Modifiers::NONE, Key::F5));
    }

    #[test]
    fn reads_screenshots() {
        assert_eq!(
            one("screenshot out/a.png window"),
            Step::Screenshot("out/a.png".into(), Crop::Window)
        );
        assert_eq!(
            one("screenshot \"my menu.png\""),
            Step::Screenshot("my menu.png".into(), Crop::Full)
        );
    }

    #[test]
    fn says_where_it_went_wrong() {
        assert_eq!(
            parse("wait 1\nclik canvas").unwrap_err(),
            "line 2: unknown step clik"
        );
        assert!(parse("type \"open").unwrap_err().contains("unclosed quote"));
        assert!(parse("click here").unwrap_err().contains("unknown target"));
        assert!(
            parse("key Hyper+A")
                .unwrap_err()
                .contains("unknown modifier")
        );
        assert!(parse("screenshot a.png all").is_err());
    }
}
