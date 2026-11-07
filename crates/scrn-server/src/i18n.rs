//! Translation catalogs for the web UI.
//!
//! Catalogs live in `crates/scrn-server/locales/<lang>.txt` and are embedded
//! at compile time. Format: one `key = value` per line, `#` starts a comment
//! line. Values may contain `{name}` placeholders.
//!
//! Which language a request gets is decided in [`crate::prefs`].

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::repo::users::Language;

const DE: &str = include_str!("../locales/de.txt");
const EN: &str = include_str!("../locales/en.txt");

/// The default UI language.
pub const DEFAULT_LANGUAGE: Language = Language::En;

static CATALOGS: LazyLock<I18n> = LazyLock::new(|| {
    // `init_state` calls `I18n::load` at startup and refuses to start on
    // errors, so the empty fallback is never used by a running server.
    I18n::load().unwrap_or_else(|_| I18n {
        de: HashMap::new(),
        en: HashMap::new(),
    })
});

/// The embedded catalogs, parsed once.
pub fn catalogs() -> &'static I18n {
    &CATALOGS
}

/// A catalog file could not be parsed.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CatalogError {
    #[error("catalog {lang}, line {line}: expected `key = value`")]
    Syntax { lang: &'static str, line: usize },
    #[error("catalog {lang}, line {line}: duplicate key {key}")]
    Duplicate {
        lang: &'static str,
        line: usize,
        key: String,
    },
}

/// All loaded catalogs.
#[derive(Debug)]
pub struct I18n {
    de: HashMap<String, String>,
    en: HashMap<String, String>,
}

impl I18n {
    /// Parses the embedded catalogs.
    pub fn load() -> Result<Self, CatalogError> {
        Ok(Self {
            de: parse("de", DE)?,
            en: parse("en", EN)?,
        })
    }

    fn catalog(&self, lang: Language) -> &HashMap<String, String> {
        match lang {
            Language::De => &self.de,
            Language::En => &self.en,
        }
    }

    /// Looks up a message. A missing key yields the key itself, so a gap
    /// is visible in the UI instead of breaking the page; a test keeps the
    /// catalogs complete.
    pub fn t<'a>(&'a self, lang: Language, key: &'a str) -> &'a str {
        self.catalog(lang).get(key).map_or(key, String::as_str)
    }

    /// Looks up a message and replaces `{name}` placeholders.
    pub fn tf(&self, lang: Language, key: &str, args: &[(&str, &str)]) -> String {
        let mut text = self.t(lang, key).to_owned();
        for (name, value) in args {
            text = text.replace(&format!("{{{name}}}"), value);
        }
        text
    }
}

/// Language code for the `lang` attribute.
pub fn code(lang: Language) -> &'static str {
    match lang {
        Language::De => "de",
        Language::En => "en",
    }
}

fn parse(lang: &'static str, source: &str) -> Result<HashMap<String, String>, CatalogError> {
    let mut map = HashMap::new();
    for (index, raw) in source.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .map(|(k, v)| (k.trim(), v.trim()))
            .filter(|(k, _)| !k.is_empty())
            .ok_or(CatalogError::Syntax {
                lang,
                line: index + 1,
            })?;
        if map.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(CatalogError::Duplicate {
                lang,
                line: index + 1,
                key: key.to_owned(),
            });
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn catalogs_parse() {
        I18n::load().unwrap();
    }

    #[test]
    fn catalogs_have_the_same_keys() {
        let i18n = I18n::load().unwrap();
        let de: BTreeSet<_> = i18n.de.keys().collect();
        let en: BTreeSet<_> = i18n.en.keys().collect();
        let only_de: Vec<_> = de.difference(&en).collect();
        let only_en: Vec<_> = en.difference(&de).collect();
        assert!(
            only_de.is_empty() && only_en.is_empty(),
            "keys missing in en: {only_de:?}, keys missing in de: {only_en:?}"
        );
    }

    #[test]
    fn no_empty_values() {
        let i18n = I18n::load().unwrap();
        for (lang, catalog) in [("de", &i18n.de), ("en", &i18n.en)] {
            for (key, value) in catalog {
                assert!(!value.is_empty(), "{lang}: {key} is empty");
            }
        }
    }

    #[test]
    fn parser_rejects_bad_lines_and_duplicates() {
        assert_eq!(
            parse("xx", "# comment\n\nno equals sign"),
            Err(CatalogError::Syntax {
                lang: "xx",
                line: 3
            })
        );
        assert_eq!(
            parse("xx", "= value"),
            Err(CatalogError::Syntax {
                lang: "xx",
                line: 1
            })
        );
        assert!(matches!(
            parse("xx", "a = 1\na = 2"),
            Err(CatalogError::Duplicate { line: 2, .. })
        ));
        let map = parse("xx", "a.b = x = y").unwrap();
        assert_eq!(map["a.b"], "x = y");
    }

    #[test]
    fn lookup_with_placeholders_and_fallback() {
        let i18n = I18n::load().unwrap();
        assert_eq!(i18n.t(Language::De, "login.title"), "Anmelden");
        assert_eq!(i18n.t(Language::En, "login.title"), "Sign in");
        assert_eq!(i18n.t(Language::De, "no.such.key"), "no.such.key");
        assert_eq!(
            i18n.tf(Language::En, "gallery.alt", &[("date", "today")]),
            "Screenshot from today"
        );
    }

    /// Every catalog key referenced in templates or in the server code must
    /// exist. Keys are found as string literals of the form `"group.name"`
    /// whose group is used by the catalogs.
    #[test]
    fn all_referenced_keys_exist() {
        let catalogs = I18n::load().unwrap();
        let groups: std::collections::HashSet<&str> = catalogs
            .de
            .keys()
            .filter_map(|k| k.split('.').next())
            .collect();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        for dir in ["templates", "src", "src/routes", "src/api", "src/repo"] {
            for entry in std::fs::read_dir(root.join(dir)).unwrap() {
                let path = entry.unwrap().path();
                if path.is_file() {
                    files.push(path);
                }
            }
        }
        let mut checked = 0;
        for path in files {
            let text = std::fs::read_to_string(&path).unwrap();
            for literal in text.split('"').skip(1).step_by(2) {
                let Some((group, rest)) = literal.split_once('.') else {
                    continue;
                };
                // File names like "error.html" or "app.css" are not keys.
                let is_file = [".html", ".css", ".js", ".svg", ".png", ".txt"]
                    .iter()
                    .any(|ext| literal.ends_with(ext));
                let looks_like_key = groups.contains(group)
                    && !rest.is_empty()
                    && !is_file
                    && literal.bytes().all(|b| {
                        b.is_ascii_lowercase() || b.is_ascii_digit() || b"._".contains(&b)
                    });
                if looks_like_key {
                    checked += 1;
                    for (lang, map) in [("de", &catalogs.de), ("en", &catalogs.en)] {
                        assert!(
                            map.contains_key(literal),
                            "{} uses {literal:?}, missing in {lang}",
                            path.display()
                        );
                    }
                }
            }
        }
        assert!(
            checked > 50,
            "only {checked} keys found, is the scan broken?"
        );
    }
}
