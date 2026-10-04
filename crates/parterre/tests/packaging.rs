//! Every channel declares the git parterre needs (#233), so that raising
//! `parterre_core::git::MINIMUM_VERSION` fails here until each of them is raised too.

use std::path::Path;

use parterre_core::git::MINIMUM_VERSION;

fn read(path: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn minimum() -> String {
    format!("{}.{}", MINIMUM_VERSION.0, MINIMUM_VERSION.1)
}

fn assert_declares(path: &str, declaration: &str) {
    assert!(
        read(path).contains(declaration),
        "{path} should declare `{declaration}`"
    );
}

#[test]
fn the_deb_depends_on_the_minimum_git() {
    // Debian and Ubuntu give git epoch 1; without it every git would pass.
    assert_declares(
        "crates/parterre/Cargo.toml",
        &format!("git (>= 1:{})", minimum()),
    );
}

#[test]
fn the_rpm_requires_the_minimum_git() {
    assert_declares(
        "crates/parterre/Cargo.toml",
        &format!("git-core = \">= {}\"", minimum()),
    );
}

#[test]
fn the_chocolatey_package_depends_on_the_minimum_git() {
    assert_declares(
        "packaging/chocolatey/parterre.nuspec",
        &format!("<dependency id=\"git\" version=\"{}.0\" />", minimum()),
    );
}

#[test]
fn the_winget_manifest_is_documented_with_the_minimum_git() {
    // The manifest lives in microsoft/winget-pkgs, made from this (#16).
    assert_declares(
        "docs/distribution.md",
        &format!("`MinimumVersion: {}.0`", minimum()),
    );
}

#[test]
fn the_readme_names_the_minimum_git() {
    assert_declares("README.md", &format!("git {} or newer", minimum()));
}

#[test]
fn the_flatpak_bundles_git_at_least_as_new_as_the_minimum() {
    let manifest = read("packaging/flatpak/se.trustfall.parterre.yml");
    let version = manifest
        .split("/git-")
        .nth(1)
        .and_then(|rest| rest.split(".tar").next())
        .expect("the manifest names a git tarball");
    let mut parts = version.split('.').map(|p| p.parse::<u32>().unwrap());
    let bundled = (parts.next().unwrap(), parts.next().unwrap());
    assert!(
        bundled >= MINIMUM_VERSION,
        "the Flatpak bundles git {version}, older than {}",
        minimum()
    );
}
