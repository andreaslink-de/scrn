//! Plain server-rendered HTML. Styling and a template engine follow in step
//! S7a; until then pages are assembled here. All text comes from the
//! translation catalogs and every dynamic value is escaped.

use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::i18n::{self, I18n};
use crate::repo::users::Language;

/// Escapes text for use in HTML element content and quoted attributes.
pub fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Rendering context: catalogs and language.
#[derive(Clone, Copy)]
pub struct Ctx<'a> {
    pub i18n: &'a I18n,
    pub lang: Language,
}

impl<'a> Ctx<'a> {
    pub fn new(lang: Language) -> Self {
        Self {
            i18n: i18n::catalogs(),
            lang,
        }
    }

    /// Translated and escaped message.
    pub fn t(&self, key: &str) -> String {
        esc(self.i18n.t(self.lang, key))
    }

    /// Translated message with placeholders, escaped as a whole.
    pub fn tf(&self, key: &str, args: &[(&str, &str)]) -> String {
        esc(&self.i18n.tf(self.lang, key, args))
    }
}

/// Navigation for logged-in pages.
pub struct Nav<'a> {
    pub is_admin: bool,
    pub csrf: &'a str,
}

/// Renders a complete page.
pub fn page(ctx: &Ctx<'_>, title_key: &str, nav: Option<&Nav<'_>>, body: &str) -> String {
    let title = ctx.t(title_key);
    let app = ctx.t("app.name");
    let nav_html = nav.map_or_else(String::new, |nav| {
        let admin = if nav.is_admin {
            format!(r#" <a href="/admin">{}</a>"#, ctx.t("nav.admin"))
        } else {
            String::new()
        };
        format!(
            r#"<nav><a href="/app">{home}</a>{admin} <form method="post" action="/logout">{csrf}<button type="submit">{logout}</button></form></nav>"#,
            home = ctx.t("nav.home"),
            csrf = csrf_field(nav.csrf),
            logout = ctx.t("nav.logout"),
        )
    });
    format!(
        "<!DOCTYPE html>\n<html lang=\"{lang}\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{title} · {app}</title>\n</head>\n<body>\n{nav_html}\n<main>\n<h1>{title}</h1>\n{body}\n</main>\n</body>\n</html>\n",
        lang = i18n::code(ctx.lang),
    )
}

/// Wraps rendered HTML in a response. Pages may contain one-time secrets
/// or per-user data, so they are never cached.
pub fn response(status: StatusCode, html: String) -> Response {
    let mut response = (status, html).into_response();
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// An error page with a translated message.
pub fn error_page(lang: Language, status: StatusCode, message_key: &str) -> Response {
    let ctx = Ctx::new(lang);
    let body = format!(
        r#"<p role="alert">{}</p>
<p><a href="/">{}</a></p>"#,
        ctx.t(message_key),
        ctx.t("error.back")
    );
    response(status, page(&ctx, "error.title", None, &body))
}

pub fn csrf_field(token: &str) -> String {
    format!(
        r#"<input type="hidden" name="csrf" value="{}">"#,
        esc(token)
    )
}

/// An already translated, escaped error message, or nothing.
pub fn alert(message: Option<&str>) -> String {
    message.map_or_else(String::new, |m| format!(r#"<p role="alert">{m}</p>"#))
}

/// A labelled input field.
pub struct Field<'a> {
    pub name: &'a str,
    pub label: String,
    pub kind: &'a str,
    pub value: &'a str,
    pub autocomplete: &'a str,
    pub hint: Option<String>,
    pub extra: &'a str,
}

impl Field<'_> {
    pub fn render(&self) -> String {
        let hint = self.hint.as_ref().map_or_else(String::new, |h| {
            format!(r#"<br><small id="{}-hint">{h}</small>"#, self.name)
        });
        let described = if self.hint.is_some() {
            format!(r#" aria-describedby="{}-hint""#, self.name)
        } else {
            String::new()
        };
        format!(
            r#"<p><label for="{name}">{label}</label><br><input id="{name}" name="{name}" type="{kind}" value="{value}" autocomplete="{ac}" required{described}{extra}>{hint}</p>"#,
            name = self.name,
            label = self.label,
            kind = self.kind,
            value = esc(self.value),
            ac = self.autocomplete,
            extra = self.extra,
        )
    }
}

/// Formats a Unix timestamp as `YYYY-MM-DD HH:MM UTC`.
pub fn format_time(ts: i64) -> String {
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        secs / 3600,
        secs % 3600 / 60
    )
}

/// Converts days since 1970-01-01 to a proleptic Gregorian date
/// (algorithm by Howard Hinnant).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_html() {
        assert_eq!(
            esc(r#"<a href="x">'&'</a>"#),
            "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;"
        );
    }

    #[test]
    fn formats_timestamps() {
        assert_eq!(format_time(0), "1970-01-01 00:00 UTC");
        assert_eq!(format_time(951_782_400), "2000-02-29 00:00 UTC");
        assert_eq!(format_time(1_793_914_620), "2026-11-05 21:37 UTC");
        assert_eq!(format_time(-60), "1969-12-31 23:59 UTC");
    }

    #[test]
    fn page_uses_language_and_escapes_values() {
        let ctx = Ctx::new(Language::En);
        let html = page(&ctx, "login.title", None, "<p>x</p>");
        assert!(html.contains(r#"<html lang="en">"#));
        assert!(html.contains("<title>Sign in · SCRN</title>"));
        let field = Field {
            name: "username",
            label: ctx.t("field.username"),
            kind: "text",
            value: "\"><script>",
            autocomplete: "username",
            hint: None,
            extra: "",
        };
        assert!(
            field
                .render()
                .contains(r#"value="&quot;&gt;&lt;script&gt;""#)
        );
    }
}
