//! Shared, deterministic name evidence. Never infers an IP location or a bill.
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, sync::LazyLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegionStatus {
    Supported,
    Unsupported,
    Restricted,
    Unknown,
    Ambiguous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RateSource {
    Name,
    Manual,
    Unknown,
    Conflict,
    Invalid,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegionRules {
    version: String,
    source: String,
    checked_at: String,
    regions: Vec<Region>,
}
#[derive(Debug, Deserialize)]
struct Region {
    code: String,
    status: RegionStatus,
    aliases: Vec<String>,
}
static RULES: LazyLock<RegionRules> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../data/openai-regions.json")).expect("bundled region rules")
});
static ALIASES: LazyLock<Vec<(String, String)>> = LazyLock::new(|| {
    RULES
        .regions
        .iter()
        .flat_map(|r| r.aliases.iter().map(|a| (normalize(a), r.code.clone())))
        .collect()
});

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeMetadata {
    pub regions: Vec<String>,
    pub region_status: RegionStatus,
    pub region_reason: String,
    pub evidence: Vec<String>,
    pub eligible: bool,
    pub rule_version: String,
    pub rule_source: String,
    pub checked_at: String,
    pub name_multiplier: Option<f64>,
    pub multiplier_source: RateSource,
}

fn normalize(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xfee0).unwrap(),
            '\u{3000}' => ' ',
            _ => c,
        })
        .collect::<String>()
        .to_lowercase()
}

// Latin names use letter boundaries (JP01 is a common explicit node prefix).
// Longest overlapping names win: Papua New Guinea is not also Guinea.
fn region_evidence(name: &str) -> (Vec<String>, Vec<String>) {
    let text = normalize(name);
    let mut matches = Vec::new();
    for (alias, code) in ALIASES.iter() {
        for (start, _) in text.match_indices(alias) {
            let end = start + alias.len();
            let latin = alias.is_ascii();
            if latin
                && (text[..start]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_alphabetic())
                    || text[end..]
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphabetic()))
            {
                continue;
            }
            matches.push((start, end, code.clone(), alias.clone()));
        }
    }
    matches.sort_by_key(|(a, b, _, _)| std::cmp::Reverse(b - a));
    let mut spans: Vec<(usize, usize, String)> = Vec::new();
    let mut codes = BTreeSet::new();
    let mut evidence = BTreeSet::new();
    for (start, end, code, token) in matches {
        if spans
            .iter()
            .any(|(a, b, c)| start >= *a && end <= *b && (start != *a || end != *b || c == &code))
        {
            continue;
        }
        spans.push((start, end, code.clone()));
        codes.insert(code);
        evidence.insert(token);
    }
    // Pair consecutive regional indicators, rather than sliding windows across
    // adjacent flags (US+JP must not accidentally create SJ).
    let mut pending = None;
    for c in text.chars() {
        if ('\u{1F1E6}'..='\u{1F1FF}').contains(&c) {
            let letter = char::from_u32(c as u32 - 0x1f1e6 + 65).unwrap();
            if let Some(first) = pending.take() {
                let code = format!("{first}{letter}");
                evidence.insert(code.clone());
                codes.insert(code);
            } else {
                pending = Some(letter);
            }
        } else {
            pending = None;
        }
    }
    (codes.into_iter().collect(), evidence.into_iter().collect())
}

pub fn eligible(name: &str) -> bool {
    let (regions, _) = region_evidence(name);
    regions.len() == 1
        && RULES
            .regions
            .iter()
            .any(|r| r.code == regions[0] && r.status == RegionStatus::Supported)
}

fn name_rate(name: &str) -> (Option<f64>, RateSource) {
    static PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
        [
            r"(?P<n>[+-]?[0-9]+(?:\.[0-9]+)?)\s*[x×倍]",
            r"[x×]\s*(?P<n>[+-]?[0-9]+(?:\.[0-9]+)?)",
            r"倍率\s*[:=]?\s*(?P<n>[+-]?[0-9]+(?:\.[0-9]+)?)",
        ]
        .iter()
        .map(|p| Regex::new(p).unwrap())
        .collect()
    });
    let text = normalize(name);
    let mut rates = Vec::new();
    let mut invalid = false;
    for regex in PATTERNS.iter() {
        for captures in regex.captures_iter(&text) {
            let whole = captures.get(0).unwrap();
            let boundary = |c: char| c.is_ascii_alphanumeric() || c == '.';
            if text[..whole.start()]
                .chars()
                .next_back()
                .is_some_and(boundary)
                || text[whole.end()..].chars().next().is_some_and(boundary)
            {
                continue;
            }
            let rate = captures["n"].parse::<f64>().unwrap_or(f64::NAN);
            if !rate.is_finite() || !(0.01..=1000.0).contains(&rate) {
                invalid = true;
            } else if !rates.contains(&rate) {
                rates.push(rate);
            }
        }
    }
    if invalid {
        (None, RateSource::Invalid)
    } else {
        match rates.as_slice() {
            [rate] => (Some(*rate), RateSource::Name),
            [] => (None, RateSource::Unknown),
            _ => (None, RateSource::Conflict),
        }
    }
}

