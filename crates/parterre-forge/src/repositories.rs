//! Repositories the user can clone, listed by the forges they are signed in to: GitHub
//! ([`crate::github::repositories`], #358) and Azure DevOps ([`crate::azure::repositories`]).

use super::{Forge, Origin};

/// A repository the user can clone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub forge: Forge,
    /// `owner/name` on GitHub, `org/project/name` on Azure DevOps.
    pub name: String,
    pub description: String,
    pub private: bool,
    pub fork: bool,
    pub archived: bool,
    /// What it is cloned from.
    pub url: String,
}

/// Repositories the user can clone, in the order the forges listed them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Repositories {
    pub list: Vec<Listed>,
}

impl Repositories {
    /// Those whose name or description has every word of `query`, in any case; all for none.
    pub fn matching(&self, query: &str) -> Vec<&Listed> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        self.list
            .iter()
            .filter(|l| {
                words.iter().all(|w| {
                    contains_lowercase(&l.name, w) || contains_lowercase(&l.description, w)
                })
            })
            .collect()
    }

    /// The one `url` clones, if it's on a forge and listed, however the URL names it.
    pub fn find_url(&self, url: &str) -> Option<&Listed> {
        let origin = Origin::from_url(url)?;
        let (forge, name) = (origin.forge(), origin.full_name());
        self.list
            .iter()
            .find(|l| l.forge == forge && l.name.eq_ignore_ascii_case(&name))
    }
}

/// Asked every frame of the whole list, so ASCII text, as names nearly always are, is compared
/// without lowercasing a copy.
fn contains_lowercase(text: &str, lowercase_word: &str) -> bool {
    if text.is_ascii() {
        text.as_bytes()
            .windows(lowercase_word.len())
            .any(|w| w.eq_ignore_ascii_case(lowercase_word.as_bytes()))
    } else {
        text.to_lowercase().contains(lowercase_word)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(forge: Forge, name: &str, description: &str) -> Listed {
        Listed {
            forge,
            name: name.into(),
            description: description.into(),
            private: false,
            fork: false,
            archived: false,
            url: String::new(),
        }
    }

    #[test]
    fn urls_are_found_again_however_they_name_the_repository() {
        let repos = Repositories {
            list: vec![
                listed(Forge::Github, "aquamoth/parterre", ""),
                listed(Forge::AzureDevops, "cosmojpn/CJP Work/Apps", ""),
            ],
        };
        let found = |url: &str| repos.find_url(url).map(|l| l.name.as_str());
        assert_eq!(
            found("https://github.com/Aquamoth/Parterre"),
            Some("aquamoth/parterre")
        );
        assert_eq!(
            found("git@ssh.dev.azure.com:v3/cosmojpn/CJP%20Work/Apps"),
            Some("cosmojpn/CJP Work/Apps")
        );
        assert_eq!(
            found("https://cosmojpn@dev.azure.com/cosmojpn/CJP%20Work/_git/apps"),
            Some("cosmojpn/CJP Work/Apps")
        );
        assert_eq!(found("https://example.com/aquamoth/parterre"), None);
        // The same name on the other forge is another repository.
        assert_eq!(found("https://dev.azure.com/aquamoth/_git/parterre"), None);
    }

    #[test]
    fn every_word_must_match_the_name_or_description() {
        let repos = Repositories {
            list: vec![
                listed(Forge::Github, "aquamoth/parterre", "Revision graph viewer"),
                listed(Forge::Github, "aquamoth/worldclock", "Clocks"),
            ],
        };
        let names =
            |q: &str| -> Vec<String> { repos.matching(q).iter().map(|l| l.name.clone()).collect() };
        assert_eq!(names("").len(), 2);
        assert_eq!(names("PART"), ["aquamoth/parterre"]);
        assert_eq!(names("aquamoth graph"), ["aquamoth/parterre"]);
        assert!(names("graph clocks").is_empty());
    }

    #[test]
    fn words_match_in_any_case_in_unicode_text_too() {
        let repos = Repositories {
            list: vec![
                listed(Forge::Github, "mira/översikt", "Rapporter för året"),
                listed(Forge::Github, "mira/checkout", "The shop's checkout"),
            ],
        };
        let names =
            |q: &str| -> Vec<String> { repos.matching(q).iter().map(|l| l.name.clone()).collect() };
        assert_eq!(names("ÖVERSIKT"), ["mira/översikt"]);
        assert_eq!(names("FÖR"), ["mira/översikt"]);
        assert_eq!(names("SHOP'S"), ["mira/checkout"]);
        assert!(names("ö checkout").is_empty());
    }
}
