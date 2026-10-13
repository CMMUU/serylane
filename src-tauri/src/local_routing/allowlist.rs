//! Restricts this relay only. A rule never changes its upstream or credentials.
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use url::Url;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Rule {
    pub url: String,
    pub enabled: bool,
}

pub fn defaults() -> Vec<Rule> {
    [
        "https://chatgpt.com/backend-api/codex",
        "https://api.openai.com/v1",
    ]
    .into_iter()
    .map(|url| Rule {
        url: url.into(),
        enabled: true,
    })
    .collect()
}

pub fn normalize(rules: &[Rule]) -> AppResult<Vec<Rule>> {
    if rules.len() > 100 {
        return Err(invalid());
    }
    let mut seen = BTreeSet::new();
    rules
        .iter()
        .map(|rule| {
            let raw = rule.url.trim();
            // Reject ambiguous path spellings before URL normalization removes them.
            if raw.len() > 2048
                || raw.contains(['\\', '%', '?', '#'])
                || raw.chars().any(char::is_whitespace)
                || raw.split('/').any(|part| matches!(part, "." | ".."))
            {
                return Err(invalid());
            }
            let mut url = Url::parse(raw).map_err(|_| invalid())?;
            if url.scheme() != "https"
                || !matches!(url.host(), Some(url::Host::Domain(_)))
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(invalid());
            }
            let path = url.path().trim_end_matches('/').to_owned();
            url.set_path(&path);
            let normalized = url.as_str().trim_end_matches('/').to_owned();
            if !seen.insert(normalized.clone()) {
                return Err(AppError::InvalidInput(
                    "路由名单存在重复 URL，请合并后保存".into(),
                ));
            }
            Ok(Rule {
                url: normalized,
                enabled: rule.enabled,
            })
        })
        .collect()
}

fn invalid() -> AppError {
    AppError::InvalidInput("路由名单最多 100 条 HTTPS URL；请使用域名和普通路径，不含账号、查询参数、片段、编码路径或相对路径".into())
}

pub fn permits(rules: &[Rule], target: &Url) -> bool {
    rules.iter().filter(|r| r.enabled).any(|rule| {
        let Ok(base) = Url::parse(&rule.url) else {
            return false;
        };
        let path = base.path().trim_end_matches('/');
        target.origin() == base.origin()
            && (path.is_empty()
                || target.path() == path
                || target
                    .path()
                    .strip_prefix(path)
                    .is_some_and(|tail| tail.starts_with('/')))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_origin_and_segment_boundaries_not_substrings() {
        let rules = normalize(&defaults()).unwrap();
        for target in [
            "https://api.openai.com/v1",
            "https://api.openai.com/v1/responses?stream=true",
            "https://chatgpt.com/backend-api/codex/responses",
        ] {
            assert!(permits(&rules, &Url::parse(target).unwrap()));
        }
        for target in [
            "http://api.openai.com/v1",
            "https://api.openai.com:8443/v1",
            "https://api.openai.com.evil.test/v1",
            "https://api.openai.com/v10",
            "https://chatgpt.com/backend-api/other",
        ] {
            assert!(!permits(&rules, &Url::parse(target).unwrap()));
        }
    }
    #[test]
    fn empty_disabled_and_custom_rules_do_not_expand_the_official_route() {
        let target = Url::parse("https://api.openai.com/v1/responses").unwrap();
        assert!(!permits(&[], &target));
        let mut rules = defaults();
        rules.iter_mut().for_each(|r| r.enabled = false);
        assert!(!permits(&rules, &target));
        assert!(!permits(
            &[Rule {
                url: "https://example.com/v1".into(),
                enabled: true
            }],
            &target
        ));
    }
    #[test]
    fn normalize_deduplicates_and_rejects_ambiguous_or_secret_urls() {
        let row = |url: &str| Rule {
            url: url.into(),
            enabled: true,
        };
        assert_eq!(
            normalize(&[row(" HTTPS://API.OPENAI.COM:443/v1/ ")]).unwrap()[0].url,
            "https://api.openai.com/v1"
        );
        assert!(normalize(&[
            row("https://api.openai.com/v1"),
            row("https://api.openai.com:443/v1/")
        ])
        .is_err());
        for url in [
            "http://example.com",
            "https://127.0.0.1/v1",
            "https://user:secret@example.com",
            "https://example.com/v1?key=secret",
            "https://example.com/#fragment",
            "https://example.com/v1/../admin",
            "https://example.com/%2e%2e",
            "https://example.com/a\\b",
        ] {
            assert!(normalize(&[row(url)]).is_err(), "{url}");
        }
        assert!(normalize(&vec![row("https://example.com"); 101]).is_err());
    }
}
