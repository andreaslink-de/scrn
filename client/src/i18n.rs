//! Strings of the tray menu and the notifications. German is the default;
//! English is used when the system language is not German. The webviews
//! have their own catalog in `ui/i18n.js`.

use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    De,
    En,
}

/// The language from `LC_ALL`, `LC_MESSAGES` or `LANG`, as POSIX orders them.
pub fn detect(var: impl Fn(&str) -> Option<String>) -> Lang {
    let value = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|name| var(name))
        .find(|v| !v.is_empty());
    match value {
        Some(v) if !v.to_ascii_lowercase().starts_with("de") && v != "C" && v != "POSIX" => {
            Lang::En
        }
        _ => Lang::De,
    }
}

static LANG: LazyLock<Lang> = LazyLock::new(|| detect(|name| std::env::var(name).ok()));

pub fn lang() -> Lang {
    *LANG
}

/// The string for `key` in the system language.
pub fn t(key: &str) -> &'static str {
    tr(lang(), key)
}

pub fn tr(lang: Lang, key: &str) -> &'static str {
    let (de, en) = match key {
        "tray.capture" => ("Screenshot aufnehmen", "Take screenshot"),
        "tray.copy_last" => ("Letzten Link kopieren", "Copy last link"),
        "tray.recent" => ("Letzte Links", "Recent links"),
        "tray.recent_empty" => ("Noch keine Uploads", "No uploads yet"),
        "tray.gallery" => ("Galerie öffnen", "Open gallery"),
        "tray.settings" => ("Einstellungen …", "Settings …"),
        "tray.quit" => ("Beenden", "Quit"),
        "tray.about" => ("Über SCRN", "About SCRN"),
        "tray.project" => ("Projekt auf GitHub", "Project on GitHub"),
        "tray.private" => ("privat", "private"),
        "notify.copied" => ("Link kopiert", "Link copied"),
        "notify.private" => ("Privat hochgeladen", "Uploaded privately"),
        "notify.private_body" => (
            "Private Bilder haben keinen Link. Du findest es in der Galerie.",
            "Private images have no link. You find it in the gallery.",
        ),
        "notify.saved" => ("Gespeichert", "Saved"),
        "notify.capture_failed" => ("Screenshot fehlgeschlagen", "Screenshot failed"),
        "permission.no_app" => (
            "SCRN läuft nicht als installierte App.",
            "SCRN does not run as an installed app.",
        ),
        "notify.upload_failed" => ("Hochladen fehlgeschlagen", "Upload failed"),
        "notify.kept" => ("Das Bild liegt unter {0}", "The image is in {0}"),
        "notify.no_links" => ("Noch kein Link", "No link yet"),
        "notify.no_links_body" => (
            "Es gibt noch keinen hochgeladenen Screenshot.",
            "There is no uploaded screenshot yet.",
        ),
        "notify.not_configured" => ("Server nicht eingerichtet", "Server not set up"),
        "notify.not_configured_body" => (
            "Trage Server und Token in den Einstellungen ein.",
            "Enter server and token in the settings.",
        ),
        "error.network" => (
            "Der Server ist nicht erreichbar.",
            "The server cannot be reached.",
        ),
        "error.unauthorized" => (
            "Der Token ist ungültig oder wurde widerrufen.",
            "The token is invalid or was revoked.",
        ),
        "error.quota" => ("Dein Speicherplatz ist voll.", "Your storage is full."),
        "error.too_large" => ("Das Bild ist zu groß.", "The image is too large."),
        "error.rate" => (
            "Zu viele Uploads, bitte kurz warten.",
            "Too many uploads, please wait a moment.",
        ),
        "error.disabled" => ("Dein Konto ist gesperrt.", "Your account is disabled."),
        "error.server" => (
            "Der Server meldet einen Fehler.",
            "The server reports an error.",
        ),
        "error.not_scrn" => (
            "Unter dieser Adresse antwortet kein SCRN-Server.",
            "No SCRN server answers at this address.",
        ),
        "error.url_scheme" => (
            "Die Adresse muss mit https:// (oder http://) beginnen.",
            "The address must start with https:// (or http://).",
        ),
        "error.url" => (
            "Das ist keine gültige Adresse.",
            "That is no valid address.",
        ),
        "error.url_extra" => (
            "Die Adresse darf kein ? oder # enthalten.",
            "The address must not contain ? or #.",
        ),
        "error.expiry" => (
            "Links können höchstens 3650 Tage gelten.",
            "Links can last at most 3650 days.",
        ),
        "error.no_token" => ("Kein Token gespeichert.", "No token stored."),
        "error.bad_token" => (
            "Das sieht nicht wie ein SCRN-Token aus (scrn_…).",
            "That does not look like a SCRN token (scrn_…).",
        ),
        "error.keyring" => (
            "Der Schlüsselbund ist nicht erreichbar.",
            "The keyring cannot be reached.",
        ),
        "age.now" => ("gerade eben", "just now"),
        "age.minutes" => ("vor {0} Min.", "{0} min ago"),
        "age.hours" => ("vor {0} Std.", "{0} h ago"),
        "age.days" => ("vor {0} Tg.", "{0} d ago"),
        _ => return "",
    };
    match lang {
        Lang::De => de,
        Lang::En => en,
    }
}

