//! The signed-in user's repositories on github.com, to clone one (#358): those GitHub's own
//! pickers list, the user's, their organisations' and those they collaborate on, most
//! recently pushed first. Their clone URL is HTTPS or SSH as `gh`'s `git_protocol` says, as
//! `gh repo clone` does.

use serde::Deserialize;

use super::{Answer, Api, ForgeError, GithubRepo, Quota, WEB};

/// Repositories asked for per request, GitHub's most.
const PER_PAGE: usize = 100;
/// Requests at most: the 1,000 most recently pushed are plenty to pick from.
const PAGES: usize = 10;

/// A repository the user can clone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    /// `owner/name`.
    pub name: String,
    pub description: String,
    pub private: bool,
    pub fork: bool,
    pub archived: bool,
}

/// How a repository is cloned: `gh`'s `git_protocol`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Protocol {
    #[default]
    Https,
    Ssh,
}

/// The user's repositories, and how they clone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Repositories {
    pub list: Vec<Listed>,
    pub protocol: Protocol,
}

impl Repositories {
    /// `listed`'s clone URL, as `gh repo clone` makes it.
    pub fn url(&self, listed: &Listed) -> String {
        match self.protocol {
            Protocol::Https => format!("{WEB}{}.git", listed.name),
            Protocol::Ssh => format!("git@github.com:{}.git", listed.name),
        }
    }

    /// Those whose name or description has every word of `query`, in any case; all for none.
    pub fn matching(&self, query: &str) -> Vec<&Listed> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        self.list
            .iter()
            .filter(|l| {
                let name = l.name.to_lowercase();
                let description = l.description.to_lowercase();
                words
                    .iter()
                    .all(|w| name.contains(w.as_str()) || description.contains(w.as_str()))
            })
            .collect()
    }

    /// The one `url` clones, if it's on github.com and listed.
    pub fn find_url(&self, url: &str) -> Option<&Listed> {
        let repo = GithubRepo::from_url(url)?.full_name();
        self.list
            .iter()
            .find(|l| l.name.eq_ignore_ascii_case(&repo))
    }
}

/// Asks GitHub for the user's repositories, signed in with `gh`. Run it on a worker thread.
pub fn repositories() -> Result<Repositories, ForgeError> {
    let list = super::with_api(list)?;
    Ok(Repositories {
        list,
        protocol: protocol(),
    })
}

/// The repositories in `json` (`--github-repositories-from`) rather than GitHub's: an array of
/// `{"name": "owner/name", "description", "private", "fork", "archived"}`, cloned over HTTPS.
pub fn repositories_canned(json: &str) -> Result<Repositories, ForgeError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Canned {
        name: String,
        #[serde(default)]
        description: String,
        #[serde(default)]
        private: bool,
        #[serde(default)]
        fork: bool,
        #[serde(default)]
        archived: bool,
    }
    let canned: Vec<Canned> =
        serde_json::from_str(json).map_err(|e| ForgeError::Canned(e.to_string()))?;
    let list = canned
        .into_iter()
        .map(|c| {
            GithubRepo::from_full_name(&c.name)
                .ok_or_else(|| ForgeError::Canned(format!("{}: not owner/name", c.name)))?;
            Ok(Listed {
                name: c.name,
                description: c.description,
                private: c.private,
                fork: c.fork,
                archived: c.archived,
            })
        })
        .collect::<Result<_, ForgeError>>()?;
    Ok(Repositories {
        list,
        protocol: Protocol::Https,
    })
}

/// `gh config get git_protocol`: HTTPS unless it says SSH, also when `gh` can't be asked.
fn protocol() -> Protocol {
    match super::gh(&["config", "get", "git_protocol", "--host", "github.com"]) {
        Ok(out) if out.trim().eq_ignore_ascii_case("ssh") => Protocol::Ssh,
        _ => Protocol::Https,
    }
}

/// Every page of the list, up to [`PAGES`]; pauses asking when the budget runs low.
fn list(api: &dyn Api) -> Result<Vec<Listed>, ForgeError> {
    let mut list = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..PAGES {
        let Answer { body, quota } = api.post(&request(after.as_deref()))?;
        if let Some(until) = quota.and_then(Quota::pause_until) {
            super::pause(until);
        }
        let page = page(&body)?;
        list.extend(page.nodes.into_iter().map(|n| Listed {
            name: n.name_with_owner,
            description: n.description.unwrap_or_default(),
            private: n.is_private,
            fork: n.is_fork,
            archived: n.is_archived,
        }));
        match page.page_info {
            PageInfo {
                has_next_page: true,
                end_cursor: Some(cursor),
            } => after = Some(cursor),
            _ => break,
        }
    }
    Ok(list)
}

/// The request for the page after `after`, as JSON.
fn request(after: Option<&str>) -> String {
    let query = format!(
        "query($after:String){{viewer{{repositories(first:{PER_PAGE},after:$after,\
         affiliations:[OWNER,ORGANIZATION_MEMBER,COLLABORATOR],\
         ownerAffiliations:[OWNER,ORGANIZATION_MEMBER,COLLABORATOR],\
         orderBy:{{field:PUSHED_AT,direction:DESC}}){{\
         pageInfo{{hasNextPage endCursor}} \
         nodes{{nameWithOwner description isPrivate isFork isArchived}}}}}}}}"
    );
    serde_json::json!({ "query": query, "variables": { "after": after } }).to_string()
}

