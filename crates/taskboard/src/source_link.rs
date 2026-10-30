//! Works out which site a task's source link points at and a compact label
//! for it, for the pill on its card.

/// A site the card shows a logo for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Site {
    GitHub,
    GitLab,
    Bitbucket,
    Jira,
    Other,
}

/// Labels longer than this are cut short with an ellipsis.
const MAX_LABEL_CHARS: usize = 28;

/// Splits a URL into its lowercased host (no `www.`, port or credentials) and
/// its path segments, ignoring any query or fragment.
fn split(url: &str) -> (String, Vec<&str>) {
    let rest = url.trim();
    let rest = rest.split_once("://").map_or(rest, |(_, r)| r);
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = authority.rsplit('@').next().unwrap_or_default();
    let host = host.split(':').next().unwrap_or_default().to_lowercase();
    let host = host
        .strip_prefix("www.")
        .map(str::to_string)
        .unwrap_or(host);
    (host, path.split('/').filter(|s| !s.is_empty()).collect())
}

/// Recognizes a site by its host, or for self-hosted GitLab by the `/-/` its
/// issue and merge request paths go through.
pub fn site(url: &str) -> Site {
    let (host, path) = split(url);
    if host.contains("github") {
        Site::GitHub
    } else if host.contains("gitlab") || path.contains(&"-") {
        Site::GitLab
    } else if host.contains("bitbucket") {
        Site::Bitbucket
    } else if host.contains("jira")
        || (host.ends_with(".atlassian.net") && path.first() == Some(&"browse"))
    {
        Site::Jira
    } else {
        Site::Other
    }
}

/// A compact label for a link: `owner/repo#42` for an issue, `owner/repo!7`
/// for a GitLab merge request, `KEY-12` for a Jira issue, otherwise the host
/// and the end of the path.
pub fn short_label(url: &str) -> String {
    let (host, path) = split(url);
    let label = match site(url) {
        Site::GitHub | Site::Bitbucket if path.len() >= 2 => {
            Some(repo_label(&path[..2], &path[2..]))
        }
        Site::GitLab => {
            // Groups nest, so the project path runs up to the `-`; keep its
            // last two parts.
            let dash = path.iter().position(|s| *s == "-").unwrap_or(path.len());
            let project = &path[dash.saturating_sub(2)..dash];
            (!project.is_empty())
                .then(|| repo_label(project, path.get(dash + 1..).unwrap_or_default()))
        }
        Site::Jira => path.last().map(|s| s.to_string()),
        _ => None,
    };
    let label = label.unwrap_or_else(|| match path.last() {
        Some(last) if path.len() > 1 => format!("{host}/…/{last}"),
        Some(last) => format!("{host}/{last}"),
        None => host,
    });
    truncate(&label, MAX_LABEL_CHARS)
}

/// `owner/repo`, plus `#N` or `!N` when `rest`, the path after the repo, is
/// an issue or a pull or merge request.
fn repo_label(repo: &[&str], rest: &[&str]) -> String {
    let repo = repo.join("/");
    match rest {
        [kind, n, ..] if n.chars().all(|c| c.is_ascii_digit()) => match *kind {
            "issues" | "pull" | "pull-requests" => format!("{repo}#{n}"),
            "merge_requests" => format!("{repo}!{n}"),
            _ => repo,
        },
        _ => repo,
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max - 1).collect();
        format!("{head}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_sites() {
        assert_eq!(
            site("https://github.com/rust-lang/rust/issues/1"),
            Site::GitHub
        );
        assert_eq!(site("https://gitlab.com/group/proj"), Site::GitLab);
        // Self-hosted GitLab is only recognizable by its `/-/` routes.
        assert_eq!(
            site("https://git.bmhenry.com/brandon/taskboard/-/issues/3"),
            Site::GitLab
        );
        assert_eq!(site("https://acme.atlassian.net/browse/OPS-12"), Site::Jira);
        assert_eq!(site("https://example.com/a/b"), Site::Other);
    }

    #[test]
    fn labels_issues_and_merge_requests() {
        assert_eq!(
            short_label("https://github.com/rust-lang/rust/pull/42#discussion"),
            "rust-lang/rust#42"
        );
        assert_eq!(
            short_label("https://gitlab.com/org/sub/proj/-/merge_requests/17?tab=diffs"),
            "sub/proj!17"
        );
        assert_eq!(
            short_label("https://acme.atlassian.net/browse/OPS-12"),
            "OPS-12"
        );
        assert_eq!(
            short_label("https://www.example.com/docs/guide/intro"),
            "example.com/…/intro"
        );
    }

    #[test]
    fn long_labels_are_truncated() {
        let label = short_label("https://example.com/a-very-long-page-name-that-keeps-going");
        assert_eq!(label.chars().count(), MAX_LABEL_CHARS);
        assert!(label.ends_with('…'));
    }
}
