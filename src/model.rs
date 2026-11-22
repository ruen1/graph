use rusqlite::Row;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone)]
pub struct Account {
    pub id: i64,
    pub access_token: String,
    pub short_name: String,
    pub author_name: String,
    pub author_url: String,
    pub auth_token: Option<String>,
    pub auth_created: i64,
    pub created_at: i64,
}

impl Account {
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            access_token: row.get("access_token")?,
            short_name: row.get("short_name")?,
            author_name: row.get("author_name")?,
            author_url: row.get("author_url")?,
            auth_token: row.get("auth_token")?,
            auth_created: row.get("auth_created")?,
            created_at: row.get("created_at")?,
        })
    }

    pub fn to_json(&self, base_url: &str) -> Value {
        json!({
            "short_name": self.short_name,
            "author_name": self.author_name,
            "author_url": self.author_url,
            "access_token": self.access_token,
            "auth_url": self.auth_token.as_ref().map(|token| format!("{base_url}/auth/{token}")),
        })
    }
}

#[derive(Clone)]
pub struct Page {
    pub id: String,
    pub path: String,
    pub account_id: i64,
    pub title: String,
    pub author_name: String,
    pub author_url: String,
    pub content: Vec<Node>,
    pub created_at: i64,
    pub updated_at: i64,
    pub views: i64,
    pub save_hash: String,
    pub hidden: bool,
}

impl Page {
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let content: String = row.get("content")?;
        let content = serde_json::from_str(&content).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                content.len(),
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
        Ok(Self {
            id: row.get("id")?,
            path: row.get("path")?,
            account_id: row.get("account_id")?,
            title: row.get("title")?,
            author_name: row.get("author_name")?,
            author_url: row.get("author_url")?,
            content,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
            views: row.get("views")?,
            save_hash: row.get("save_hash")?,
            hidden: row.get("hidden")?,
        })
    }

    pub fn to_json(&self, base_url: &str, return_content: bool, can_edit: Option<bool>) -> Value {
        let mut page = json!({
            "path": self.path,
            "url": format!("{base_url}/{}", self.path),
            "title": self.title,
            "description": self.description(),
            "author_name": self.author_name,
            "author_url": self.author_url,
            "views": self.views,
        });
        let object = page.as_object_mut().expect("page is an object");
        if self.author_name.is_empty() {
            object.remove("author_name");
        }
        if self.author_url.is_empty() {
            object.remove("author_url");
        }
        if let Some(image_url) = self.image_url() {
            object.insert("image_url".into(), json!(image_url));
        }
        if return_content {
            object.insert("content".into(), json!(self.content));
        }
        if let Some(can_edit) = can_edit {
            object.insert("can_edit".into(), json!(can_edit));
        }
        page
    }

    pub fn description(&self) -> String {
        let mut text = String::new();
        for node in &self.content {
            node.collect_text(&mut text);
        }
        let description = text.split_whitespace().collect::<Vec<_>>().join(" ");
        description.chars().take(200).collect()
    }

    pub fn content_char_count(&self) -> usize {
        self.content.iter().map(Node::char_count).sum()
    }

    pub fn image_url(&self) -> Option<String> {
        self.content.iter().find_map(Node::first_image)
    }
}

pub struct ReportedPage {
    pub page: Page,
    pub reasons: String,
    pub report_count: i64,
}

impl ReportedPage {
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            page: Page::from_row(row)?,
            reasons: row.get("reasons")?,
            report_count: row.get("report_count")?,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Node {
    Text(String),
    Element(NodeElement),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeElement {
    pub tag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attrs: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<Node>>,
}

impl Node {
    fn collect_text(&self, output: &mut String) {
        match self {
            Self::Text(text) => output.push_str(text),
            Self::Element(element) => {
                if let Some(children) = &element.children {
                    for child in children {
                        child.collect_text(output);
                    }
                }
                output.push(' ');
            }
        }
    }

    fn char_count(&self) -> usize {
        match self {
            Self::Text(text) => text.chars().count(),
            Self::Element(element) if element.tag == "br" => 1,
            Self::Element(element) => element
                .children
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(Self::char_count)
                .sum(),
        }
    }

    fn first_image(&self) -> Option<String> {
        match self {
            Self::Text(_) => None,
            Self::Element(element) => {
                if element.tag == "img" {
                    if let Some(src) = element.attrs.as_ref()?.get("src") {
                        return Some(src.clone());
                    }
                }
                element
                    .children
                    .as_ref()?
                    .iter()
                    .find_map(Node::first_image)
            }
        }
    }
}

pub fn random_hex(bytes: usize) -> String {
    use rand::RngCore;
    let mut value = vec![0; bytes];
    rand::thread_rng().fill_bytes(&mut value);
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod content_tests {
    use super::*;

    #[test]
    fn character_count_uses_visible_characters_instead_of_bytes_or_markup() {
        let nodes = vec![
            Node::Text("A🦀".into()),
            Node::Element(NodeElement {
                tag: "br".into(),
                attrs: None,
                children: None,
            }),
            Node::Element(NodeElement {
                tag: "strong".into(),
                attrs: None,
                children: Some(vec![Node::Text("!".into())]),
            }),
            Node::Element(NodeElement {
                tag: "img".into(),
                attrs: Some(BTreeMap::from([("src".into(), "/file/image.png".into())])),
                children: None,
            }),
        ];
        assert_eq!(nodes.iter().map(Node::char_count).sum::<usize>(), 4);
    }
}
