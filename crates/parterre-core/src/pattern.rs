use std::str::Chars;

use crate::repo::RefKind;

/// Wildcard patterns for branch names, written separated by commas or spaces:
/// `pipeline/*, release/*`. Only branches match; tags, stash and other refs never do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BranchPatterns {
    patterns: Vec<String>,
}

impl BranchPatterns {
    pub fn parse(list: &str) -> BranchPatterns {
        BranchPatterns {
            patterns: list
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|p| !p.is_empty())
                .map(str::to_owned)
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// `name` is the display name. A remote-tracking branch matches with or without its
    /// remote: `origin/release/1.0` matches both `release/*` and `origin/release/*`.
    pub fn matches(&self, kind: RefKind, name: &str) -> bool {
        let without_remote = match kind {
            RefKind::LocalBranch => None,
            RefKind::RemoteBranch => name.split_once('/').map(|(_, branch)| branch),
            _ => return false,
        };
        self.patterns.iter().any(|p| {
            wildcard_match(p, name) || without_remote.is_some_and(|b| wildcard_match(p, b))
        })
    }
}

/// Against all of `text`. `*` matches any run of characters, slashes included, and `?` any one
/// character. ASCII letters match regardless of case.
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let (mut p, mut t) = (pattern.chars(), text.chars());
    // The rest of the pattern after the latest `*`, and the text from where it would continue
    // if the `*` took one more character.
    let mut star: Option<(Chars, Chars)> = None;
    loop {
        let t_here = t.clone();
        match (p.next(), t.next()) {
            (Some('*'), _) => {
                t = t_here;
                star = Some((p.clone(), t.clone()));
            }
            (Some(pc), Some(tc)) if pc == '?' || pc.eq_ignore_ascii_case(&tc) => {}
            (None, None) => return true,
            _ => {
                let Some((after_star, resume)) = &mut star else {
                    return false;
                };
                if resume.next().is_none() {
                    return false;
                }
                p = after_star.clone();
                t = resume.clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards() {
        assert!(wildcard_match("release/*", "release/1.0"));
        assert!(wildcard_match("release/*", "release/1.0/hotfix"));
        assert!(wildcard_match("release/*", "release/"));
        assert!(!wildcard_match("release/*", "release"));
        assert!(!wildcard_match("release/*", "prerelease/1.0"));
        assert!(wildcard_match("*release*", "prerelease/1.0"));
        assert!(wildcard_match("v?.?", "v1.2"));
        assert!(!wildcard_match("v?.?", "v1.23"));
        assert!(wildcard_match("a*b*c", "axxbyybzc"));
        assert!(!wildcard_match("a*b*c", "axxbyybz"));
        assert!(wildcard_match("**", ""));
        assert!(wildcard_match("", ""));
        assert!(!wildcard_match("", "x"));
        assert!(wildcard_match("main", "main"));
        assert!(!wildcard_match("main", "main2"));
        assert!(wildcard_match("Feature/*", "feature/X"));
        assert!(wildcard_match("f?r", "för"), "? takes a whole character");
        assert!(!wildcard_match("f??r", "för"));
    }

    #[test]
    fn lists_split_on_commas_and_spaces() {
        let p = BranchPatterns::parse(" pipeline/*,release/*  dev ,, ");
        assert_eq!(p.patterns, ["pipeline/*", "release/*", "dev"]);
        assert!(BranchPatterns::parse(" , ").is_empty());
    }

    #[test]
    fn remote_branches_match_with_or_without_the_remote() {
        let p = BranchPatterns::parse("release/*");
        assert!(p.matches(RefKind::LocalBranch, "release/1.0"));
        assert!(p.matches(RefKind::RemoteBranch, "origin/release/1.0"));
        assert!(!p.matches(RefKind::LocalBranch, "origin/release/1.0"));
        let p = BranchPatterns::parse("origin/release/*");
        assert!(p.matches(RefKind::RemoteBranch, "origin/release/1.0"));
        assert!(!p.matches(RefKind::RemoteBranch, "upstream/release/1.0"));
    }

    #[test]
    fn only_branches_match() {
        let p = BranchPatterns::parse("*");
        assert!(p.matches(RefKind::LocalBranch, "main"));
        for kind in [
            RefKind::Tag,
            RefKind::Stash,
            RefKind::DetachedHead,
            RefKind::Other,
        ] {
            assert!(!p.matches(kind, "main"), "{kind:?}");
        }
    }
}
