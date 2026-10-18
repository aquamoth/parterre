use std::path::PathBuf;

use eframe::egui::{Key, Modifiers, PointerButton, Vec2, vec2};

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Text(String),
    /// A ref name or hash prefix.
    Node(String),
    Canvas,
    Toolbar(String),
    Point(Vec2),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Crop {
    Full,
    Window,
    Popup,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Wait {
        seconds: f32,
    },
    WaitFor(Target),
    Click {
        target: Target,
        button: PointerButton,
        count: u32,
        modifiers: Modifiers,
    },
    Hover(Target),
    Drag {
        target: Target,
        by: Vec2,
    },
    Scroll(Vec2),
    Key(Modifiers, Key),
    Type(String),
    Open(String),
    DragFile(PathBuf),
    DropFile,
    Screenshot(PathBuf, Crop),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub number: usize,
    pub step: Step,
}

pub fn parse(text: &str) -> Result<Vec<Line>, String> {
    let mut lines = Vec::new();
    for (line, number) in text.lines().zip(1..) {
        let words = split(line).map_err(|e| format!("line {number}: {e}"))?;
        let Some((command, args)) = words.split_first() else {
            continue;
        };
        let step = step(command, args).map_err(|e| format!("line {number}: {e}"))?;
        lines.push(Line { number, step });
    }
    Ok(lines)
}

/// Words are separated by spaces, `"quoted"` ones may hold spaces (`\"` and `\\` inside), and
/// `#` outside quotes starts a comment.
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

fn step(command: &str, args: &[String]) -> Result<Step, String> {
    let expect_args = |n: usize| {
        if args.len() == n {
            Ok(())
        } else {
            Err(format!(
                "{command} takes {n} argument(s), not {}",
                args.len()
            ))
        }
    };
    let click = |button, count, modifiers| -> Result<Step, String> {
        expect_args(1)?;
        Ok(Step::Click {
            target: target(&args[0])?,
            button,
            count,
            modifiers,
        })
    };
    Ok(match command {
        "wait" => {
            expect_args(1)?;
            Step::Wait {
                seconds: args[0].parse().map_err(|_| "expected seconds")?,
            }
        }
        "wait-for" => {
            expect_args(1)?;
            Step::WaitFor(target(&args[0])?)
        }
        "click" => click(PointerButton::Primary, 1, Modifiers::NONE)?,
        "ctrl-click" => click(PointerButton::Primary, 1, Modifiers::COMMAND)?,
        "shift-click" => click(PointerButton::Primary, 1, Modifiers::SHIFT)?,
        "double-click" => click(PointerButton::Primary, 2, Modifiers::NONE)?,
        "right-click" => click(PointerButton::Secondary, 1, Modifiers::NONE)?,
        "hover" => {
            expect_args(1)?;
            Step::Hover(target(&args[0])?)
        }
        "drag" => {
            expect_args(2)?;
            Step::Drag {
                target: target(&args[0])?,
                by: pair(&args[1])?,
            }
        }
        "scroll" => {
            expect_args(1)?;
            Step::Scroll(pair(&args[0])?)
        }
        "key" => {
            expect_args(1)?;
            let (modifiers, key) = key(&args[0])?;
            Step::Key(modifiers, key)
        }
        "type" => {
            expect_args(1)?;
            Step::Type(quoted(&args[0]).ok_or("type takes quoted text")?.to_owned())
        }
        "open" => {
            expect_args(1)?;
            Step::Open(args[0].clone())
        }
        "drag-file" => {
            expect_args(1)?;
            Step::DragFile(PathBuf::from(unquoted(&args[0])))
        }
        "drop-file" => {
            expect_args(0)?;
            Step::DropFile
        }
        "screenshot" => {
            if args.is_empty() || args.len() > 2 {
                return Err("screenshot takes a file and optionally window or popup".into());
            }
            let crop = match args.get(1).map(String::as_str) {
                None | Some("full") => Crop::Full,
                Some("window") => Crop::Window,
                Some("popup") => Crop::Popup,
                Some(other) => return Err(format!("unknown crop {other}: full, window or popup")),
            };
            Step::Screenshot(PathBuf::from(unquoted(&args[0])), crop)
        }
        _ => return Err(format!("unknown step {command}")),
    })
}

fn quoted(word: &str) -> Option<&str> {
    word.strip_prefix('"')
}

fn unquoted(word: &str) -> &str {
    quoted(word).unwrap_or(word)
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
            "filter" | "zoom" | "drag" => Ok(Target::Toolbar(name.to_owned())),
            _ => Err(format!(
                "unknown toolbar button {name}: filter, zoom or drag"
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

/// `Ctrl+Shift+Z`: modifiers, then one of egui's key names in any case.
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
                button: PointerButton::Secondary,
                count: 1,
                modifiers: Modifiers::NONE,
            }
        );
        assert_eq!(
            lines[1].step,
            Step::Click {
                target: Target::Text("Create branch…".into()),
                button: PointerButton::Primary,
                count: 1,
                modifiers: Modifiers::NONE,
            }
        );
        assert_eq!(
            one("shift-click \"fix\""),
            Step::Click {
                target: Target::Text("fix".into()),
                button: PointerButton::Primary,
                count: 1,
                modifiers: Modifiers::SHIFT,
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
            one("hover toolbar:zoom"),
            Step::Hover(Target::Toolbar("zoom".into()))
        );
        assert_eq!(
            one("drag 10,20 -30,4.5"),
            Step::Drag {
                target: Target::Point(vec2(10.0, 20.0)),
                by: vec2(-30.0, 4.5)
            }
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
        assert_eq!(
            one("drag-file \"/tmp/a repo\""),
            Step::DragFile(PathBuf::from("/tmp/a repo"))
        );
        assert_eq!(one("drop-file"), Step::DropFile);
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
