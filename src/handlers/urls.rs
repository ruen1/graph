use crate::model::Node;
use url::Url;

pub fn sanitize_url_nodes(nodes: &mut [Node], strip_utm: bool) {
    normalize_youtube_embeds(nodes, strip_utm);
    if strip_utm {
        strip_utm_from_nodes(nodes);
    }
    for node in nodes {
        let Node::Element(element) = node else {
            continue;
        };
        if let Some(attrs) = &mut element.attrs {
            match element.tag.as_str() {
                "a" => {
                    if attrs
                        .get("href")
                        .is_some_and(|value| !is_safe_url(value, false))
                    {
                        attrs.remove("href");
                    }
                }
                "img" | "video" | "iframe" => {
                    if attrs
                        .get("src")
                        .is_some_and(|value| !is_safe_resource_url(value, element.tag == "iframe"))
                    {
                        attrs.remove("src");
                    }
                }
                _ => {}
            }
        }
        if let Some(children) = &mut element.children {
            sanitize_url_nodes(children, strip_utm);
        }
    }
}

pub fn strip_utm_from_nodes(nodes: &mut [Node]) {
    for node in nodes {
        let Node::Element(element) = node else {
            continue;
        };
        if let Some(attrs) = &mut element.attrs {
            let attribute = match element.tag.as_str() {
                "a" => Some("href"),
                "iframe" | "img" | "video" => Some("src"),
                _ => None,
            };
            if let Some(attribute) = attribute {
                if let Some(value) = attrs.get_mut(attribute) {
                    *value = strip_utm_from_url(value);
                }
            }
        }
        if let Some(children) = &mut element.children {
            strip_utm_from_nodes(children);
        }
    }
}

pub fn sanitize_author_url(value: &str, strip_utm: bool) -> String {
    let value = if strip_utm {
        strip_utm_from_url(value)
    } else {
        value.to_string()
    };
    if is_safe_url(&value, false) {
        value
    } else {
        String::new()
    }
}

pub fn is_safe_url(value: &str, https_only: bool) -> bool {
    let value = value.trim();
    if value.chars().any(|character| character.is_control()) {
        return false;
    }
    let lowercase = value.to_ascii_lowercase();
    lowercase.starts_with("https://")
        || !https_only
            && (lowercase.starts_with("http://")
                || lowercase.starts_with("mailto:")
                || lowercase.starts_with("tel:")
                || value.starts_with('/'))
}

pub(crate) fn is_safe_resource_url(value: &str, https_only: bool) -> bool {
    if !is_safe_url(value, https_only) {
        return false;
    }
    let value = value.trim();
    let lowercase = value.to_ascii_lowercase();
    lowercase.starts_with("https://")
        || !https_only && (lowercase.starts_with("http://") || value.starts_with('/'))
}

fn normalize_youtube_embeds(nodes: &mut [Node], strip_utm: bool) {
    for node in nodes {
        let Node::Element(element) = node else {
            continue;
        };
        if element.tag == "iframe" {
            if let Some(src) = element
                .attrs
                .as_mut()
                .and_then(|attrs| attrs.get_mut("src"))
            {
                let original = if let Some(query) = src.strip_prefix("/embed/youtube?") {
                    url::form_urlencoded::parse(query.as_bytes())
                        .find(|(name, _)| name == "url")
                        .map(|(_, value)| value.into_owned())
                } else {
                    Some(src.clone())
                };
                if let Some(original) = original {
                    if let Some(embed) = youtube_embed_url(&original, strip_utm) {
                        *src = embed;
                    }
                }
            }
        }
        if let Some(children) = &mut element.children {
            normalize_youtube_embeds(children, strip_utm);
        }
    }
}

pub(crate) fn youtube_embed_url(value: &str, strip_utm: bool) -> Option<String> {
    let parsed = Url::parse(value).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    if !is_youtube_host(&host) {
        return None;
    }
    let mut segments = parsed
        .path_segments()?
        .filter(|segment| !segment.is_empty());
    let video_id = if host == "youtu.be" {
        segments.next()?.to_string()
    } else if parsed.path() == "/watch" {
        parsed
            .query_pairs()
            .find(|(name, _)| name == "v")
            .map(|(_, value)| value.into_owned())?
    } else {
        let kind = segments.next()?;
        if !matches!(kind, "shorts" | "live" | "embed") {
            return None;
        }
        segments.next()?.to_string()
    };
    if video_id.is_empty()
        || video_id.len() > 64
        || !video_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return None;
    }
    let query = parsed
        .query_pairs()
        .filter(|(name, _)| name != "v")
        .collect::<Vec<_>>();
    let mut result = format!("https://www.youtube.com/embed/{video_id}");
    if !query.is_empty() {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(
                query
                    .iter()
                    .map(|(name, value)| (name.as_ref(), value.as_ref())),
            )
            .finish();
        result.push('?');
        result.push_str(&query);
    }
    Some(if strip_utm {
        strip_utm_from_url(&result)
    } else {
        result
    })
}

fn is_youtube_url(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    let authority = if let Some(value) = value.strip_prefix("https://") {
        value
    } else if let Some(value) = value.strip_prefix("http://") {
        value
    } else if let Some(value) = value.strip_prefix("//") {
        value
    } else {
        return false;
    };
    let authority = authority.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = host.split(':').next().unwrap_or(host);
    is_youtube_host(host)
}

fn is_youtube_host(host: &str) -> bool {
    host == "youtu.be"
        || host == "youtube.com"
        || host.ends_with(".youtube.com")
        || host == "youtube-nocookie.com"
        || host.ends_with(".youtube-nocookie.com")
}

