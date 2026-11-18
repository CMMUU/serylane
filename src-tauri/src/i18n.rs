use fluent_bundle::{concurrent::FluentBundle, FluentArgs, FluentResource};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};
use unic_langid::LanguageIdentifier;

const RESOURCES: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/locale_resources.rs"));
const DEFAULT: &str = "en";
const LEGACY: &str = "zh-Hans";

#[derive(Deserialize)]
struct Language {
    id: &'static str,
    format: &'static str,
    #[cfg(target_os = "macos")]
    direction: &'static str,
}

static LANGUAGES: LazyLock<Vec<Language>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../locales/manifest.json")).expect("language registry")
});

type Catalog = FluentBundle<FluentResource>;
static CATALOGS: LazyLock<RwLock<HashMap<&'static str, Catalog>>> = LazyLock::new(Default::default);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Locale {
    pub preference: &'static str,
    pub language: &'static str,
    pub format_locale: String,
}

static ACTIVE: LazyLock<RwLock<Locale>> = LazyLock::new(|| RwLock::new(resolve(LEGACY, [])));

fn language(tag: &str) -> Option<&'static Language> {
    LANGUAGES.iter().find(|language| language.id == tag)
}

fn match_requested(tag: &str) -> Option<(&'static Language, String)> {
    let parsed = tag.replace('_', "-").parse::<LanguageIdentifier>().ok()?;
    let base = parsed.language.as_str();
    let id = match base {
        "zh" => match parsed.script.as_ref().map(|script| script.as_str()) {
            Some("Hant") => "zh-Hant",
            Some("Hans") => LEGACY,
            Some(_) => return None,
            None if parsed
                .region
                .as_ref()
                .is_some_and(|region| matches!(region.as_str(), "TW" | "HK" | "MO")) =>
            {
                "zh-Hant"
            }
            None => LEGACY,
        },
        "pt" => "pt-BR",
        _ => base,
    };
    let found = language(id)?;
    let format = if parsed.region.is_some() {
        parsed.to_string()
    } else {
        found.format.to_owned()
    };
    Some((found, format))
}

fn resolve<'a>(preference: &str, system: impl IntoIterator<Item = &'a str>) -> Locale {
    let detected = if preference == "system" {
        system.into_iter().find_map(match_requested)
    } else {
        match_requested(preference)
    };
    let fallback = if preference == "system" {
        DEFAULT
    } else {
        LEGACY
    };
    let (found, format_locale) = detected.unwrap_or_else(|| {
        let fallback = language(fallback).expect("default application locale");
        (fallback, fallback.format.to_owned())
    });
    Locale {
        preference: if preference == "system" {
            "system"
        } else {
            found.id
        },
        language: found.id,
        format_locale,
    }
}

pub(crate) fn resolve_preference(preference: &str) -> Locale {
    if preference == "system" {
        let system = sys_locale::get_locales().collect::<Vec<_>>();
        resolve(preference, system.iter().map(String::as_str))
    } else {
        resolve(preference, [])
    }
}

pub(crate) fn valid_preference(preference: &str) -> bool {
    preference == "system" || language(preference).is_some()
}

pub(crate) fn active() -> Locale {
    ACTIVE.read().expect("locale state").clone()
}

pub(crate) fn activate(locale: Locale) -> Locale {
    *ACTIVE.write().expect("locale state") = locale.clone();
    locale
}

#[cfg(target_os = "macos")]
pub(crate) fn right_to_left() -> bool {
    language(active().language).is_some_and(|language| language.direction == "rtl")
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn catalog(tag: &'static str) -> Option<Catalog> {
    let source = RESOURCES.iter().find(|(id, _)| *id == tag)?.1;
    let resource = FluentResource::try_new(source.to_owned()).ok()?;
    let mut bundle = Catalog::new_concurrent(vec![tag.parse().ok()?]);
    bundle.add_resource(resource).ok()?;
    Some(bundle)
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn message(id: &str, args: Option<&FluentArgs<'_>>) -> Option<String> {
    let selected = active().language;
    let tags = [selected, DEFAULT, LEGACY];
    let mut catalogs = CATALOGS.write().expect("locale catalogs");
    for tag in tags {
        if !catalogs.contains_key(tag) {
            if let Some(catalog) = catalog(tag) {
                catalogs.insert(tag, catalog);
            }
        }
        let Some(catalog) = catalogs.get(tag) else {
            continue;
        };
        let Some(pattern) = catalog.get_message(id).and_then(|message| message.value()) else {
            continue;
        };
        let mut errors = Vec::new();
        let value = catalog.format_pattern(pattern, args, &mut errors);
        if errors.is_empty() {
            return Some(value.into_owned());
        }
    }
    None
}

/// Compatibility for existing, application-owned text at native presentation boundaries.
pub(crate) fn text(source: &str) -> String {
    if source.trim().is_empty() || active().language == LEGACY {
        return source.to_owned();
    }
    let digest = format!("{:x}", Sha256::digest(source.trim().as_bytes()));
    message(&format!("ui-{}", &digest[..14]), None).unwrap_or_else(|| source.to_owned())
}

#[cfg(test)]
mod tests {
    #[test]
    fn registry_and_bundled_catalogs_agree() {
        use super::{catalog, LANGUAGES, RESOURCES};

        assert_eq!(LANGUAGES.len(), 11);
        assert_eq!(RESOURCES.len(), LANGUAGES.len());
        for language in &*LANGUAGES {
            assert!(catalog(language.id).is_some(), "{}", language.id);
        }
    }

    #[test]
    fn legacy_explicit_and_system_language_match() {
        use super::{resolve, valid_preference};

        assert_eq!(resolve("zh-CN", ["ja"]).language, "zh-Hans");
        assert_eq!(resolve("zh-HK", []).language, "zh-Hant");
        assert_eq!(resolve("zh-Hant-CN", []).language, "zh-Hant");
        assert_eq!(resolve("system", ["sv-SE", "en-GB"]).format_locale, "en-GB");
        assert_eq!(resolve("system", ["sv-SE"]).language, "en");
        assert_eq!(resolve("ar", []).language, "ar");
        assert!(!valid_preference("../ar"));
    }
}
