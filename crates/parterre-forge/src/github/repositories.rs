//! The signed-in user's repositories, to clone one (#358): those GitHub's own pickers list, the
//! user's, their organisations' and those they collaborate on, most recently pushed first.

use serde::Deserialize;

use super::{Api, Forge, ForgeError, GithubRepo, Quota, Response, WEB, json};
use crate::repositories::{Listed, Repositories};

/// GitHub's maximum.
const REPOSITORIES_PER_PAGE: usize = 100;
/// The 1,000 most recently pushed are plenty to pick from.
const MAX_PAGES: usize = 10;

/// `gh`'s `git_protocol`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Protocol {
    #[default]
    Https,
    Ssh,
}

impl Protocol {
    /// The clone URL of `owner/name`, as `gh repo clone` makes it.
    fn url(self, name: &str) -> String {
        match self {
            Protocol::Https => format!("{WEB}{name}.git"),
            Protocol::Ssh => format!("git@github.com:{name}.git"),
        }
    }
}

/// Blocks: run it on a worker thread.
pub fn repositories() -> Result<Repositories, ForgeError> {
    let mut list = super::with_api(list)?;
    let protocol = protocol();
    for listed in &mut list {
        listed.url = protocol.url(&listed.name);
    }
    Ok(Repositories { list })
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
                forge: Forge::Github,
                url: Protocol::Https.url(&c.name),
                name: c.name,
                description: c.description,
                private: c.private,
                fork: c.fork,
                archived: c.archived,
            })
        })
        .collect::<Result<_, ForgeError>>()?;
    Ok(Repositories { list })
}

/// HTTPS unless `gh` says SSH.
fn protocol() -> Protocol {
    match super::gh(&["config", "get", "git_protocol", "--host", "github.com"]) {
        Ok(out) if out.trim().eq_ignore_ascii_case("ssh") => Protocol::Ssh,
        _ => Protocol::Https,
    }
}

/// With no clone URLs yet.
fn list(api: &dyn Api) -> Result<Vec<Listed>, ForgeError> {
    let mut list = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let Response { body, quota } = api.post(&request(after.as_deref()))?;
        if let Some(until) = quota.and_then(Quota::pause_until) {
            super::pause(until);
        }
        let page = page(&body)?;
        list.extend(page.nodes.into_iter().map(|n| Listed {
            forge: Forge::Github,
            url: String::new(),
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

fn request(after: Option<&str>) -> String {
    let query = format!(
        "query($after:String){{viewer{{repositories(first:{REPOSITORIES_PER_PAGE},after:$after,\
         affiliations:[OWNER,ORGANIZATION_MEMBER,COLLABORATOR],\
         ownerAffiliations:[OWNER,ORGANIZATION_MEMBER,COLLABORATOR],\
         orderBy:{{field:PUSHED_AT,direction:DESC}}){{\
         pageInfo{{hasNextPage endCursor}} \
         nodes{{nameWithOwner description isPrivate isFork isArchived}}}}}}}}"
    );
    serde_json::json!({ "query": query, "variables": { "after": after } }).to_string()
}

fn page(body: &str) -> Result<Page, ForgeError> {
    let answer: json::Answer<Data> = crate::parse_answer(Forge::Github, body)?;
    if let Some(page) = answer.data.and_then(|d| d.viewer).map(|v| v.repositories) {
        return Ok(page);
    }
    Err(match answer.errors.into_iter().next() {
        Some(e) => e.into_forge_error(),
        None => ForgeError::Parse(Forge::Github, "no repositories in the answer".into()),
    })
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
    use super::super::tests::{fake, requests};
    use super::*;

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
        let api = fake(
            &[
                &page(&["me/new", "org/shared"], Some("c1")),
                &page(&["me/old"], None),
            ],
            None,
        );
        let list = list(&api).unwrap();
        let names: Vec<&str> = list.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["me/new", "org/shared", "me/old"]);
        assert!(list[2].archived);
        let asked = requests(&api);
        assert_eq!(asked.len(), 2);
        assert_eq!(asked[0]["variables"]["after"], serde_json::Value::Null);
        assert_eq!(asked[1]["variables"]["after"], "c1");
        let query = asked[0]["query"].as_str().unwrap();
        assert!(query.contains("ORGANIZATION_MEMBER"), "{query}");
        assert!(query.contains("PUSHED_AT"), "{query}");
    }

    #[test]
    fn an_error_answer_is_said() {
        let api = fake(
            &[r#"{"data":null,"errors":[{"type":"FORBIDDEN","message":"no"}]}"#],
            None,
        );
        assert!(matches!(list(&api), Err(ForgeError::Status { .. })));
    }

    #[test]
    fn urls_follow_ghs_protocol() {
        assert_eq!(
            Protocol::Https.url("aquamoth/parterre"),
            "https://github.com/aquamoth/parterre.git"
        );
        assert_eq!(
            Protocol::Ssh.url("aquamoth/parterre"),
            "git@github.com:aquamoth/parterre.git"
        );
    }

    #[test]
    fn a_canned_list_needs_owner_and_name() {
        let repos = repositories_canned(
            r#"[{"name":"me/a","private":true},{"name":"org/b","archived":true}]"#,
        )
        .unwrap();
        assert_eq!(repos.list.len(), 2);
        assert!(repos.list[0].private && repos.list[1].archived);
        assert_eq!(repos.list[0].url, "https://github.com/me/a.git");
        assert!(repositories_canned(r#"[{"name":"nope"}]"#).is_err());
        assert!(repositories_canned(r#"[{"name":"a/b","stars":3}]"#).is_err());
    }
}
