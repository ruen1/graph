use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Request, header};
use axum::middleware::Next;
use axum::response::Response;
use std::collections::BTreeMap;
use std::sync::OnceLock;

type LanguageStrings = BTreeMap<String, String>;

#[derive(serde::Deserialize)]
struct Catalog {
    #[serde(flatten)]
    languages: BTreeMap<String, LanguageStrings>,
}

static CATALOG: OnceLock<Catalog> = OnceLock::new();

impl Catalog {
    fn load() -> Result<Self, toml::de::Error> {
        toml::from_str(include_str!("../Strings.toml"))
    }

    fn select(&self, accept_language: Option<&str>) -> (String, LanguageStrings) {
        for (language, _, _) in parse_accept_language(accept_language) {
            let base = language.split('-').next().unwrap_or(&language);
            for candidate in [language.as_str(), base] {
                if let Some(localized) = self.languages.get(candidate) {
                    let mut strings = self.languages.get("en").cloned().unwrap_or_default();
                    strings.extend(localized.clone());
                    return (candidate.to_string(), strings);
                }
            }
        }
        (
            "en".into(),
            self.languages.get("en").cloned().unwrap_or_default(),
        )
    }
}

fn catalog() -> &'static Catalog {
    CATALOG.get_or_init(|| Catalog::load().expect("valid Strings.toml"))
}

pub fn render(template: &str, headers: &HeaderMap) -> String {
    render_with_catalog(template, headers, catalog())
}

fn render_with_catalog(template: &str, headers: &HeaderMap, catalog: &Catalog) -> String {
    let accept_language = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok());
    let (_, strings) = catalog.select(accept_language);
    let json = serde_json::to_string(&strings)
        .expect("strings should serialize")
        .replace('<', "\\u003c");
    let mut rendered = template.replace("{{template_strings}}", &json);
    for (key, value) in strings {
        rendered = rendered.replace(&format!("{{{{{key}}}}}"), &escape_html(&value));
    }
    rendered
}

pub fn render_javascript(template: &str, headers: &HeaderMap) -> String {
    let accept_language = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok());
    let (_, strings) = catalog().select(accept_language);
    let json = serde_json::to_string(&strings)
        .expect("strings should serialize")
        .replace('<', "\\u003c");
    let mut rendered = template.replace("{{template_strings}}", &json);
    for (key, value) in strings {
        rendered = rendered.replace(&format!("{{{{{key}}}}}"), &escape_javascript(&value));
    }
    rendered
}

fn escape_javascript(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

pub fn language(headers: &HeaderMap) -> String {
    catalog()
        .select(
            headers
                .get(header::ACCEPT_LANGUAGE)
                .and_then(|value| value.to_str().ok()),
        )
        .0
}

pub async fn middleware(request: Request<Body>, next: Next) -> Response {
    let language = language(request.headers());
    let mut response = next.run(request).await;
    let is_localized = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.starts_with("text/html")
                || value.starts_with("application/javascript")
                || value.starts_with("text/javascript")
        });
    if is_localized {
        if let Ok(value) = HeaderValue::from_str(&language) {
            response
                .headers_mut()
                .insert(header::CONTENT_LANGUAGE, value);
        }
        response
            .headers_mut()
            .append(header::VARY, HeaderValue::from_static("accept-language"));
    }
    response
}

fn parse_accept_language(header: Option<&str>) -> Vec<(String, u16, usize)> {
    let mut languages = header
        .unwrap_or("")
        .split(',')
        .enumerate()
        .filter_map(|(index, item)| {
            let mut parts = item.trim().split(';');
            let language = parts.next()?.trim().to_ascii_lowercase();
            if language.is_empty() || language == "*" {
                return None;
            }
            let quality = parts
                .find_map(|part| part.trim().strip_prefix("q="))
                .and_then(|value| value.parse::<f32>().ok())
                .map(|value| (value.clamp(0.0, 1.0) * 1000.0) as u16)
                .unwrap_or(1000);
            (quality > 0).then_some((language, quality, index))
        })
        .collect::<Vec<_>>();
    languages.sort_by(|left, right| right.1.cmp(&left.1).then(left.2.cmp(&right.2)));
    languages
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        toml::from_str(
            r#"
            [en]
            publish = "Publish"
            greeting = "Hello"
            [fr]
            publish = "Publier"
            [es]
            publish = "Publicar"
            greeting = "Hola </script>"
            "#,
        )
        .unwrap()
    }

    #[test]
    fn quality_values_and_order_are_respected() {
        let catalog = catalog();
        assert_eq!(catalog.select(Some("fr;q=0.5,es;q=0.9")).0, "es");
        assert_eq!(catalog.select(Some("fr,es;q=0.5")).0, "fr");
        assert_eq!(catalog.select(Some("es;q=0,fr;q=0.8")).0, "fr");
    }

    #[test]
    fn regional_and_partial_translations_fall_back() {
        let catalog = catalog();
        let (language, strings) = catalog.select(Some("fr-CA"));
        assert_eq!(language, "fr");
        assert_eq!(strings["publish"], "Publier");
        assert_eq!(strings["greeting"], "Hello");
        assert_eq!(catalog.select(Some("de-DE")).0, "en");
        assert_eq!(catalog.select(Some("en-GB")).0, "en");
    }

    #[test]
    fn template_json_and_values_are_safe() {
        let catalog = catalog();
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT_LANGUAGE, "es".parse().unwrap());
        let rendered = render_with_catalog(
            "{{template_strings}} {{publish}} {{greeting}}",
            &headers,
            &catalog,
        );
        assert!(!rendered.contains("{{template_strings}}"));
        assert!(rendered.contains("Publicar"));
        assert!(rendered.contains("Hola &lt;/script&gt;"));
        assert!(!rendered.contains("Hola </script>"));
        let json = render_with_catalog("{{template_strings}}", &headers, &catalog);
        assert!(serde_json::from_str::<serde_json::Value>(&json).is_ok());
    }

    #[test]
    fn bundled_english_strings_are_complete() {
        let catalog = super::catalog();
        for key in [
            "publish",
            "edit",
            "report_content",
            "report_page",
            "page_not_found",
            "create_new",
            "title_placeholder",
            "author_placeholder",
            "your_name_placeholder",
            "content_placeholder",
            "homepage_javascript_notice",
            "source_code",
        ] {
            assert!(catalog.languages["en"].contains_key(key), "missing {key}");
        }
    }
}

#[cfg(test)]
mod javascript_tests {
    #[test]
    fn javascript_strings_escape_quotes_backslashes_and_newlines() {
        assert_eq!(
            super::escape_javascript("can't\nnext\\line"),
            "can\\'t\\nnext\\\\line"
        );
    }
}
