use crate::model::Node;
use std::collections::BTreeMap;

pub const CONTENT_TAGS: &[&str] = &[
    "a",
    "aside",
    "b",
    "blockquote",
    "br",
    "code",
    "em",
    "figcaption",
    "figure",
    "h3",
    "h4",
    "hr",
    "i",
    "iframe",
    "img",
    "li",
    "ol",
    "p",
    "pre",
    "s",
    "strong",
    "u",
    "ul",
    "video",
];

pub struct SanitizedContent(Vec<Node>);

impl SanitizedContent {
    pub fn new(nodes: &[Node], strip_utm: bool) -> Self {
        let mut nodes = nodes.to_vec();
        sanitize_nodes(&mut nodes, strip_utm);
        Self(nodes)
    }

    pub fn as_slice(&self) -> &[Node] {
        &self.0
    }
}

pub struct SanitizedReport {
    page_id: String,
    reason: String,
    comment: String,
}

impl SanitizedReport {
    pub fn new(page_id: &str, reason: &str, comment: &str) -> Result<Self, String> {
        const REASONS: &[&str] = &[
            "violence",
            "childabuse",
            "copyright",
            "illegal_drugs",
            "personal_details",
            "other",
        ];
        if page_id.is_empty() {
            return Err("PAGE_NOT_FOUND".into());
        }
        if !REASONS.contains(&reason) {
            return Err("REPORT_INVALID".into());
        }
        if comment.chars().count() > 500 {
            return Err("COMMENT_TOO_LONG".into());
        }
        Ok(Self {
            page_id: page_id.into(),
            reason: reason.into(),
            comment: sanitize_text(comment),
        })
    }

    pub fn page_id(&self) -> &str {
        &self.page_id
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    pub fn comment(&self) -> &str {
        &self.comment
    }
}

pub fn sanitize_text(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\r' | '\t'))
        .collect()
}

fn sanitize_nodes(nodes: &mut [Node], strip_utm: bool) {
    crate::handlers::urls::sanitize_url_nodes(nodes, strip_utm);
    for node in nodes {
        let Node::Element(element) = node else {
            continue;
        };
        if let Some(attrs) = element.attrs.take() {
            let attrs = sanitize_attributes(&element.tag, &attrs);
            if !attrs.is_empty() {
                element.attrs = Some(attrs);
            }
        }
        if let Some(children) = &mut element.children {
            sanitize_nodes(children, strip_utm);
        }
    }
}

pub fn sanitize_attributes(
    tag: &str,
    attrs: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let blank_target = tag == "a" && attrs.get("target").is_some_and(|value| value == "_blank");
    let mut sanitized = BTreeMap::new();
    for (name, value) in attrs {
        let allowed = match name.as_str() {
            "href" => tag == "a" && crate::handlers::urls::is_safe_url(value, false),
            "src" => {
                matches!(tag, "img" | "video" | "iframe")
                    && crate::handlers::urls::is_safe_resource_url(value, tag == "iframe")
            }
            "target" => tag == "a" && matches!(value.as_str(), "_blank" | "_self"),
            "dir" => matches!(value.as_str(), "auto" | "ltr" | "rtl"),
            "rel" => tag == "a" && !blank_target,
            "class" => tag == "br" && value == "inline",
            "id" => matches!(tag, "h3" | "h4") && !value.starts_with('_'),
            "width" | "height" | "frameborder" | "allowfullscreen" | "controls" | "data-label"
            | "data-title" => true,
            _ => false,
        };
        if allowed {
            sanitized.insert(name.clone(), value.clone());
        }
    }
    if blank_target {
        let mut rel = attrs
            .get("rel")
            .map(|value| {
                value
                    .split_whitespace()
                    .filter(|token| matches!(*token, "nofollow" | "ugc" | "sponsored"))
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for token in ["noopener", "noreferrer"] {
            if !rel.iter().any(|existing| existing == token) {
                rel.push(token.to_string());
            }
        }
        sanitized.insert("rel".into(), rel.join(" "));
    }
    sanitized
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sanitizes_urls_and_attributes_after_removing_utm_parameters() {
        let mut nodes: Vec<Node> = serde_json::from_value(json!([
            {"tag":"p","attrs":{"onclick":"alert(1)","dir":"auto"},"children":[
                {"tag":"a","attrs":{"href":"https://example.com/?id=1&utm_source=x","target":"_blank","rel":"opener nofollow","ping":"https://evil.example"},"children":["safe"]},
                {"tag":"a","attrs":{"href":"javascript:alert(1)?utm_source=x"},"children":["unsafe"]}
            ]},
            {"tag":"figure","children":[
                {"tag":"iframe","attrs":{"src":"https://example.com/embed?id=1&utm_source=x","srcdoc":"<script>alert(1)</script>"}},
                {"tag":"img","attrs":{"src":"data:text/html;base64,PHNjcmlwdD4=?utm_source=x","onerror":"alert(1)"}}
            ]}
        ]))
        .unwrap();
        sanitize_nodes(&mut nodes, true);
        let value = serde_json::to_value(nodes).unwrap();
        let serialized = value.to_string();
        assert!(!serialized.contains("utm_"));
        assert!(!serialized.contains("javascript:"));
        assert!(!serialized.contains("data:text/html"));
        assert!(!serialized.contains("onclick"));
        assert!(!serialized.contains("onerror"));
        assert!(!serialized.contains("srcdoc"));
        assert!(!serialized.contains("ping"));
        assert_eq!(
            value[0]["children"][0]["attrs"]["href"],
            "https://example.com/?id=1"
        );
        assert_eq!(
            value[0]["children"][0]["attrs"]["rel"],
            "nofollow noopener noreferrer"
        );
        assert_eq!(
            value[1]["children"][0]["attrs"]["src"],
            "https://example.com/embed?id=1"
        );
    }

    #[test]
    fn reports_require_valid_reasons_and_bounded_plain_text() {
        let report =
            SanitizedReport::new("page-id", "other", "bad\0<script>").expect("valid report");
        assert_eq!(report.page_id(), "page-id");
        assert_eq!(report.comment(), "bad<script>");
        assert_eq!(
            SanitizedReport::new("page-id", "not-a-reason", "comment")
                .err()
                .unwrap(),
            "REPORT_INVALID"
        );
        assert_eq!(
            SanitizedReport::new("page-id", "other", &"x".repeat(501))
                .err()
                .unwrap(),
            "COMMENT_TOO_LONG"
        );
        assert_eq!(
            SanitizedReport::new("", "other", "comment").err().unwrap(),
            "PAGE_NOT_FOUND"
        );
    }

    #[test]
    fn sanitized_content_is_constructed_through_the_pipeline() {
        let nodes: Vec<Node> = serde_json::from_value(json!([
            {"tag":"a","attrs":{"href":"javascript:alert(1)?utm_source=x","onclick":"alert(1)"},"children":["text"]}
        ]))
        .unwrap();
        let content = SanitizedContent::new(&nodes, true);
        let serialized = serde_json::to_string(content.as_slice()).unwrap();
        assert!(!serialized.contains("javascript:"));
        assert!(!serialized.contains("onclick"));
        assert!(!serialized.contains("utm_"));
    }
}
