//! *Update now*'s download (#431): the release file, checked against the release's
//! `SHA256SUMS`. Saved by parterre rather than a browser, it carries no Mark of the Web or
//! quarantine, so neither SmartScreen nor Gatekeeper asks about it.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::{DownloadError, Install};

const MAX_SUMS_BYTES: u64 = 64 * 1024;
/// Far more than any release file: only a guard against a response that never ends.
const MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;

pub(crate) fn download(
    install: &Install,
    dir: &Path,
    progress: &dyn Fn(u64, Option<u64>),
    cancelled: &dyn Fn() -> bool,
) -> Result<PathBuf, DownloadError> {
    let agent = crate::github::agent(None);
    let sums = agent
        .get(&install.sums)
        .call()
        .and_then(|mut r| {
            r.body_mut()
                .with_config()
                .limit(MAX_SUMS_BYTES)
                .read_to_string()
        })
        .map_err(|e| failed(format!("couldn't read the release's checksums: {e}")))?;
    let expected = sum_of(&sums, &install.file).ok_or_else(|| {
        failed(format!(
            "the release's checksums don't list {}",
            install.file
        ))
    })?;
    std::fs::create_dir_all(dir)
        .map_err(|e| failed(format!("couldn't create {}: {e}", dir.display())))?;
    let path = dir.join(&install.file);
    let part = dir.join(format!("{}.part", install.file));
    let result = fetch(&agent, &install.url, &part, progress, cancelled).and_then(|actual| {
        if actual != expected {
            return Err(failed(format!(
                "{} doesn't match the release's checksum",
                install.file
            )));
        }
        std::fs::rename(&part, &path)
            .map_err(|e| failed(format!("couldn't save {}: {e}", path.display())))
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result.map(|()| path)
}

/// Writes `url`'s body to `to`, and returns its SHA-256 in lower-case hex.
fn fetch(
    agent: &ureq::Agent,
    url: &str,
    to: &Path,
    progress: &dyn Fn(u64, Option<u64>),
    cancelled: &dyn Fn() -> bool,
) -> Result<String, DownloadError> {
    let mut response = agent
        .get(url)
        .call()
        .map_err(|e| failed(format!("couldn't download {url}: {e}")))?;
    let size = response.body().content_length();
    let mut body = response
        .body_mut()
        .with_config()
        .limit(MAX_FILE_BYTES)
        .reader();
    let write_error = |e: std::io::Error| failed(format!("couldn't write {}: {e}", to.display()));
    let mut file = File::create(to).map_err(write_error)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 64 * 1024];
    let mut done = 0;
    loop {
        if cancelled() {
            return Err(DownloadError::Cancelled);
        }
        let read = body
            .read(&mut buffer)
            .map_err(|e| failed(format!("the download broke off: {e}")))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        file.write_all(&buffer[..read]).map_err(write_error)?;
        done += read as u64;
        progress(done, size);
    }
    file.sync_all().map_err(write_error)?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn failed(message: String) -> DownloadError {
    DownloadError::Failed(message)
}

/// The SHA-256 that `sums`, as `sha256sum` writes them, gives `file`: in lower-case hex.
fn sum_of(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (sum, name) = line.split_once(' ')?;
        // Two spaces before the name in text mode, a space and a `*` in binary mode.
        let name = name.strip_prefix([' ', '*'])?;
        let is_sum = sum.len() == 64 && sum.bytes().all(|b| b.is_ascii_hexdigit());
        (is_sum && name == file).then(|| sum.to_ascii_lowercase())
    })
}

#[cfg(test)]
mod tests {
    use super::sum_of;

    #[test]
    fn the_sum_is_found_by_its_file_name() {
        let msi = "a".repeat(64);
        let zip = "B".repeat(64);
        let sums = format!(
            "{msi}  parterre-1.2.3-x86_64-pc-windows-msvc.msi\n\
             {zip} *parterre-1.2.3-x86_64-pc-windows-msvc.zip\r\n\
             0123  parterre-1.2.3-x86_64-apple-darwin.dmg\n"
        );
        assert_eq!(
            sum_of(&sums, "parterre-1.2.3-x86_64-pc-windows-msvc.msi"),
            Some(msi)
        );
        assert_eq!(
            sum_of(&sums, "parterre-1.2.3-x86_64-pc-windows-msvc.zip"),
            Some("b".repeat(64))
        );
        // Not a SHA-256.
        assert_eq!(
            sum_of(&sums, "parterre-1.2.3-x86_64-apple-darwin.dmg"),
            None
        );
        assert_eq!(sum_of(&sums, "parterre-1.2.3-x86_64-pc-windows"), None);
        assert_eq!(sum_of("", "parterre.msi"), None);
    }
}
