//! Fluent FTL localization parser and dictionary lookup for French (`fr`) and English (`en`).

use std::collections::HashMap;

const FTL_FR: &str = include_str!("../../../../locales/fr/main.ftl");
const FTL_EN: &str = include_str!("../../../../locales/en/main.ftl");

/// Simple Fluent `.ftl` catalog supporting runtime language switching.
#[derive(Debug, Clone)]
pub struct I18nCatalog {
    lang: String,
    messages: HashMap<String, String>,
}

impl I18nCatalog {
    /// Loads the Fluent catalog for `"fr"` or `"en"`.
    pub fn new(lang: &str) -> Self {
        let raw = match lang {
            "en" => FTL_EN,
            _ => FTL_FR,
        };
        let mut messages = HashMap::new();
        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some((key, val)) = trimmed.split_once('=') {
                messages.insert(key.trim().to_string(), val.trim().to_string());
            }
        }
        Self {
            lang: lang.to_string(),
            messages,
        }
    }

    pub fn lang(&self) -> &str {
        &self.lang
    }

    /// Translates `key`, returning the key itself if not found.
    pub fn t<'a>(&'a self, key: &'a str) -> &'a str {
        self.messages.get(key).map(String::as_str).unwrap_or(key)
    }
}