/// Replaces `{0}` in `template`.
pub fn fill(template: &str, value: &str) -> String {
    template.replace("{0}", value)
}

/// How long ago `then` was, e.g. "vor 5 Min.".
pub fn age(lang: Lang, then: i64, now: i64) -> String {
    let secs = (now - then).max(0);
    let (key, n) = match secs {
        0..60 => return tr(lang, "age.now").to_owned(),
        60..3600 => ("age.minutes", secs / 60),
        3600..86_400 => ("age.hours", secs / 3600),
        _ => ("age.days", secs / 86_400),
    };
    fill(tr(lang, key), &n.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn language_follows_the_locale_variables() {
        assert_eq!(detect(env(&[])), Lang::De);
        assert_eq!(detect(env(&[("LANG", "de_DE.UTF-8")])), Lang::De);
        assert_eq!(detect(env(&[("LANG", "en_US.UTF-8")])), Lang::En);
        assert_eq!(detect(env(&[("LANG", "C")])), Lang::De);
        assert_eq!(
            detect(env(&[
                ("LANG", "de_DE.UTF-8"),
                ("LC_MESSAGES", "en_GB.UTF-8")
            ])),
            Lang::En
        );
        assert_eq!(
            detect(env(&[("LC_ALL", "de_AT.UTF-8"), ("LANG", "en_US.UTF-8")])),
            Lang::De
        );
        assert_eq!(
            detect(env(&[("LC_ALL", ""), ("LANG", "fr_FR.UTF-8")])),
            Lang::En
        );
    }

    #[test]
    fn every_key_exists_in_both_languages() {
        for key in [
            "tray.capture",
            "tray.copy_last",
            "tray.recent",
            "tray.gallery",
            "tray.about",
            "tray.project",
            "notify.copied",
            "notify.capture_failed",
            "permission.no_app",
            "error.network",
            "error.url_scheme",
            "age.days",
        ] {
            assert!(!tr(Lang::De, key).is_empty(), "{key}");
            assert!(!tr(Lang::En, key).is_empty(), "{key}");
        }
        assert_eq!(tr(Lang::De, "no.such.key"), "");
    }

    #[test]
    fn ages_are_rounded_down() {
        assert_eq!(age(Lang::De, 100, 130), "gerade eben");
        assert_eq!(age(Lang::De, 0, 5 * 60 + 59), "vor 5 Min.");
        assert_eq!(age(Lang::En, 0, 2 * 3600), "2 h ago");
        assert_eq!(age(Lang::De, 0, 3 * 86_400), "vor 3 Tg.");
        assert_eq!(age(Lang::De, 200, 100), "gerade eben");
    }
}