fn strip_utm_from_url(value: &str) -> String {
    let Some(query_start) = value.find('?') else {
        return value.to_string();
    };
    let fragment_start = value[query_start + 1..]
        .find('#')
        .map(|index| query_start + 1 + index);
    let query_end = fragment_start.unwrap_or(value.len());
    let query = &value[query_start + 1..query_end];
    let youtube = is_youtube_url(value);
    let filtered = query
        .split('&')
        .filter(|parameter| {
            let name = parameter
                .split('=')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            !name.starts_with("utm_") && !(youtube && matches!(name.as_str(), "si" | "pp"))
        })
        .collect::<Vec<_>>();
    if filtered.len() == query.split('&').count() {
        return value.to_string();
    }
    let query = filtered.join("&");
    let mut result = value[..query_start].to_string();
    if !query.is_empty() {
        result.push('?');
        result.push_str(&query);
    }
    if let Some(fragment_start) = fragment_start {
        result.push_str(&value[fragment_start..]);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn removes_only_utm_parameters_and_preserves_url_structure() {
        for (original, expected) in [
            (
                "https://www.youtube.com/watch?v=abc&utm_source=newsletter#t=30",
                "https://www.youtube.com/watch?v=abc#t=30",
            ),
            (
                "https://example.com/?a=1&UTM_MEDIUM=x&b=2",
                "https://example.com/?a=1&b=2",
            ),
            ("https://example.com/?utm_source=x", "https://example.com/"),
            ("/relative?x=1&utm_campaign=test#frag", "/relative?x=1#frag"),
            (
                "https://example.com/?a=1&a=2&utm_term=x&empty=&flag",
                "https://example.com/?a=1&a=2&empty=&flag",
            ),
            (
                "https://example.com/?x=utm_source",
                "https://example.com/?x=utm_source",
            ),
            (
                "https://example.com/?utm%5Fsource=x",
                "https://example.com/?utm%5Fsource=x",
            ),
        ] {
            assert_eq!(strip_utm_from_url(original), expected);
        }
    }

    #[test]
    fn removes_si_and_pp_only_on_youtube_urls() {
        for (original, expected) in [
            (
                "https://www.youtube.com/watch?v=abc&si=tracking&pp=tracking&t=30",
                "https://www.youtube.com/watch?v=abc&t=30",
            ),
            (
                "https://youtu.be/abc?si=x&pp=y&utm_source=z",
                "https://youtu.be/abc",
            ),
            (
                "https://www.youtube-nocookie.com/embed/abc?si=x&pp=y&rel=0",
                "https://www.youtube-nocookie.com/embed/abc?rel=0",
            ),
            (
                "//www.youtube.com/watch?v=abc&SI=x",
                "//www.youtube.com/watch?v=abc",
            ),
            (
                "https://example.com/watch?si=keep&pp=keep&utm_source=x",
                "https://example.com/watch?si=keep&pp=keep",
            ),
            (
                "https://youtube.com.evil.example/watch?si=keep&pp=keep",
                "https://youtube.com.evil.example/watch?si=keep&pp=keep",
            ),
        ] {
            assert_eq!(strip_utm_from_url(original), expected);
        }
    }

    #[test]
    fn canonicalizes_editor_youtube_embeds_before_sanitization() {
        let mut nodes: Vec<Node> = serde_json::from_value(json!([
            {"tag":"iframe","attrs":{"src":"/embed/youtube?url=https%3A%2F%2Fwww.youtube.com%2Fwatch%3Fv%3Dabc123%26t%3D30%26si%3Dtracking"}},
            {"tag":"iframe","attrs":{"src":"/embed/youtube?url=https%3A%2F%2Fyoutu.be%2FdQw4w9WgXcQ%3Fsi%3Dx%26pp%3Dy%26t%3D42"}},
            {"tag":"iframe","attrs":{"src":"https://www.youtube.com/shorts/abc123?si=x&pp=y"}},
            {"tag":"iframe","attrs":{"src":"/embed/youtube?url=javascript%3Aalert(1)"}},
            {"tag":"iframe","attrs":{"src":"/embed/youtube?url=https%3A%2F%2Fevil.example%2Fwatch%3Fv%3Dabc"}}
        ]))
        .unwrap();
        sanitize_url_nodes(&mut nodes, true);
        let value = serde_json::to_value(nodes).unwrap();
        assert_eq!(
            value[0]["attrs"]["src"],
            "https://www.youtube.com/embed/abc123?t=30"
        );
        assert_eq!(
            value[1]["attrs"]["src"],
            "https://www.youtube.com/embed/dQw4w9WgXcQ?t=42"
        );
        assert_eq!(
            value[2]["attrs"]["src"],
            "https://www.youtube.com/embed/abc123"
        );
        assert!(value[3]["attrs"].get("src").is_none());
        assert!(value[4]["attrs"].get("src").is_none());
        assert_eq!(
            youtube_embed_url("https://www.youtube.com/live/abc?rel=0", true).unwrap(),
            "https://www.youtube.com/embed/abc?rel=0"
        );
        assert_eq!(
            youtube_embed_url("https://www.youtube.com/embed/abc?rel=0", true).unwrap(),
            "https://www.youtube.com/embed/abc?rel=0"
        );
    }

    #[test]
    fn author_urls_are_cleaned_and_validated() {
        assert_eq!(
            sanitize_author_url("https://example.com/?id=1&utm_source=x", true),
            "https://example.com/?id=1"
        );
        assert_eq!(
            sanitize_author_url("javascript:alert(1)?utm_source=x", true),
            ""
        );
        assert_eq!(sanitize_author_url("java\nscript:alert(1)", true), "");
    }
}
