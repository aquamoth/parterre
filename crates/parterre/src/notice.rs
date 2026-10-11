//! `build.rs` includes this file too, so it must not use anything outside `std`.

/// NOTICE's copyright line, without the address.
pub fn copyright_line(notice: &str) -> &str {
    let line = notice
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("Copyright"))
        .unwrap_or_default();
    line.split_once('<').map_or(line, |(name, _)| name).trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_copyright_line_comes_without_the_email() {
        let notice = "parterre, a viewer\nCopyright (C) 2026 Trustfall AB <m@example.com>\n\nMore";
        assert_eq!(copyright_line(notice), "Copyright (C) 2026 Trustfall AB");
        assert_eq!(copyright_line("no such line"), "");
    }
}
