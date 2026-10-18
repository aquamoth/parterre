//! Windows' version resource, written in place: the `FileVersion` and `ProductVersion` strings,
//! which `build.rs` pads with spaces to make room, and the numbers of `VS_FIXEDFILEINFO`
//! (#400). Nothing moves.

use std::ops::Range;

const FIXED_SIGNATURE: u32 = 0xFEEF_04BD;

/// Writes `version` (`1.2.3-rc+a1b2c3d`) into the version resource of the executable `file`,
/// returning the ranges it may have changed.
pub fn write(file: &mut [u8], version: &str) -> Result<Vec<Range<usize>>, String> {
    let numbers = numbers(version)?;
    let info = find_once(file, &key("VS_VERSION_INFO"), "VS_VERSION_INFO")?;
    // The structure starts with three WORDs before its key, and its value, VS_FIXEDFILEINFO,
    // follows the key at the next 32-bit boundary.
    let header = info.checked_sub(6).ok_or("VS_VERSION_INFO is cut off")?;
    let fixed = header + align(6 + key("VS_VERSION_INFO").len());
    if read_u32(file, fixed) != Some(FIXED_SIGNATURE) {
        return Err("VS_FIXEDFILEINFO isn't after VS_VERSION_INFO".to_owned());
    }
    let [major, minor, patch] = numbers.map(u32::from);
    let ms = (major << 16) | minor;
    let ls = patch << 16;
    // dwFileVersionMS, LS, dwProductVersionMS, LS.
    let numbers_at = fixed + 8..fixed + 24;
    if file.len() < numbers_at.end {
        return Err("VS_FIXEDFILEINFO is cut off".to_owned());
    }
    for (i, value) in [ms, ls, ms, ls].into_iter().enumerate() {
        let at = numbers_at.start + 4 * i;
        file[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    let mut changed = vec![numbers_at];
    for name in ["FileVersion", "ProductVersion"] {
        changed.push(write_string(file, header, name, version)?);
    }
    Ok(changed)
}

/// A `String` of the `StringTable`: `wLength`, `wValueLength` (in UTF-16 units), `wType`, the
/// key, padding to 32 bits from `base`, then the value.
fn write_string(
    file: &mut [u8],
    base: usize,
    name: &str,
    version: &str,
) -> Result<Range<usize>, String> {
    let key = key(name);
    let at = find_once(file, &key, name)?;
    let header = at
        .checked_sub(6)
        .ok_or_else(|| format!("{name} is cut off"))?;
    let units = read_u16(file, header + 2).ok_or_else(|| format!("{name} is cut off"))? as usize;
    let value = base + align(header - base + 6 + key.len());
    let room = value..value + 2 * units;
    let wide: Vec<u8> = version.encode_utf16().flat_map(u16::to_le_bytes).collect();
    // One unit left for the terminator.
    if wide.len() + 2 > room.len() {
        return Err(format!(
            "{version} is longer than the {} characters {name} has room for",
            units.saturating_sub(1)
        ));
    }
    if file.len() < room.end {
        return Err(format!("{name} is cut off"));
    }
    file[room.clone()].fill(0);
    file[value..value + wide.len()].copy_from_slice(&wide);
    Ok(room)
}

/// `1.2.3` of `1.2.3-rc+a1b2c3d`, each a number Windows' versions hold.
fn numbers(version: &str) -> Result<[u16; 3], String> {
    let numbers = version.split(['-', '+']).next().unwrap_or(version);
    let parts: Vec<Option<u16>> = numbers.split('.').map(|n| n.parse().ok()).collect();
    match parts[..] {
        [Some(major), Some(minor), Some(patch)] => Ok([major, minor, patch]),
        _ => Err(format!("{version} doesn't fit a Windows version number")),
    }
}

/// `name` in UTF-16 with its terminator, as the resource keys are written.
fn key(name: &str) -> Vec<u8> {
    name.encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn find_once(file: &[u8], needle: &[u8], name: &str) -> Result<usize, String> {
    let mut found = crate::slot::find(file, needle);
    let at = found
        .next()
        .ok_or_else(|| format!("no {name} in the file"))?;
    if found.next().is_some() {
        return Err(format!("{name} is in the file more than once"));
    }
    Ok(at)
}

fn align(n: usize) -> usize {
    n.next_multiple_of(4)
}

fn read_u16(file: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(file.get(at..at + 2)?.try_into().ok()?))
}

fn read_u32(file: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(file.get(at..at + 4)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A structure of the version resource: header, key, padding, value, children.
    fn node(name: &str, value: &[u8], value_units: u16, kind: u16, children: &[u8]) -> Vec<u8> {
        let mut out = vec![0; 6];
        out.extend(key(name));
        out.resize(align(out.len()), 0);
        out.extend(value);
        out.resize(align(out.len()), 0);
        out.extend(children);
        let len = out.len() as u16;
        out[0..2].copy_from_slice(&len.to_le_bytes());
        out[2..4].copy_from_slice(&value_units.to_le_bytes());
        out[4..6].copy_from_slice(&kind.to_le_bytes());
        out
    }

    fn string(name: &str, value: &str) -> Vec<u8> {
        let wide: Vec<u8> = value
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect();
        node(name, &wide, (wide.len() / 2) as u16, 1, &[])
    }

    fn fixed(ms: u32, ls: u32) -> Vec<u8> {
        let mut out = Vec::new();
        for value in [
            FIXED_SIGNATURE,
            0x1_0000,
            ms,
            ls,
            ms,
            ls,
            0x3f,
            0,
            0x40004,
            1,
            0,
            0,
            0,
        ] {
            out.extend(value.to_le_bytes());
        }
        out
    }

    /// As `rc.exe` compiles `build.rs`'s script, behind some other bytes.
    fn executable(version: &str) -> Vec<u8> {
        let padded = format!("{version:<47}");
        let table = node(
            "040904B0",
            &[],
            0,
            1,
            &[
                string("CompanyName", "Trustfall AB"),
                string("FileVersion", &padded),
                string("ProductVersion", &padded),
            ]
            .concat(),
        );
        let strings = node("StringFileInfo", &[], 0, 1, &table);
        let info = node(
            "VS_VERSION_INFO",
            &fixed(0x1_0002, 0x3_0000),
            52,
            0,
            &strings,
        );
        [
            b"MZ code and data ".to_vec(),
            vec![0; 3],
            info,
            b"more".to_vec(),
        ]
        .concat()
    }

    fn read_string(file: &[u8], name: &str) -> String {
        let at = crate::slot::find(file, &key(name)).next().unwrap() + key(name).len();
        let units: Vec<u16> = file[align(at)..]
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&u| u != 0)
            .collect();
        String::from_utf16(&units).unwrap()
    }

    #[test]
    fn the_strings_and_numbers_are_written_in_place() {
        let mut file = executable("1.2.3-dev+a1b2c3d");
        let before = file.clone();
        assert_eq!(
            read_string(&file, "ProductVersion").trim_end(),
            "1.2.3-dev+a1b2c3d"
        );
        let changed = write(&mut file, "1.4.7-nightly.20261016+a1b2c3d").unwrap();
        assert_eq!(file.len(), before.len());
        for name in ["FileVersion", "ProductVersion"] {
            assert_eq!(read_string(&file, name), "1.4.7-nightly.20261016+a1b2c3d");
        }
        assert_eq!(read_string(&file, "CompanyName"), "Trustfall AB");
        let fixed_at = crate::slot::find(&file, &FIXED_SIGNATURE.to_le_bytes())
            .next()
            .unwrap();
        for (i, expected) in [0x1_0004u32, 0x7_0000, 0x1_0004, 0x7_0000]
            .into_iter()
            .enumerate()
        {
            assert_eq!(read_u32(&file, fixed_at + 8 + 4 * i), Some(expected));
        }
        let outside = (0..file.len()).filter(|i| !changed.iter().any(|r| r.contains(i)));
        assert!(outside.into_iter().all(|i| file[i] == before[i]));
    }

    #[test]
    fn a_version_too_long_or_not_numbered_is_refused() {
        let mut file = executable("1.2.3-dev+a1b2c3d");
        assert!(write(&mut file, &format!("1.2.3-{}", "x".repeat(60))).is_err());
        assert!(write(&mut file, "1.2.70000").is_err());
        assert!(write(&mut b"MZ nothing".to_vec(), "1.2.3").is_err());
    }
}