/// The page in an answer, or why there is none.
fn page(body: &str) -> Result<Page, ForgeError> {
    let answer: PageAnswer =
        serde_json::from_str(body).map_err(|e| ForgeError::Parse(e.to_string()))?;
    if let Some(page) = answer.data.and_then(|d| d.viewer).map(|v| v.repositories) {
        return Ok(page);
    }
    Err(match answer.errors.into_iter().next() {
        Some(e) if e.kind.as_deref() == Some("RATE_LIMITED") => {
            ForgeError::RateLimited { minutes: 60 }
        }
        Some(e) => ForgeError::Status {
            status: 200,
            message: e.message,
        },
        None => ForgeError::Parse("no repositories in the answer".into()),
    })
}

#[derive(Debug, Deserialize)]
struct PageAnswer {
    data: Option<Data>,
    #[serde(default)]
    errors: Vec<super::json::Error>,
}

#[derive(Debug, Deserialize)]
struct Data {
    viewer: Option<Viewer>,
}

#[derive(Debug, Deserialize)]
struct Viewer {
    repositories: Page,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    page_info: PageInfo,
    nodes: Vec<Node>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Node {
    name_with_owner: String,
    description: Option<String>,
    is_private: bool,
    is_fork: bool,
    is_archived: bool,
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// Answers each request with the next body.
    struct Pages {
        bodies: RefCell<Vec<String>>,
        asked: RefCell<Vec<serde_json::Value>>,
    }

    impl Api for Pages {
        fn post(&self, body: &str) -> Result<Answer, ForgeError> {
            self.asked
                .borrow_mut()
                .push(serde_json::from_str(body).unwrap());
            let mut bodies = self.bodies.borrow_mut();
            if bodies.is_empty() {
                return Err(ForgeError::Network("no more answers".into()));
            }
            Ok(Answer {
                body: bodies.remove(0),
                quota: None,
            })
        }
    }

    fn page(names: &[&str], next: Option<&str>) -> String {
        let nodes: Vec<String> = names
            .iter()
            .map(|n| {
                format!(
                    r#"{{"nameWithOwner":"{n}","description":null,"isPrivate":false,"isFork":false,"isArchived":{}}}"#,
                    n.ends_with("old")
                )
            })
            .collect();
        format!(
            r#"{{"data":{{"viewer":{{"repositories":{{"pageInfo":{{"hasNextPage":{},"endCursor":{}}},"nodes":[{}]}}}}}}}}"#,
            next.is_some(),
            next.map_or("null".to_owned(), |c| format!("\"{c}\"")),
            nodes.join(",")
        )
    }

    #[test]
    fn every_page_is_read_in_order() {
        let api = Pages {
            bodies: RefCell::new(vec![
                page(&["me/new", "org/shared"], Some("c1")),
                page(&["me/old"], None),
            ]),
            asked: RefCell::default(),
        };
        let list = list(&api).unwrap();
        let names: Vec<&str> = list.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["me/new", "org/shared", "me/old"]);
        assert!(list[2].archived);
        let asked = api.asked.borrow();
        assert_eq!(asked.len(), 2);
        assert_eq!(asked[0]["variables"]["after"], serde_json::Value::Null);
        assert_eq!(asked[1]["variables"]["after"], "c1");
        let query = asked[0]["query"].as_str().unwrap();
        assert!(query.contains("ORGANIZATION_MEMBER"), "{query}");
        assert!(query.contains("PUSHED_AT"), "{query}");
    }

    #[test]
    fn an_error_answer_is_said() {
        let api = Pages {
            bodies: RefCell::new(vec![
                r#"{"data":null,"errors":[{"type":"FORBIDDEN","message":"no"}]}"#.into(),
            ]),
            asked: RefCell::default(),
        };
        assert!(matches!(list(&api), Err(ForgeError::Status { .. })));
    }

    fn listed(name: &str, description: &str) -> Listed {
        Listed {
            name: name.into(),
            description: description.into(),
            private: false,
            fork: false,
            archived: false,
        }
    }

    #[test]
    fn urls_follow_ghs_protocol_and_are_found_again() {
        let mut repos = Repositories {
            list: vec![listed("aquamoth/parterre", "")],
            protocol: Protocol::Https,
        };
        let one = &repos.list[0].clone();
        assert_eq!(repos.url(one), "https://github.com/aquamoth/parterre.git");
        repos.protocol = Protocol::Ssh;
        assert_eq!(repos.url(one), "git@github.com:aquamoth/parterre.git");
        assert_eq!(
            repos.find_url("https://github.com/Aquamoth/Parterre"),
            Some(one)
        );
        assert_eq!(
            repos.find_url("https://example.com/aquamoth/parterre"),
            None
        );
    }

    #[test]
    fn every_word_must_match_the_name_or_description() {
        let repos = Repositories {
            list: vec![
                listed("aquamoth/parterre", "Revision graph viewer"),
                listed("aquamoth/worldclock", "Clocks"),
            ],
            protocol: Protocol::Https,
        };
        let names =
            |q: &str| -> Vec<String> { repos.matching(q).iter().map(|l| l.name.clone()).collect() };
        assert_eq!(names("").len(), 2);
        assert_eq!(names("PART"), ["aquamoth/parterre"]);
        assert_eq!(names("aquamoth graph"), ["aquamoth/parterre"]);
        assert!(names("graph clocks").is_empty());
    }

    #[test]
    fn a_canned_list_needs_owner_and_name() {
        let repos = repositories_canned(
            r#"[{"name":"me/a","private":true},{"name":"org/b","archived":true}]"#,
        )
        .unwrap();
        assert_eq!(repos.list.len(), 2);
        assert!(repos.list[0].private && repos.list[1].archived);
        assert!(repositories_canned(r#"[{"name":"nope"}]"#).is_err());
        assert!(repositories_canned(r#"[{"name":"a/b","stars":3}]"#).is_err());
    }
}
