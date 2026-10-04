#!/usr/bin/env python3
"""Make a clean, disposable checkout with Cargo versions generated from a release tag."""

import argparse
import pathlib
import re
import shlex
import subprocess
import tempfile
import tomllib

ROOT = pathlib.Path(__file__).resolve().parent.parent
VERSION = re.compile(r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?")
# The workspace's crates, all published at parterre's version.
CRATES = ["parterre", "parterre-util", "parterre-core", "parterre-forge", "parterre-highlight", "parterre-telemetry"]
LOCK_PACKAGE = re.compile(r'(?m)^(\[\[package\]\]\nname = "(?:' + "|".join(CRATES) + r')"\nversion = ")([^"]+)(")')


def run(*args: str, cwd: pathlib.Path = ROOT) -> str:
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


def replace_once(source: str, old: str, new: str) -> str:
    if source.count(old) != 1:
        raise ValueError(f"expected exactly one occurrence of {old!r}")
    return source.replace(old, new, 1)


def update_versions(checkout: pathlib.Path, version: str) -> bool:
    manifest = checkout / "Cargo.toml"
    source = manifest.read_text()
    data = tomllib.loads(source)
    old = data["workspace"]["package"]["version"]
    internal = [name for name in CRATES if name != "parterre"]
    for name in internal:
        dependency = data["workspace"]["dependencies"][name]["version"]
        if dependency != f"={old}":
            raise ValueError(f"{name} dependency {dependency!r} differs from workspace version {old!r}")
    lock = checkout / "Cargo.lock"
    lock_source = lock.read_text()
    packages = [p for p in tomllib.loads(lock_source)["package"] if p["name"] in set(CRATES)]
    if len(packages) != len(CRATES) or any(p["version"] != old for p in packages):
        raise ValueError("Cargo.lock workspace versions differ from Cargo.toml")
    if old == version:
        return False

    changed = replace_once(source, f'version = "{old}"', f'version = "{version}"')
    if changed.count(f'version = "={old}"') != len(internal):
        raise ValueError(f"expected {len(internal)} internal dependency versions in Cargo.toml")
    changed = changed.replace(f'version = "={old}"', f'version = "={version}"')

    matches = LOCK_PACKAGE.findall(lock_source)
    if len(matches) != len(CRATES) or any(found != old for _, found, _ in matches):
        raise ValueError(f"expected {len(CRATES)} workspace package entries in Cargo.lock")
    changed_lock = LOCK_PACKAGE.sub(lambda match: f"{match[1]}{version}{match[3]}", lock_source)
    manifest.write_text(changed)
    lock.write_text(changed_lock)
    return True


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tag", help="release tag, such as v0.5.0-rc1")
    parser.add_argument("--output", type=pathlib.Path, help="new checkout directory (default: temporary directory)")
    args = parser.parse_args()
    match = VERSION.fullmatch(args.tag)
    if not match or (match[4] and any(p.isdigit() and len(p) > 1 and p[0] == "0" for p in match[4].split("."))):
        parser.error("tag must be vX.Y.Z with an optional semver pre-release suffix")
    version = args.tag[1:]
    commit = run("git", "rev-parse", f"refs/tags/{args.tag}^{{commit}}")
    checkout = args.output.resolve() if args.output else pathlib.Path(tempfile.mkdtemp(prefix="parterre-crates-"))
    run("git", "clone", "--local", "--no-hardlinks", "--quiet", str(ROOT), str(checkout))
    run("git", "checkout", "--quiet", "--detach", commit, cwd=checkout)
    if update_versions(checkout, version):
        run("git", "add", "Cargo.toml", "Cargo.lock", cwd=checkout)
        run("git", "-c", "user.name=Parterre release preparation", "-c", "user.email=release@localhost", "commit", "--quiet", "-m", f"Prepare crates.io packages {version}", cwd=checkout)
    run("cargo", "metadata", "--locked", "--no-deps", "--format-version", "1", cwd=checkout)
    print(checkout)
    print(f"Prepared {version} from tag {args.tag} ({commit[:7]}).")
    print(f"Run: cd {shlex.quote(str(checkout))} && cargo publish --workspace")


if __name__ == "__main__":
    main()
