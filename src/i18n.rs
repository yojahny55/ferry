// Follows the language picked under Omarchy's Setup > Language. Catalogs are flat JSON
// objects from English text to translation, one per Omarchy language code, compiled in.
// A counted string maps to {"one": ..., "other": ...} and its "%n" is filled with the count.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::RwLock;

use serde_json::Value;

pub const CATALOGS: &[(&str, &str)] = &[
    ("en", include_str!("i18n/en.json")),
    ("es", include_str!("i18n/es.json")),
];

static STRINGS: RwLock<Option<HashMap<String, Value>>> = RwLock::new(None);
static DECIMAL: RwLock<char> = RwLock::new('.');

/// One registry code on the first line; anything else, or no file, is English.
pub fn read_language(path: &Path) -> String {
    let code = fs::read_to_string(path).unwrap_or_default();
    let code = code.lines().next().unwrap_or("").trim();
    let valid = (2..=6).contains(&code.len())
        && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if valid {
        code.to_owned()
    } else {
        "en".to_owned()
    }
}

/// English first, then the base language, then the regional one (es, then es-MX),
/// so a missing string falls back toward English one step at a time.
pub fn load(language: &str, catalogs: &[(&str, &str)]) -> HashMap<String, Value> {
    let mut strings = HashMap::new();
    let base = language.split('-').next().unwrap_or(language);
    for code in ["en", base, language] {
        let Some((_, text)) = catalogs.iter().find(|(c, _)| *c == code) else {
            continue;
        };
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) else {
            continue;
        };
        for (k, v) in map {
            if v.as_str().is_some_and(|s| !s.is_empty()) || v.is_object() {
                strings.insert(k, v);
            }
        }
    }
    strings
}

pub fn set(language: &str) {
    *STRINGS.write().unwrap() = Some(load(language, CATALOGS));
    // The decimal separator comes from Omarchy's language registry, so sizes read like the desktop's.
    let registry = std::env::var("OMARCHY_PATH").unwrap_or_else(|_| "/usr/share/omarchy".into())
        + "/default/i18n/locales.json";
    let decimal = fs::read_to_string(registry)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| {
            v[language]["decimal"]
                .as_str()
                .and_then(|s| s.chars().next())
        })
        .unwrap_or('.');
    *DECIMAL.write().unwrap() = decimal;
}

pub fn decimal() -> char {
    *DECIMAL.read().unwrap()
}

pub fn tr(text: &str) -> String {
    let strings = STRINGS.read().unwrap();
    match strings.as_ref().and_then(|s| s.get(text)) {
        Some(Value::String(s)) => s.clone(),
        _ => text.to_owned(),
    }
}

// shortcut: plural forms are only "one" and "other"; add CLDR rules when shipping a language with more forms.
pub fn trn(text: &str, n: usize) -> String {
    let strings = STRINGS.read().unwrap();
    let form = match strings.as_ref().and_then(|s| s.get(text)) {
        Some(Value::Object(forms)) => {
            let pick = if n == 1 { "one" } else { "other" };
            forms
                .get(pick)
                .or(forms.get("other"))
                .and_then(Value::as_str)
                .unwrap_or(text)
                .to_owned()
        }
        Some(Value::String(s)) => s.clone(),
        _ => text.to_owned(),
    };
    form.replace("%n", &n.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_step_by_step() {
        let catalogs = [
            (
                "en",
                r#"{"%n files": {"one": "%n file", "other": "%n files"}}"#,
            ),
            ("pt", r#"{"Save": "Guardar", "Open": "Abrir"}"#),
            ("pt-BR", r#"{"Save": "Salvar", "Quit": ""}"#),
        ];
        let s = load("pt-BR", &catalogs);
        assert_eq!(s["Save"], "Salvar");
        assert_eq!(s["Open"], "Abrir");
        assert!(!s.contains_key("Quit"));
        assert!(s["%n files"].is_object());
    }

    #[test]
    fn reads_language_file() {
        let dir = std::env::temp_dir().join(format!("ferry-lang-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_language(&dir.join("missing")), "en");
        fs::write(dir.join("language"), "es-MX\n").unwrap();
        assert_eq!(read_language(&dir.join("language")), "es-MX");
        fs::write(dir.join("language"), "../../etc\n").unwrap();
        assert_eq!(read_language(&dir.join("language")), "en");
    }

    #[test]
    fn every_catalog_parses() {
        for (code, text) in CATALOGS {
            assert!(
                serde_json::from_str::<Value>(text).is_ok_and(|v| v.is_object()),
                "{code}.json"
            );
        }
    }
}
