//! The version slot: room in the binary for the version it is published under, which the
//! pipeline writes in after the build, without rebuilding (#400, #409). 64 bytes: a marker, a
//! byte that is 1 once the version was written, and the version, NUL-padded.
//!
//! `parterre-stamp` includes this file to write the slot, and the app to read it, so it must
//! not use anything outside `std`. The app holds the one slot (`main.rs`) and never names
//! [`MARKER`] at run time, so the marker is found in its binary exactly once.

pub const LEN: usize = 64;
pub const MARKER: &[u8; 16] = b"@parterre-slot@1";
const WRITTEN: usize = MARKER.len();
const VERSION: usize = WRITTEN + 1;
/// The longest version the slot holds.
pub const ROOM: usize = LEN - VERSION;

/// A slot holding `version` as built, unwritten. Fails to compile if it doesn't fit.
pub const fn new(version: &str) -> [u8; LEN] {
    let mut slot = [0; LEN];
    let mut i = 0;
    while i < MARKER.len() {
        slot[i] = MARKER[i];
        i += 1;
    }
    let bytes = version.as_bytes();
    assert!(bytes.len() <= ROOM, "the version doesn't fit the slot");
    let mut i = 0;
    while i < bytes.len() {
        slot[VERSION + i] = bytes[i];
        i += 1;
    }
    slot
}

/// The version in `slot`, and whether it was written after the build.
pub fn read(slot: &[u8; LEN]) -> (String, bool) {
    let version = &slot[VERSION..];
    let end = version
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(version.len());
    (
        String::from_utf8_lossy(&version[..end]).into_owned(),
        slot[WRITTEN] == 1,
    )
}

/// Writes `version` into the one slot in `file`, returning where the slot starts.
pub fn write(file: &mut [u8], version: &str) -> Result<usize, String> {
    if version.len() > ROOM || version.bytes().any(|b| b == 0) {
        return Err(format!("{version:?} doesn't fit the slot's {ROOM} bytes"));
    }
    let found: Vec<usize> = find(file, MARKER).take(2).collect();
    let start = match found[..] {
        [start] => start,
        [] => return Err("no version slot found".to_owned()),
        _ => return Err("the version slot's marker is there more than once".to_owned()),
    };
    if start + LEN > file.len() {
        return Err("the version slot is cut off".to_owned());
    }
    let slot = &mut file[start..start + LEN];
    slot[WRITTEN] = 1;
    slot[VERSION..].fill(0);
    slot[VERSION..VERSION + version.len()].copy_from_slice(version.as_bytes());
    Ok(start)
}

/// Where `needle` starts in `haystack`, each time.
pub fn find<'a>(haystack: &'a [u8], needle: &'a [u8]) -> impl Iterator<Item = usize> + 'a {
    haystack
        .windows(needle.len())
        .enumerate()
        .filter(move |(_, w)| *w == needle)
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_built_slot_reads_as_unwritten() {
        let slot = new("1.2.3-dev+a1b2c3d");
        assert_eq!(read(&slot), ("1.2.3-dev+a1b2c3d".to_owned(), false));
    }

    #[test]
    fn a_written_slot_reads_as_written() {
        let mut file = b"code".to_vec();
        file.extend(new("1.2.3-dev+a1b2c3d.dirty"));
        file.extend(b"more code");
        let before = file.clone();
        assert_eq!(write(&mut file, "1.2.3-canary+a1b2c3d"), Ok(4));
        let slot: &[u8; LEN] = file[4..4 + LEN].try_into().unwrap();
        assert_eq!(read(slot), ("1.2.3-canary+a1b2c3d".to_owned(), true));
        assert_eq!(file[..4], before[..4]);
        assert_eq!(file[4 + LEN..], before[4 + LEN..]);
        // Written again, as a nightly over a canary.
        write(&mut file, "1.2.3-nightly.20261016+a1b2c3d").unwrap();
        let slot: &[u8; LEN] = file[4..4 + LEN].try_into().unwrap();
        assert_eq!(read(slot).0, "1.2.3-nightly.20261016+a1b2c3d");
    }

    #[test]
    fn the_slot_must_be_there_once_and_the_version_fit() {
        assert!(write(&mut b"no slot".to_vec(), "1.2.3").is_err());
        let mut twice = [new("1"), new("2")].concat();
        assert!(
            write(&mut twice, "1.2.3")
                .unwrap_err()
                .contains("more than once")
        );
        let mut file = new("1").to_vec();
        assert!(write(&mut file, &"9".repeat(ROOM + 1)).is_err());
        assert!(write(&mut file, &"9".repeat(ROOM)).is_ok());
    }
}
