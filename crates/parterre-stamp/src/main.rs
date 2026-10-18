//! The pipeline's tool for a built parterre binary: the version it is published under, and
//! writing that version in (README.md, docs/releasing.md).

#[path = "../../parterre/src/slot.rs"]
#[allow(dead_code)]
mod slot;
#[path = "../../parterre/src/version.rs"]
#[allow(dead_code)]
mod version;

mod pe;

use std::path::Path;
use std::process::{Command, ExitCode};

use version::Ring;

const USAGE: &str = "usage:
  parterre-stamp version --ring canary|nightly|rc|stable [--date YYYYMMDD] [--rev REV]
  parterre-stamp write BINARY VERSION
  parterre-stamp read BINARY";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args[..] {
        ["version", ref rest @ ..] => published_version(rest).map(|v| println!("{v}")),
        ["write", binary, version] => write(Path::new(binary), version),
        ["read", binary] => read(Path::new(binary)),
        _ => Err(USAGE.to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("parterre-stamp: {e}");
            ExitCode::FAILURE
        }
    }
}

fn published_version(args: &[&str]) -> Result<String, String> {
    let (mut ring, mut date, mut rev) = (None, None, "HEAD");
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || args.next().copied().ok_or_else(|| USAGE.to_owned());
        match *arg {
            "--ring" => ring = Some(value()?),
            "--date" => date = Some(value()?),
            "--rev" => rev = value()?,
            _ => return Err(USAGE.to_owned()),
        }
    }
    let ring = match (ring, date) {
        (Some("canary"), None) => Ring::Canary,
        (Some("nightly"), Some(date)) => Ring::Nightly(parse_date(date)?),
        (Some("rc"), None) => Ring::Rc,
        (Some("stable"), None) => Ring::Stable,
        _ => return Err(USAGE.to_owned()),
    };
    let base =
        version::base(git, rev).ok_or_else(|| format!("no version tag in the history of {rev}"))?;
    let short_hash = git(&["rev-parse", "--short=7", &format!("{rev}^{{commit}}")])
        .ok_or_else(|| format!("{rev} isn't a commit"))?;
    Ok(version::published(base, ring, &short_hash))
}

fn parse_date(date: &str) -> Result<u32, String> {
    (date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()))
        .then(|| date.parse().ok())
        .flatten()
        .ok_or_else(|| format!("--date takes YYYYMMDD, not {date}"))
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn write(binary: &Path, version: &str) -> Result<(), String> {
    let at = |e: std::io::Error| format!("{}: {e}", binary.display());
    let before = std::fs::read(binary).map_err(at)?;
    let mut file = before.clone();
    let slot = slot::write(&mut file, version)?;
    let mut changed = if file.starts_with(b"MZ") {
        pe::write(&mut file, version)?
    } else {
        Vec::new()
    };
    changed.push(slot..slot + slot::LEN);
    let stray = (0..file.len())
        .filter(|&i| file[i] != before[i])
        .find(|i| !changed.iter().any(|r| r.contains(i)));
    if let Some(i) = stray {
        return Err(format!("byte {i:#x} changed outside the version"));
    }
    std::fs::write(binary, &file).map_err(at)?;
    println!("{}: {version}", binary.display());
    Ok(())
}

fn read(binary: &Path) -> Result<(), String> {
    let file = std::fs::read(binary).map_err(|e| format!("{}: {e}", binary.display()))?;
    let at = slot::locate(&file)?;
    let slot: &[u8; slot::LEN] = file[at..at + slot::LEN].try_into().unwrap();
    let (version, written) = slot::read(slot);
    println!("{version}{}", if written { " written" } else { "" });
    Ok(())
}
