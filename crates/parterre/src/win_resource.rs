//! The resource script `build.rs` compiles into the Windows executable: the icon, and the version
//! information Explorer shows under *Properties → Details* (product name, versions, copyright).
//!
//! Like `version.rs`, `build.rs` includes this file and the app compiles it only for its tests,
//! so it must not use anything outside `std`.

use std::fmt::Write as _;

/// What goes into the script.
#[derive(Debug)]
pub struct Resources<'a> {
    /// Path of the `.ico` file.
    pub icon: &'a str,
    /// `major.minor.patch`, which Explorer shows as the file version (with a fourth part, 0).
    pub numeric_version: [u16; 3],
    /// The full version string (`PARTERRE_VERSION`), shown as the product version.
    pub version: &'a str,
    /// The package description, `CARGO_PKG_DESCRIPTION`.
    pub description: &'a str,
    /// The copyright line of `NOTICE`.
    pub notice: &'a str,
}

/// The resource script for `resources`.
pub fn script(resources: &Resources) -> String {
    let [major, minor, patch] = resources.numeric_version;
    let numeric = format!("{major},{minor},{patch},0");
    let strings = [
        // The publisher and copyright holder (see `docs/distribution.md`).
        ("CompanyName", "Trustfall AB"),
        // Also the name Task Manager lists the process under.
        ("FileDescription", "parterre"),
        ("FileVersion", resources.version),
        ("InternalName", "parterre"),
        ("LegalCopyright", copyright(resources.notice)),
        ("OriginalFilename", "parterre.exe"),
        ("ProductName", "parterre"),
        ("ProductVersion", resources.version),
        ("Comments", resources.description),
    ];

    // Both resource compilers take forward slashes, which need no escaping.
    let icon = resources.icon.replace('\\', "/");
    let mut rc = format!(
        "1 ICON \"{icon}\"\n\
         \n\
         1 VERSIONINFO\n\
         FILEVERSION {numeric}\n\
         PRODUCTVERSION {numeric}\n\
         FILEFLAGSMASK 0x3F\n\
         FILEFLAGS 0x0\n\
         FILEOS 0x40004\n\
         FILETYPE 0x1\n\
         FILESUBTYPE 0x0\n\
         BEGIN\n\
         \x20   BLOCK \"StringFileInfo\"\n\
         \x20   BEGIN\n\
         \x20       BLOCK \"040904B0\"\n\
         \x20       BEGIN\n"
    );
    for (key, value) in strings {
        writeln!(rc, "            VALUE \"{key}\", {}", wide_string(value)).unwrap();
    }
    // US English, Unicode: the string block above.
    rc.push_str(
        "        END\n    \
             END\n    \
             BLOCK \"VarFileInfo\"\n    \
             BEGIN\n        \
                 VALUE \"Translation\", 0x409, 1200\n    \
             END\n\
         END\n",
    );
    rc
}

/// The first line of `notice` that starts with "Copyright", without the email address.
fn copyright(notice: &str) -> &str {
    let line = notice
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("Copyright"))
        .unwrap_or_default();
    line.split_once('<').map_or(line, |(name, _)| name).trim()
}

/// `s` as a wide string literal of a resource script. Anything outside ASCII is escaped as UTF-16
/// code units, so the script is plain ASCII and needs no code page, which the resource compilers
/// would otherwise guess differently.
fn wide_string(s: &str) -> String {
    let mut literal = String::from("L\"");
    for c in s.chars() {
        match c {
            '"' => literal.push_str("\"\""),
            '\\' => literal.push_str("\\\\"),
            ' '..='~' => literal.push(c),
            _ => {
                for unit in c.encode_utf16(&mut [0; 2]) {
                    write!(literal, "\\x{unit:04X}").unwrap();
                }
            }
        }
    }
    literal.push('"');
    literal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copyright_comes_from_the_notice_without_the_email() {
        let notice = "parterre, a viewer\nCopyright (C) 2026 Trustfall AB <m@example.com>\n\nMore";
        assert_eq!(copyright(notice), "Copyright (C) 2026 Trustfall AB");
        assert_eq!(copyright("no such line"), "");
    }

    #[test]
    fn wide_strings_are_ascii_with_escapes() {
        assert_eq!(wide_string("plain"), r#"L"plain""#);
        assert_eq!(wide_string(r#"a "b" c\d"#), r#"L"a ""b"" c\\d""#);
        assert_eq!(wide_string("Enköping ©"), r#"L"Enk\x00F6ping \x00A9""#);
        // Outside the BMP: a surrogate pair.
        assert_eq!(wide_string("🌳"), r#"L"\xD83C\xDF33""#);
    }

    #[test]
    fn script_holds_the_icon_and_version_information() {
        let rc = script(&Resources {
            icon: r"C:\src\parterre.ico",
            numeric_version: [0, 4, 1],
            version: "0.4.1 (a1b2c3d)",
            description: "A viewer.",
            notice: "Copyright (C) 2026 Trustfall AB <m@example.com>",
        });
        assert!(rc.starts_with("1 ICON \"C:/src/parterre.ico\"\n"));
        assert!(rc.contains("FILEVERSION 0,4,1,0\n"));
        assert!(rc.contains("PRODUCTVERSION 0,4,1,0\n"));
        assert!(rc.contains(r#"VALUE "ProductName", L"parterre""#));
        assert!(rc.contains(r#"VALUE "ProductVersion", L"0.4.1 (a1b2c3d)""#));
        assert!(rc.contains(r#"VALUE "CompanyName", L"Trustfall AB""#));
        assert!(rc.contains(r#"VALUE "LegalCopyright", L"Copyright (C) 2026 Trustfall AB""#));
        assert!(rc.contains(r#"VALUE "Translation", 0x409, 1200"#));
        assert!(rc.is_ascii());
    }
}