pub fn parse(name: &str) -> NodeMetadata {
    let (regions, evidence) = region_evidence(name);
    let region_status = match regions.as_slice() {
        [] => RegionStatus::Unknown,
        [code] => RULES
            .regions
            .iter()
            .find(|r| &r.code == code)
            .map_or(RegionStatus::Unknown, |r| r.status),
        _ => RegionStatus::Ambiguous,
    };
    let reason = match region_status {
        RegionStatus::Supported => "名称地区在 API 支持名单内",
        RegionStatus::Unsupported => "地区不在 API 支持名单内",
        RegionStatus::Restricted => "地区有子区域限制，待确认",
        RegionStatus::Unknown => "地区未知，待确认",
        RegionStatus::Ambiguous => "多地区冲突，待确认出口",
    };
    let (name_multiplier, multiplier_source) = name_rate(name);
    NodeMetadata {
        regions,
        region_status,
        evidence,
        region_reason: reason.into(),
        eligible: region_status == RegionStatus::Supported,
        rule_version: RULES.version.clone(),
        rule_source: RULES.source.clone(),
        checked_at: RULES.checked_at.clone(),
        name_multiplier,
        multiplier_source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn regions_are_explicit_and_conflicts_stay_closed() {
        for name in [
            "🇯🇵 日本 01 · 0.5x",
            "US-02 [倍率:2]",
            "Tokyo 03",
            "SG01",
            "ＴＷ－０１",
            "Papua New Guinea 01",
        ] {
            assert!(eligible(name), "{name}");
        }
        for name in [
            "香港 0.1x",
            "CN-01",
            "RU01",
            "RUS",
            "AUS",
            "inside",
            "🇺🇦 UA",
            "专线01",
            "HK → JP",
            "🇭🇰 日本",
            "🇺🇸🇯🇵",
        ] {
            assert!(!eligible(name), "{name}");
        }
        assert_eq!(parse("🇺🇸🇯🇵").regions, vec!["JP", "US"]);
        assert_eq!(parse("香港").region_status, RegionStatus::Unsupported);
        assert_eq!(parse("Ukraine").region_status, RegionStatus::Restricted);
    }
    #[test]
    fn parses_only_explicit_rates_and_keeps_conflicts() {
        for name in [
            "JP 0.5x",
            "JP 0.5X",
            "JP 0.5×",
            "JP ×0.5",
            "JP x0.5",
            "JP 倍率:0.5",
            "JP 倍率 0.5",
            "JP 0.5倍",
            "JP 【０．５Ｘ】",
            "JP 0.5x 0.5倍",
        ] {
            assert_eq!(name_rate(name), (Some(0.5), RateSource::Name), "{name}");
        }
        for name in [
            "JP-01 2Gbps 剩余100GB",
            "JP 2026-09-20",
            "JP 1e3x",
            "JP v2x",
            "JP x2large",
        ] {
            assert_eq!(name_rate(name).0, None, "{name}");
        }
        assert_eq!(name_rate("JP 0.5x 2倍").1, RateSource::Conflict);
        for name in ["JP 0x", "JP -1x", "JP 1001x", "JP 0.001x"] {
            assert_eq!(name_rate(name).1, RateSource::Invalid, "{name}");
        }
    }
    #[test]
    fn every_bundled_supported_country_matches_its_code_and_flag() {
        let mut seen = BTreeSet::new();
        for r in &RULES.regions {
            assert!(seen.insert(&r.code), "duplicate {}", r.code);
            let flag: String = r
                .code
                .chars()
                .map(|c| char::from_u32(c as u32 - 65 + 0x1f1e6).unwrap())
                .collect();
            assert_eq!(eligible(&flag), r.status == RegionStatus::Supported);
            assert_eq!(
                eligible(&format!("{}-01", r.code)),
                r.status == RegionStatus::Supported,
                "{}",
                r.code
            );
        }
    }
}
