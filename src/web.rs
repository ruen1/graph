use crate::api::parse_editor_content;
use crate::db::Store;
use crate::model::{Account, Node};
use axum::extract::{Form, Multipart, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::{Json, Router, routing::get, routing::post};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::{DateTime, Utc};
use rusqlite::OptionalExtension;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

const INDEX_HTML: &str = include_str!("../static/index.html");
const ARTICLE_HTML: &str = include_str!("../static/article.html");
const API_HTML: &str = include_str!("../static/api.html");
const NOT_FOUND_HTML: &str = include_str!("../static/404.html");
const CORE_JS: &str = include_str!("../static/js/core.min.js");

async fn index(State(store): State<Arc<Store>>, headers: HeaderMap) -> Html<String> {
    Html(crate::i18n::render(
        &with_config(INDEX_HTML, &store.frontend),
        &headers,
    ))
}

async fn api_docs(State(store): State<Arc<Store>>, headers: HeaderMap) -> Html<String> {
    Html(crate::i18n::render(
        &with_config(API_HTML, &store.frontend),
        &headers,
    ))
}

async fn favicon(State(store): State<Arc<Store>>, headers: HeaderMap) -> Response {
    file_response("static/favicon.ico", &headers, &store.frontend).await
}

async fn robots(State(store): State<Arc<Store>>, headers: HeaderMap) -> Response {
    file_response("static/robots.txt", &headers, &store.frontend).await
}

async fn core_js(State(store): State<Arc<Store>>, headers: HeaderMap) -> Response {
    let mut response =
        crate::i18n::render_javascript(&with_config_javascript(CORE_JS, &store.frontend), &headers)
            .into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/javascript; charset=utf-8"),
    );
    response
}

#[derive(serde::Deserialize)]
struct ConfigParams {
    page_id: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct FrontendResponse {
    api_url: &'static str,
    upload_enabled: bool,
    datetime: i64,
    page_id: Value,
    editable: bool,
    report_enabled: bool,
}

impl From<crate::db::FrontendConfig> for FrontendResponse {
    fn from(config: crate::db::FrontendConfig) -> Self {
        Self {
            api_url: "/api",
            upload_enabled: config.upload_enabled,
            datetime: 0,
            page_id: Value::Number(0.into()),
            editable: config.editable,
            report_enabled: config.report_enabled,
        }
    }
}

fn frontend_config(
    store: &Store,
    params: &ConfigParams,
    headers: &HeaderMap,
) -> Result<FrontendResponse, Response> {
    let mut response = FrontendResponse::from(store.frontend.clone());
    if let Some(page_id) = &params.page_id {
        if page_id == "api" {
            response.datetime = 1482182613;
            response.page_id = json!(page_id);
            response.editable = false;
        } else if page_id != "0" {
            let page = store
                .page_by_id(page_id)
                .map_err(|_| not_found(headers, &store.frontend))?
                .ok_or_else(|| not_found(headers, &store.frontend))?;
            response.datetime = page.created_at;
            response.page_id = json!(page.id);
        }
    }
    Ok(response)
}

async fn config_json(
    State(store): State<Arc<Store>>,
    Query(params): Query<ConfigParams>,
    headers: HeaderMap,
) -> Result<Json<FrontendResponse>, Response> {
    Ok(Json(frontend_config(&store, &params, &headers)?))
}

async fn config_js(
    State(store): State<Arc<Store>>,
    Query(params): Query<ConfigParams>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let config = frontend_config(&store, &params, &headers)?;
    let javascript = format!(
        "var T={};{}",
        serde_json::to_string(&config).map_err(|_| not_found(&headers, &store.frontend))?,
        "(function(){var b=document.querySelector('time');if(b&&T.datetime){var a=new Date(1E3*T.datetime),d='January February March April May June July August September October November December'.split(' ')[a.getMonth()],c=a.getDate();b.innerText=d+' '+(10>c?'0':'')+c+', '+a.getFullYear()}})();"
    );
    let mut response = javascript.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/javascript; charset=utf-8"),
    );
    Ok(response)
}

async fn article(
    State(store): State<Arc<Store>>,
    Path(path): Path<String>,
    headers: HeaderMap,
) -> Result<Html<String>, Response> {
    let page = store
        .page_by_path(&path)
        .ok()
        .flatten()
        .ok_or_else(|| not_found(&headers, &store.frontend))?;
    let page = store
        .record_view(&page)
        .map_err(|_| not_found(&headers, &store.frontend))?;
    Ok(Html(render_article_with_headers(
        &page,
        &headers,
        &store.frontend,
    )))
}

fn not_found(headers: &HeaderMap, frontend: &crate::db::FrontendConfig) -> Response {
    (
        StatusCode::NOT_FOUND,
        Html(crate::i18n::render(
            &with_config(NOT_FOUND_HTML, frontend),
            headers,
        )),
    )
        .into_response()
}

pub async fn not_found_catcher(State(store): State<Arc<Store>>, headers: HeaderMap) -> Response {
    not_found(&headers, &store.frontend)
}

async fn file_response(
    path: impl AsRef<std::path::Path>,
    headers: &HeaderMap,
    frontend: &crate::db::FrontendConfig,
) -> Response {
    let path = path.as_ref();
    let Ok(bytes) = tokio::fs::read(path).await else {
        return not_found(headers, frontend);
    };
    let mime = mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string();
    let mut response = bytes.into_response();
    if let Ok(value) = HeaderValue::from_str(&mime) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    response
}

fn with_config(template: &str, frontend: &crate::db::FrontendConfig) -> String {
    template
        .lines()
        .filter(|line| !line.contains("https://t.me/_websync_"))
        .collect::<Vec<_>>()
        .join("\n")
        .replace("{{site_name}}", &escape_html(&frontend.name))
        .replace("{{description}}", &escape_html(&frontend.description))
}

fn with_config_javascript(template: &str, frontend: &crate::db::FrontendConfig) -> String {
    let name = serde_json::to_string(&frontend.name).expect("site name should serialize");
    template.replace("{{site_name}}", &name[1..name.len() - 1])
}

#[cfg(test)]
fn render_article(page: &crate::model::Page) -> String {
    render_article_with_headers(
        page,
        &HeaderMap::new(),
        &crate::db::FrontendConfig::default(),
    )
}

fn render_article_with_headers(
    page: &crate::model::Page,
    headers: &HeaderMap,
    frontend: &crate::db::FrontendConfig,
) -> String {
    let article_template = crate::i18n::render(ARTICLE_HTML, headers);
    let title = escape_html(&page.title);
    let description = escape_html(&page.description());
    let author = escape_html(&page.author_name);
    let author_url = crate::handlers::urls::is_safe_url(&page.author_url, false)
        .then(|| escape_attribute(&page.author_url));
    let created = DateTime::from_timestamp(page.created_at, 0).unwrap_or_else(Utc::now);
    let modified = DateTime::from_timestamp(page.updated_at, 0).unwrap_or(created);
    let created_iso = created.format("%Y-%m-%dT%H:%M:%S%z").to_string();
    let modified_iso = modified.format("%Y-%m-%dT%H:%M:%S%z").to_string();
    let date = created.format("%B %d, %Y").to_string();

    let mut html = article_template
        .replace("Sample Page", &title)
        .replace("Sample-Page-12-15", &page.path)
        .replace("45237495d34c04a7bf28d", &page.id)
        .replace("Hello, world&#33;", &description)
        .replace("2016-12-15T02:11:33+0000", &created_iso)
        .replace("2026-10-21T01:18:54+0000", &modified_iso)
        .replace("December 15, 2016", &date)
        .replace(
            "<meta property=\"article:author\" content=\"Anonymous\">",
            &format!("<meta property=\"article:author\" content=\"{author}\">"),
        );

    let header_author = if let Some(author_url) = &author_url {
        format!("<a rel=\"author\" href=\"{author_url}\">{author}</a>")
    } else {
        format!("<a rel=\"author\">{author}</a>")
    };
    html = html.replace("<a rel=\"author\">Anonymous</a>", &header_author);

    let body = render_article_content(page);
    if let Some(start) = html.find("<article id=\"_tl_editor\"") {
        if let Some(end) = html[start..].find("</article>") {
            html.replace_range(start..start + end + "</article>".len(), &body);
        }
    }

    with_config(&html, frontend)
}

fn render_article_content(page: &crate::model::Page) -> String {
    let title = escape_html(&page.title);
    let author = escape_html(&page.author_name);
    let author_url = crate::handlers::urls::is_safe_url(&page.author_url, false)
        .then(|| escape_attribute(&page.author_url));
    let body_author = if let Some(author_url) = &author_url {
        format!("<a href=\"{author_url}\">{author}</a>")
    } else {
        author
    };
    format!(
        "<article id=\"_tl_editor\" class=\"tl_article_content\"><h1>{title}<br></h1><address>{body_author}<br></address>{}</article>",
        nodes_to_html(&page.content)
    )
}

fn normalize_editor_content(nodes: Vec<Node>) -> Vec<Node> {
    nodes
        .into_iter()
        .flat_map(|node| match node {
            Node::Text(text) => vec![Node::Text(text)],
            Node::Element(mut element) => {
                let had_children = element.children.is_some();
                let children = element
                    .children
                    .take()
                    .map(normalize_editor_content)
                    .unwrap_or_default();
                if matches!(element.tag.as_str(), "div" | "span") {
                    children
                } else {
                    if had_children {
                        element.children = Some(children);
                    }
                    vec![Node::Element(element)]
                }
            }
        })
        .collect()
}

fn nodes_to_html(nodes: &[Node]) -> String {
    nodes_to_html_with_options(nodes, false)
}

fn nodes_to_html_with_options(nodes: &[Node], preserve_newlines: bool) -> String {
    nodes
        .iter()
        .map(|node| node_to_html(node, preserve_newlines))
        .collect()
}

fn node_to_html(node: &Node, preserve_newlines: bool) -> String {
    match node {
        Node::Text(text) => {
            let escaped = escape_html(text);
            if preserve_newlines {
                escaped
            } else {
                escaped
                    .replace("\r\n", "\n")
                    .replace('\r', "\n")
                    .replace('\n', "<br class=\"inline\">")
            }
        }
        Node::Element(element) => {
            const TAGS: &[&str] = crate::handlers::sanitize::CONTENT_TAGS;
            if !TAGS.contains(&element.tag.as_str()) {
                return element
                    .children
                    .as_deref()
                    .map(|children| nodes_to_html_with_options(children, preserve_newlines))
                    .unwrap_or_default();
            }
            let tag = element.tag.as_str();
            let attributes = element
                .attrs
                .as_ref()
                .map(|attrs| render_attrs(tag, attrs))
                .unwrap_or_default();
            if matches!(tag, "br" | "hr" | "img") {
                format!("<{tag}{attributes}>")
            } else {
                let children = element
                    .children
                    .as_deref()
                    .map(|children| {
                        nodes_to_html_with_options(children, preserve_newlines || tag == "pre")
                    })
                    .unwrap_or_default();
                format!("<{tag}{attributes}>{children}</{tag}>")
            }
        }
    }
}

fn render_attrs(tag: &str, attrs: &BTreeMap<String, String>) -> String {
    crate::handlers::sanitize::sanitize_attributes(tag, attrs)
        .iter()
        .map(|(name, value)| format!(" {name}=\"{}\"", escape_attribute(value)))
        .collect()
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn escape_attribute(value: &str) -> String {
    escape_html(value)
}

fn session_cookie(session: String) -> Cookie<'static> {
    Cookie::build(("tl_session", session))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .build()
}

fn existing_browser_account(store: &Store, cookies: &CookieJar) -> Result<Option<Account>, String> {
    let Some(token) = cookies.get("tl_session") else {
        return Ok(None);
    };
    store.account_by_session(token.value())
}

fn browser_account(store: &Store, cookies: &mut CookieJar) -> Result<Account, String> {
    if let Some(account) = existing_browser_account(store, cookies)? {
        return Ok(account);
    }
    let account = store.create_account("", "", "")?;
    let session = store.create_session(account.id)?;
    *cookies = std::mem::take(cookies).add(session_cookie(session));
    Ok(account)
}

#[derive(serde::Deserialize)]
struct CheckParams {
    page_id: Option<String>,
}

async fn check(
    State(store): State<Arc<Store>>,
    mut cookies: CookieJar,
    Form(params): Form<CheckParams>,
) -> (CookieJar, Json<Value>) {
    let result: Result<Value, String> = (|| {
        let existing = existing_browser_account(&store, &cookies)?;
        let page_id = params.page_id.as_deref().unwrap_or("0");
        if existing.is_none() && matches!(page_id, "" | "0") {
            return Ok(json!({
                "can_edit": true,
                "author_name": "",
                "author_url": "",
                "short_name": "",
            }));
        }
        if existing.is_none() && page_id != "0" && !page_id.is_empty() {
            store.page_by_id(page_id)?.ok_or("PAGE_NOT_FOUND")?;
            let mut result = json!({
                "can_edit": false,
                "author_name": "",
                "author_url": "",
                "short_name": "",
            });
            if store.frontend.report_enabled {
                result["report_hash"] = json!("local");
            }
            return Ok(result);
        }
        let account = browser_account(&store, &mut cookies)?;
        let mut result = json!({
            "can_edit": true,
            "author_name": account.author_name,
            "author_url": account.author_url,
            "short_name": account.short_name,
        });
        if page_id != "0" && !page_id.is_empty() {
            let page = store.page_by_id(page_id)?.ok_or("PAGE_NOT_FOUND")?;
            result["can_edit"] = json!(page.account_id == account.id);
            if page.account_id == account.id {
                result["save_hash"] = json!(page.save_hash);
            } else if store.frontend.report_enabled {
                result["report_hash"] = json!("local");
            }
        }
        Ok(result)
    })();
    let response = result.unwrap_or_else(|error| json!({"error": error}));
    (cookies, Json(response))
}

#[derive(Default)]
struct SaveForm {
    data: Option<String>,
    title: Option<String>,
    author: Option<String>,
    author_url: Option<String>,
    save_hash: Option<String>,
    page_id: Option<String>,
}

async fn read_save_form(multipart: &mut Multipart) -> Result<SaveForm, String> {
    let mut form = SaveForm::default();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| error.to_string())?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "Data" => {
                let bytes = field.bytes().await.map_err(|error| error.to_string())?;
                form.data = Some(String::from_utf8(bytes.to_vec()).map_err(|_| "CONTENT_INVALID")?);
            }
            "title" => form.title = Some(field.text().await.map_err(|error| error.to_string())?),
            "author" => form.author = Some(field.text().await.map_err(|error| error.to_string())?),
            "author_url" => {
                form.author_url = Some(field.text().await.map_err(|error| error.to_string())?)
            }
            "save_hash" => {
                form.save_hash = Some(field.text().await.map_err(|error| error.to_string())?)
            }
            "page_id" => {
                form.page_id = Some(field.text().await.map_err(|error| error.to_string())?)
            }
            _ => {}
        }
    }
    Ok(form)
}

async fn save(
    State(store): State<Arc<Store>>,
    mut cookies: CookieJar,
    mut multipart: Multipart,
) -> (CookieJar, Json<Value>) {
    let result: Result<Value, String> = async {
        let params = read_save_form(&mut multipart).await?;
        let title = params.title.unwrap_or_default();
        if title.is_empty() || title.chars().count() > store.validation.title_max_chars {
            return Err("TITLE_INVALID".to_string());
        }
        let content = normalize_editor_content(parse_editor_content(
            &params.data,
            store.validation.content_max_bytes(),
        )?);
        let content =
            crate::handlers::sanitize::SanitizedContent::new(&content, store.privacy.strip_utm);
        let author = params.author.as_deref().unwrap_or("");
        let author_url = params.author_url.as_deref().unwrap_or("");
        if author.chars().count() > store.validation.author_name_max_chars
            || author_url.chars().count() > store.validation.author_url_max_chars
        {
            return Err("AUTHOR_INVALID".into());
        }
        let page_id = params.page_id.as_deref().unwrap_or("0");
        if page_id != "0"
            && !page_id.is_empty()
            && existing_browser_account(&store, &cookies)?.is_none()
        {
            return Err("ACCESS_DENIED".into());
        }
        let account = browser_account(&store, &mut cookies)?;
        let page = match page_id {
            "" | "0" => store.create_page(&account, &title, author, author_url, content)?,
            page_id => {
                let page = store.page_by_id(page_id)?.ok_or("PAGE_NOT_FOUND")?;
                if page.account_id != account.id
                    || params.save_hash.as_deref() != Some(&page.save_hash)
                {
                    return Err("ACCESS_DENIED".into());
                }
                store.update_page(&page, &title, author, author_url, content)?
            }
        };
        Ok(json!({"path": page.path, "page_id": page.id}))
    }
    .await;
    let response = result.unwrap_or_else(|error| {
        log::error!("save failed: {error}");
        json!({"error": error})
    });
    (cookies, Json(response))
}

async fn upload(State(store): State<Arc<Store>>, mut multipart: Multipart) -> Response {
    if !store.frontend.upload_enabled {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "UPLOADS_DISABLED"})),
        )
            .into_response();
    }
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() != Some("file") {
            continue;
        }
        let content_type = field.content_type().unwrap_or("").to_string();
        let Ok(bytes) = field.bytes().await else {
            return Json(json!({"error": "UPLOAD_FAILED"})).into_response();
        };
        let media = match crate::handlers::media::inspect_media_with_options(
            &bytes,
            &content_type,
            store.privacy.strip_exif,
        ) {
            Ok(media) => media,
            Err(crate::handlers::media::MediaError::UnsupportedType) => {
                return Json(json!({"error": "FILE_TYPE_NOT_ALLOWED"})).into_response();
            }
            Err(crate::handlers::media::MediaError::InvalidData) => {
                return Json(json!({"error": "INVALID_MEDIA"})).into_response();
            }
            Err(crate::handlers::media::MediaError::MimeMismatch) => {
                return Json(json!({"error": "MIME_MISMATCH"})).into_response();
            }
        };
        let destination = store.upload_dir.join(&media.file_name);
        if !destination.exists() && tokio::fs::write(&destination, media.bytes).await.is_err() {
            return Json(json!({"error": "UPLOAD_FAILED"})).into_response();
        }
        return Json(json!([{"src": format!("/file/{}", media.file_name)}])).into_response();
    }
    Json(json!({"error": "UPLOAD_FAILED"})).into_response()
}

async fn uploaded_file(
    State(store): State<Arc<Store>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    if name.contains('/') || name.contains("..") {
        return not_found(&headers, &store.frontend);
    }
    let local = store.upload_dir.join(&name);
    if local.is_file() {
        file_response(local, &headers, &store.frontend).await
    } else {
        file_response(format!("static/file/{name}"), &headers, &store.frontend).await
    }
}

#[derive(serde::Deserialize)]
struct ReportParams {
    page_id: Option<String>,
    reason: Option<String>,
    reason_text: Option<String>,
}

async fn report(State(store): State<Arc<Store>>, Form(params): Form<ReportParams>) -> Response {
    if !store.frontend.report_enabled {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "REPORTING_DISABLED"})),
        )
            .into_response();
    }
    let result: Result<Value, String> = (|| {
        let page_id = params.page_id.as_deref().ok_or("PAGE_NOT_FOUND")?;
        let page = store.page_by_id(page_id)?.ok_or("PAGE_NOT_FOUND")?;
        let comment = params.reason_text.as_deref().unwrap_or("");
        let report = crate::handlers::sanitize::SanitizedReport::new(
            &page.id,
            params.reason.as_deref().unwrap_or("other"),
            comment,
        )?;
        store.create_report(&report)?;
        Ok(json!({"ok": true}))
    })();
    Json(result.unwrap_or_else(|error| json!({"error": error}))).into_response()
}

#[derive(serde::Deserialize)]
struct MigrateParams {
    migrate_hash: Option<String>,
}

async fn migrate(Form(params): Form<MigrateParams>) -> Json<Value> {
    let _ = &params.migrate_hash;
    Json(json!({"migrated_count": 0}))
}

async fn auth(
    State(store): State<Arc<Store>>,
    Path(token): Path<String>,
    mut cookies: CookieJar,
) -> Result<(CookieJar, Redirect), StatusCode> {
    let connection = store
        .conn()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let account = connection
        .query_row(
            "SELECT * FROM accounts WHERE auth_token = ?1",
            [token],
            Account::from_row,
        )
        .optional()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    if Utc::now().timestamp() - account.auth_created > 300 {
        return Err(StatusCode::NOT_FOUND);
    }
    connection
        .execute(
            "UPDATE accounts SET auth_token = NULL WHERE id = ?1",
            [account.id],
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    drop(connection);
    let session = store
        .create_session(account.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    cookies = cookies.add(session_cookie(session));
    Ok((cookies, Redirect::to("/")))
}

#[derive(serde::Deserialize)]
struct YouTubeEmbedParams {
    url: Option<String>,
}

async fn youtube_embed(
    State(store): State<Arc<Store>>,
    Query(params): Query<YouTubeEmbedParams>,
) -> Response {
    let Some(url) = params.url else {
        return (StatusCode::BAD_REQUEST, "Missing YouTube URL").into_response();
    };
    match crate::handlers::urls::youtube_embed_url(&url, store.privacy.strip_utm) {
        Some(url) => Redirect::to(&url).into_response(),
        None => (StatusCode::BAD_REQUEST, "Invalid YouTube URL").into_response(),
    }
}

pub fn routes() -> Router<Arc<Store>> {
    Router::new()
        .route("/", get(index))
        .route("/api", get(api_docs))
        .route("/embed/youtube", get(youtube_embed))
        .route("/favicon.ico", get(favicon))
        .route("/robots.txt", get(robots))
        .route("/config.json", get(config_json))
        .route("/js/config.js", get(config_js))
        .route("/js/core.min.js", get(core_js))
        .route("/file/{name}", get(uploaded_file))
        .route("/auth/{token}", get(auth))
        .merge(browser_api_routes())
        .route("/{path}", get(article))
}

pub fn browser_api_routes() -> Router<Arc<Store>> {
    Router::new()
        .route("/check", post(check))
        .route("/save", post(save))
        .route("/upload", post(upload))
        .route("/report", post(report))
        .route("/migrate", post(migrate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{app, form, get, multipart, request};
    use axum::http::{Method, StatusCode, header};

    fn test_page(content: Value) -> crate::model::Page {
        crate::model::Page {
            id: "page-id".into(),
            path: "Test-Page-01-01".into(),
            account_id: 1,
            title: "</title><script>alert(1)</script>".into(),
            author_name: "\"><img src=x onerror=alert(1)>".into(),
            author_url: "javascript:alert(1)".into(),
            content: serde_json::from_value(content).unwrap(),
            created_at: 0,
            updated_at: 0,
            views: 0,
            save_hash: "hash".into(),
            hidden: false,
        }
    }

    #[tokio::test]
    async fn hidden_pages_are_not_public_and_web_admin_is_removed() {
        let (directory, app) = app();
        let account = form(
            app.clone(),
            "/createAccount",
            &[("short_name", "Moderation")],
        )
        .await
        .json();
        let token = account["result"]["access_token"].as_str().unwrap();
        let page = form(
            app.clone(),
            "/createPage",
            &[
                ("access_token", token),
                ("title", "Visibility"),
                ("content", "[]"),
            ],
        )
        .await
        .json();
        let path = page["result"]["path"].as_str().unwrap();
        let page_uri = format!("/{path}");
        assert_eq!(get(app.clone(), &page_uri).await.status, StatusCode::OK);
        assert_eq!(
            get(app.clone(), "/admin").await.status,
            StatusCode::NOT_FOUND
        );

        let connection = rusqlite::Connection::open(directory.path().join("test.db")).unwrap();
        connection
            .execute("UPDATE pages SET hidden = 1 WHERE path = ?1", [path])
            .unwrap();
        assert_eq!(
            get(app.clone(), &page_uri).await.status,
            StatusCode::NOT_FOUND
        );
        let hidden_api = get(
            app.clone(),
            &format!("/api/getPage/{path}?return_content=true"),
        )
        .await
        .json();
        assert_eq!(hidden_api["error"], "PAGE_NOT_FOUND");

        connection
            .execute("UPDATE pages SET hidden = 0 WHERE path = ?1", [path])
            .unwrap();
        assert_eq!(get(app, &page_uri).await.status, StatusCode::OK);
    }

    #[tokio::test]
    async fn youtube_preview_redirects_to_a_canonical_https_embed() {
        let (_directory, app) = crate::test_utils::app_with_privacy(true);
        let redirected = get(
            app.clone(),
            "/embed/youtube?url=https%3A%2F%2Fyoutu.be%2FyTvS6_7yYR0%3Fsi%3DOi_HXK5clEezAHYa",
        )
        .await;
        assert_eq!(redirected.status, StatusCode::SEE_OTHER);
        assert_eq!(
            redirected.headers[header::LOCATION],
            "https://www.youtube.com/embed/yTvS6_7yYR0"
        );

        let rejected = get(
            app,
            "/embed/youtube?url=https%3A%2F%2Fevil.example%2Fwatch%3Fv%3DyTvS6_7yYR0",
        )
        .await;
        assert_eq!(rejected.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn rendering_sanitizes_text_links_and_attributes() {
        let page = test_page(json!([
            {
                "tag": "p",
                "attrs": {
                    "dir": "auto",
                    "class": "cursor_wrapper",
                    "onclick": "alert(1)",
                    "style": "display:none"
                },
                "children": ["<script>alert(1)</script>", {"tag":"br","attrs":{"class":"inline"}}]
            },
            {
                "tag": "a",
                "attrs": {
                    "href": "https://example.com/?a=1&b=2",
                    "target": "_blank",
                    "rel": "opener nofollow",
                    "ping": "https://evil.example"
                },
                "children": ["safe link"]
            },
            {"tag":"h3","attrs":{"id":"_publish_button"},"children":["internal id"]},
            {"tag":"h3","attrs":{"id":"section-1"},"children":["safe id"]},
            {"tag":"img","attrs":{"src":"data:text/html;base64,PHNjcmlwdD4=","onerror":"alert(1)"}},
            {"tag":"iframe","attrs":{"src":"http://example.com","srcdoc":"<script>alert(1)</script>"},"children":[]}
        ]));
        let html = render_article(&page);
        assert!(html.contains("&lt;/title&gt;&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("href=\"https://example.com/?a=1&amp;b=2\""));
        assert!(html.contains("rel=\"nofollow noopener noreferrer\""));
        assert!(html.contains("<br class=\"inline\">"));
        assert!(html.contains("id=\"section-1\""));
        assert!(html.contains("<img>"));
        assert!(html.contains("<iframe></iframe>"));
        for forbidden in [
            "javascript:",
            "data:text/html",
            "onclick=\"",
            "onerror=\"",
            "style=",
            "srcdoc=",
            "ping=",
            "rel=\"opener",
        ] {
            assert!(!html.contains(forbidden), "rendered unsafe {forbidden}");
        }
        let article = html
            .split("<article id=\"_tl_editor\"")
            .nth(1)
            .unwrap()
            .split("</article>")
            .next()
            .unwrap();
        assert!(!article.contains("id=\"_publish_button\""));
        assert!(!article.contains("class=\"cursor_wrapper\""));
    }

    #[tokio::test]
    async fn gzip_is_negotiated_for_text_but_not_images() {
        let (_directory, app) = app();
        let compressed = request(
            app.clone(),
            Method::GET,
            "/js/dom.modern.js",
            &[("accept-encoding", "gzip")],
            Vec::new(),
        )
        .await;
        assert_eq!(compressed.status, StatusCode::OK);
        assert_eq!(compressed.headers[header::CONTENT_ENCODING], "gzip");
        assert!(
            compressed.headers[header::VARY]
                .to_str()
                .unwrap()
                .to_ascii_lowercase()
                .contains("accept-encoding")
        );
        assert!(compressed.body.starts_with(&[0x1f, 0x8b]));
        assert!(compressed.body.len() < std::fs::read("static/js/dom.modern.js").unwrap().len());

        let html = request(
            app.clone(),
            Method::GET,
            "/",
            &[("accept-encoding", "gzip"), ("accept-language", "en")],
            Vec::new(),
        )
        .await;
        assert_eq!(html.headers[header::CONTENT_ENCODING], "gzip");
        let vary = html
            .headers
            .get_all(header::VARY)
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect::<Vec<_>>()
            .join(",");
        assert!(vary.contains("accept-encoding"));
        assert!(vary.contains("accept-language"));

        let identity = get(app.clone(), "/js/dom.modern.js").await;
        assert!(identity.headers.get(header::CONTENT_ENCODING).is_none());
        assert_eq!(
            identity.body,
            std::fs::read("static/js/dom.modern.js").unwrap()
        );

        let image = get(app, "/images/logo.png").await;
        assert!(image.headers.get(header::CONTENT_ENCODING).is_none());
        assert_eq!(image.body, std::fs::read("static/images/logo.png").unwrap());
    }

    #[test]
    fn editor_wrappers_are_removed_before_storage() {
        let nodes: Vec<Node> = serde_json::from_value(json!([{
            "tag": "figure",
            "children": [
                {"tag":"div","attrs":{"class":"figure_wrapper"},"children":[{"tag":"img","attrs":{"src":"/file/image.png"}}]},
                {"tag":"figcaption","children":["caption"]}
            ]
        }]))
        .unwrap();
        let normalized = normalize_editor_content(nodes);
        let figure = match &normalized[0] {
            Node::Element(element) => element,
            _ => panic!("expected figure"),
        };
        let children = figure.children.as_ref().unwrap();
        assert_eq!(children.len(), 2);
        assert!(matches!(&children[0], Node::Element(element) if element.tag == "img"));
        assert!(matches!(&children[1], Node::Element(element) if element.tag == "figcaption"));
    }

    #[tokio::test]
    async fn cookie_sessions_control_editing_and_reporting() {
        let (_directory, app) = app();
        let index = get(app.clone(), "/").await.text();
        assert!(index.contains("/js/dom.modern.js"));
        assert!(!index.contains("jquery"));

        let checked = form(app.clone(), "/api/check", &[("page_id", "0")]).await;
        assert!(checked.headers.get(header::SET_COOKIE).is_none());

        let content = json!([{"tag":"p","children":["owner"]}]).to_string();
        let (body, content_type) = multipart(
            &[
                ("title", "Cookie Page"),
                ("author", ""),
                ("author_url", ""),
                ("save_hash", ""),
                ("page_id", "0"),
            ],
            ("Data", "content.html", content.as_bytes(), "text/plain"),
        );
        let saved_response = request(
            app.clone(),
            Method::POST,
            "/api/save",
            &[("content-type", &content_type)],
            body,
        )
        .await;
        let cookie = saved_response
            .headers
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let set_cookie = saved_response
            .headers
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(set_cookie.contains("HttpOnly"));
        assert!(set_cookie.contains("SameSite=Lax"));
        let saved = saved_response.json();
        let path = saved["path"].as_str().unwrap().to_string();
        let page_id = saved["page_id"].as_str().unwrap().to_string();

        let owner_check = request(
            app.clone(),
            Method::POST,
            "/api/check",
            &[
                ("content-type", "application/x-www-form-urlencoded"),
                ("cookie", &cookie),
            ],
            format!("page_id={page_id}").into_bytes(),
        )
        .await
        .json();
        assert_eq!(owner_check["can_edit"], true);
        assert!(owner_check.get("report_hash").is_none());
        let save_hash = owner_check["save_hash"].as_str().unwrap().to_string();

        let visitor_check = form(app.clone(), "/api/check", &[("page_id", &page_id)])
            .await
            .json();
        assert_eq!(visitor_check["can_edit"], false);
        assert!(visitor_check.get("save_hash").is_none());
        assert!(visitor_check.get("report_hash").is_some());

        let updated = json!([{"tag":"p","children":["updated"]}]).to_string();
        let (body, content_type) = multipart(
            &[
                ("title", "Cookie Page"),
                ("author", ""),
                ("author_url", ""),
                ("save_hash", &save_hash),
                ("page_id", &page_id),
            ],
            ("Data", "content.html", updated.as_bytes(), "text/plain"),
        );
        let edited = request(
            app.clone(),
            Method::POST,
            "/api/save",
            &[("content-type", &content_type), ("cookie", &cookie)],
            body,
        )
        .await
        .json();
        assert_eq!(edited["path"], path);

        let (body, content_type) = multipart(
            &[
                ("title", "Unauthorized"),
                ("author", ""),
                ("author_url", ""),
                ("save_hash", "wrong"),
                ("page_id", &page_id),
            ],
            ("Data", "content.html", b"[]", "text/plain"),
        );
        let denied = request(
            app.clone(),
            Method::POST,
            "/api/save",
            &[("content-type", &content_type)],
            body,
        )
        .await
        .json();
        assert_eq!(denied["error"], "ACCESS_DENIED");
    }

    #[tokio::test]
    async fn anonymous_checks_and_failed_saves_do_not_create_accounts() {
        fn counts(directory: &tempfile::TempDir) -> (i64, i64) {
            let connection = rusqlite::Connection::open(directory.path().join("test.db")).unwrap();
            (
                connection
                    .query_row("SELECT COUNT(*) FROM accounts", [], |row| row.get(0))
                    .unwrap(),
                connection
                    .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
                    .unwrap(),
            )
        }

        let (directory, app) = app();
        let account = form(app.clone(), "/createAccount", &[("short_name", "Visitors")])
            .await
            .json();
        let token = account["result"]["access_token"].as_str().unwrap();
        let page = form(
            app.clone(),
            "/createPage",
            &[
                ("access_token", token),
                ("title", "Visitor Check"),
                ("content", "[]"),
            ],
        )
        .await
        .json();
        let path = page["result"]["path"].as_str().unwrap();
        let html = get(app.clone(), &format!("/{path}")).await.text();
        let page_id = html
            .split("page_id=")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .to_string();
        let baseline = counts(&directory);

        let checked = form(app.clone(), "/api/check", &[("page_id", &page_id)]).await;
        assert!(checked.headers.get(header::SET_COOKIE).is_none());
        assert_eq!(checked.json()["can_edit"], false);
        let baseline_after_check = counts(&directory);
        assert_eq!(baseline_after_check, baseline);

        let (body, content_type) = multipart(
            &[
                ("title", "Unauthorized"),
                ("author", ""),
                ("author_url", ""),
                ("save_hash", "wrong"),
                ("page_id", &page_id),
            ],
            ("Data", "content.html", b"[]", "text/plain"),
        );
        let denied = request(
            app.clone(),
            Method::POST,
            "/api/save",
            &[("content-type", &content_type)],
            body,
        )
        .await
        .json();
        assert_eq!(denied["error"], "ACCESS_DENIED");
        assert_eq!(counts(&directory), baseline);

        let (body, content_type) = multipart(
            &[("title", ""), ("page_id", "0")],
            ("Data", "content.html", b"[]", "text/plain"),
        );
        let invalid = request(
            app.clone(),
            Method::POST,
            "/api/save",
            &[("content-type", &content_type)],
            body,
        )
        .await
        .json();
        assert_eq!(invalid["error"], "TITLE_INVALID");
        assert_eq!(counts(&directory), baseline);

        let editor = form(app.clone(), "/api/check", &[("page_id", "0")]).await;
        assert!(editor.headers.get(header::SET_COOKIE).is_none());
        assert_eq!(counts(&directory), baseline);

        let (body, content_type) = multipart(
            &[
                ("title", "First Publish"),
                ("author", ""),
                ("author_url", ""),
                ("save_hash", ""),
                ("page_id", "0"),
            ],
            ("Data", "content.html", b"[]", "text/plain"),
        );
        let published = request(
            app.clone(),
            Method::POST,
            "/api/save",
            &[("content-type", &content_type)],
            body,
        )
        .await;
        assert!(published.headers.get(header::SET_COOKIE).is_some());
        assert!(published.json().get("path").is_some());
        assert_eq!(counts(&directory), (baseline.0 + 1, baseline.1 + 1));
    }

    #[tokio::test]
    async fn upload_and_traversal_security() {
        let (directory, app) = app();
        let (body, content_type) =
            multipart(&[], ("file", "x.svg", b"<svg></svg>", "image/svg+xml"));
        let rejected = request(
            app.clone(),
            Method::POST,
            "/upload",
            &[("content-type", &content_type)],
            body,
        )
        .await
        .json();
        assert_eq!(rejected["error"], "FILE_TYPE_NOT_ALLOWED");

        let (body, content_type) = multipart(
            &[],
            ("file", "x.png", b"<html>not an image</html>", "image/png"),
        );
        let rejected = request(
            app.clone(),
            Method::POST,
            "/upload",
            &[("content-type", &content_type)],
            body,
        )
        .await
        .json();
        assert_eq!(rejected["error"], "INVALID_MEDIA");

        let image = image::ImageBuffer::<image::Rgb<u8>, Vec<u8>>::new(1, 1);
        let mut png = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let (body, content_type) = multipart(&[], ("file", "x.png", &png, "image/png"));
        let uploaded = request(
            app.clone(),
            Method::POST,
            "/upload",
            &[("content-type", &content_type)],
            body,
        )
        .await
        .json();
        let src = uploaded[0]["src"].as_str().unwrap();
        let response = get(app.clone(), src).await;
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, png);
        assert!(
            response.headers[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("image/png")
        );
        assert_eq!(response.headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");

        for uri in [
            "/file/..%2FConfig.toml",
            "/file/%2e%2e%2fConfig.toml",
            "/file/..%5CConfig.toml",
        ] {
            let response = get(app.clone(), uri).await;
            assert_eq!(response.status, StatusCode::NOT_FOUND);
            assert_eq!(
                response.body,
                crate::i18n::render(
                    &with_config(NOT_FOUND_HTML, &crate::db::FrontendConfig::default()),
                    &HeaderMap::new()
                )
                .as_bytes()
            );
        }

        let upload_dir = directory.path().join("uploads");
        let before = std::fs::read_dir(&upload_dir).unwrap().count();
        let (body, content_type) = multipart(
            &[],
            (
                "file",
                "large.png",
                &vec![b'x'; 10 * 1024 * 1024 + 1024],
                "image/png",
            ),
        );
        let rejected = request(
            app.clone(),
            Method::POST,
            "/upload",
            &[("content-type", &content_type)],
            body,
        )
        .await
        .json();
        assert_eq!(rejected["error"], "UPLOAD_FAILED");
        assert_eq!(std::fs::read_dir(upload_dir).unwrap().count(), before);
    }
}

#[cfg(test)]
mod i18n_tests {
    use crate::test_utils::{app, form, request};
    use axum::http::{Method, StatusCode, header};

    #[tokio::test]
    async fn html_responses_use_browser_language_and_remove_placeholders() {
        let (_directory, app) = app();
        let response = request(
            app.clone(),
            Method::GET,
            "/",
            &[("accept-language", "fr,en;q=0.8")],
            Vec::new(),
        )
        .await;
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.headers[header::CONTENT_LANGUAGE], "en");
        assert!(
            response
                .headers
                .get_all(header::VARY)
                .iter()
                .any(|value| value.to_str().unwrap().contains("accept-language"))
        );
        let html = response.text();
        assert!(html.contains("Publish"));
        assert!(html.contains("data-placeholder=\"Title\""));
        assert!(html.contains("data-placeholder=\"Your name\""));
        assert!(html.contains("data-label=\"Author\""));
        assert!(html.contains("data-placeholder=\"Your story…\""));
        assert!(!html.contains("window.TELEGRAPH_STRINGS"));
        assert!(!html.contains("{{template_strings}}"));
        assert!(!html.contains("{{publish}}"));

        let russian = request(
            app.clone(),
            Method::GET,
            "/",
            &[("accept-language", "ru,en;q=0.5")],
            Vec::new(),
        )
        .await;
        assert_eq!(russian.headers[header::CONTENT_LANGUAGE], "ru");
        let html = russian.text();
        assert!(html.contains(">Отправить</button>"));
        assert!(html.contains("data-placeholder=\"Название\""));
        assert!(html.contains("data-placeholder=\"Ваше имя\""));
        assert!(html.contains("data-label=\"Автор\""));
        assert!(html.contains("data-placeholder=\"Ваша история…\""));
        assert!(!html.contains("window.TELEGRAPH_STRINGS"));

        let javascript = request(
            app.clone(),
            Method::GET,
            "/js/core.min.js",
            &[("accept-language", "ru")],
            Vec::new(),
        )
        .await
        .text();
        assert!(javascript.contains("Ваша история…"));
        assert!(javascript.contains("Название"));
        assert!(javascript.contains("Ваше имя"));
        assert!(javascript.contains("Автор"));
        assert!(!javascript.contains("Your story..."));
        assert!(!javascript.contains("Your name"));

        let account = form(
            app.clone(),
            "/createAccount",
            &[("short_name", "Templates")],
        )
        .await
        .json();
        let token = account["result"]["access_token"].as_str().unwrap();
        let content = "[{\"tag\":\"p\",\"children\":[\"{{publish}}\"]}]";
        let page = form(
            app.clone(),
            "/createPage",
            &[
                ("access_token", token),
                ("title", "Template Safety"),
                ("content", content),
            ],
        )
        .await
        .json();
        let path = page["result"]["path"].as_str().unwrap();
        let article = request(
            app.clone(),
            Method::GET,
            &format!("/{path}"),
            &[("accept-language", "fr")],
            Vec::new(),
        )
        .await
        .text();
        assert!(article.contains("<p>{{publish}}</p>"));
        assert!(!article.contains("{{template_strings}}"));

        let missing = request(
            app.clone(),
            Method::GET,
            "/missing",
            &[("accept-language", "fr")],
            Vec::new(),
        )
        .await;
        assert_eq!(missing.status, StatusCode::NOT_FOUND);
        assert_eq!(missing.headers[header::CONTENT_LANGUAGE], "en");
        let html = missing.text();
        assert!(html.contains("Page does not exist"));
        assert!(html.contains("Create new"));
        assert!(!html.contains("{{page_not_found}}"));
        assert!(!html.contains("{{create_new}}"));
    }
}

#[cfg(test)]
mod localized_javascript_tests {
    use crate::test_utils::{app, request};
    use axum::http::{Method, StatusCode, header};

    #[tokio::test]
    async fn report_confirmation_is_localized_in_core_javascript() {
        let (_directory, app) = app();
        let response = request(
            app,
            Method::GET,
            "/js/core.min.js",
            &[("accept-language", "ru,en;q=0.5")],
            Vec::new(),
        )
        .await;
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.headers[header::CONTENT_LANGUAGE], "ru");
        assert!(
            response.headers[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("application/javascript")
        );
        assert!(
            response
                .headers
                .get_all(header::VARY)
                .iter()
                .any(|value| value.to_str().unwrap().contains("accept-language"))
        );
        let javascript = response.text();
        assert!(javascript.contains("Жалоба отправлена"));
        assert!(javascript.contains("Модераторы рассмотрят жалобу. Спасибо!"));
        assert!(!javascript.contains("{{report_sent}}"));
        assert!(!javascript.contains("{{report_sent_body}}"));
        assert!(!javascript.contains("{{done}}"));
    }
}

#[cfg(test)]
mod report_comment_tests {
    use crate::test_utils::{app, form, get};

    #[tokio::test]
    async fn report_comments_are_limited_to_500_characters() {
        let (_directory, app) = app();
        let account = form(app.clone(), "/createAccount", &[("short_name", "Reports")])
            .await
            .json();
        let token = account["result"]["access_token"].as_str().unwrap();
        let page = form(
            app.clone(),
            "/createPage",
            &[
                ("access_token", token),
                ("title", "Reports"),
                ("content", "[]"),
            ],
        )
        .await
        .json();
        let path = page["result"]["path"].as_str().unwrap();
        let html = get(app.clone(), &format!("/{path}")).await.text();
        assert!(html.contains("maxlength=\"500\""));
        let page_id = html
            .split("page_id=")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .to_string();

        let accepted = form(
            app.clone(),
            "/api/report",
            &[
                ("page_id", &page_id),
                ("reason", "other"),
                ("reason_text", &"x".repeat(500)),
            ],
        )
        .await
        .json();
        assert_eq!(accepted["ok"], true);

        let rejected = form(
            app.clone(),
            "/api/report",
            &[
                ("page_id", &page_id),
                ("reason", "other"),
                ("reason_text", &"é".repeat(501)),
            ],
        )
        .await
        .json();
        assert_eq!(rejected["error"], "COMMENT_TOO_LONG");

        let connection = rusqlite::Connection::open(_directory.path().join("test.db")).unwrap();
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM reports", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}

#[cfg(test)]
mod line_break_tests {
    use super::*;

    #[test]
    fn inline_breaks_round_trip_through_json_and_html() {
        let nodes: Vec<Node> = serde_json::from_value(json!([
            {"tag":"p","children":["first line\nsecond line"]},
            {"tag":"pre","children":["first line\nsecond line"]}
        ]))
        .unwrap();
        let encoded = serde_json::to_string(&nodes).unwrap();
        assert!(encoded.contains("\\n"));
        let decoded: Vec<Node> = serde_json::from_str(&encoded).unwrap();
        let html = nodes_to_html(&decoded);
        assert!(html.contains("<p>first line<br class=\"inline\">second line</p>"));
        assert!(html.contains("<pre>first line\nsecond line</pre>"));
        assert!(!html.contains("<p>first line</p><p>second line</p>"));
    }
}

#[cfg(test)]
mod robots_tests {
    use crate::test_utils::{app, get};
    use axum::http::{StatusCode, header};

    #[tokio::test]
    async fn robots_txt_allows_only_the_homepage() {
        let (_directory, app) = app();
        let response = get(app, "/robots.txt").await;
        assert_eq!(response.status, StatusCode::OK);
        assert!(
            response.headers[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/plain")
        );
        assert_eq!(response.text(), "User-agent: *\nDisallow: /\nAllow: /$\n");
    }
}

#[cfg(test)]
mod identity_tests {
    use crate::test_utils::{app_with_identity, form, get};

    #[tokio::test]
    async fn configured_identity_is_used_in_html_and_editor_javascript() {
        let (_directory, app) = app_with_identity("Example Press", "A custom description");
        let index = get(app.clone(), "/").await.text();
        assert!(index.contains("<title>Example Press</title>"));
        assert!(index.contains("A custom description"));
        assert!(!index.contains("{{site_name}}"));
        assert!(!index.contains("{{description}}"));

        let not_found = get(app.clone(), "/not-a-real-page").await;
        assert_eq!(not_found.status, axum::http::StatusCode::NOT_FOUND);
        let not_found = not_found.text();
        assert!(not_found.contains("<title>404 – Example Press</title>"));
        assert!(!not_found.contains("{{site_name}}"));

        let docs = get(app.clone(), "/api").await.text();
        assert!(docs.contains("Example Press API"));
        assert!(docs.contains("A custom description"));

        let javascript = get(app.clone(), "/js/core.min.js").await.text();
        assert!(javascript.contains("Example Press page"));
        assert!(!javascript.contains("{{site_name}}"));

        let account = form(app.clone(), "/createAccount", &[("short_name", "Identity")])
            .await
            .json();
        let token = account["result"]["access_token"].as_str().unwrap();
        let page = form(
            app.clone(),
            "/createPage",
            &[
                ("access_token", token),
                ("title", "Custom Identity"),
                ("content", r#"[{"tag":"p","children":["Article summary"]}]"#),
            ],
        )
        .await
        .json();
        let path = page["result"]["path"].as_str().unwrap();
        let article = get(app, &format!("/{path}")).await.text();
        assert!(article.contains("Custom Identity – Example Press"));
        assert!(article.contains("content=\"Example Press\""));
        assert!(article.contains("content=\"Article summary\""));
        assert!(!article.contains("content=\"A custom description\""));
    }
}

#[cfg(test)]
mod frontend_config_tests {
    use crate::test_utils::{app, form, get};
    use axum::http::{StatusCode, header};

    #[tokio::test]
    async fn config_is_available_as_json_and_javascript() {
        let (_directory, app) = app();
        let index = get(app.clone(), "/").await.text();
        assert!(index.contains("<script src=\"/js/config.js\"></script>"));
        assert!(!index.contains("var T="));

        let config = get(app.clone(), "/config.json").await.json();
        assert_eq!(config["apiUrl"], "/api");
        assert_eq!(config["uploadEnabled"], true);
        assert_eq!(config["pageId"], 0);
        assert_eq!(config["editable"], true);
        assert_eq!(config["datetime"], 0);

        let javascript = get(app.clone(), "/js/config.js").await;
        assert_eq!(javascript.status, StatusCode::OK);
        assert!(
            javascript.headers[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("application/javascript")
        );
        let javascript = javascript.text();
        assert!(javascript.starts_with("var T="));
        assert!(javascript.contains("\"pageId\":0"));

        let account = form(app.clone(), "/createAccount", &[("short_name", "Config")])
            .await
            .json();
        let token = account["result"]["access_token"].as_str().unwrap();
        let page = form(
            app.clone(),
            "/createPage",
            &[
                ("access_token", token),
                ("title", "Config"),
                ("content", "[]"),
            ],
        )
        .await
        .json();
        let path = page["result"]["path"].as_str().unwrap();
        let html = get(app.clone(), &format!("/{path}")).await.text();
        let page_id = html
            .split("page_id=")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .to_string();
        let article_config = get(app.clone(), &format!("/config.json?page_id={page_id}"))
            .await
            .json();
        assert_eq!(article_config["pageId"], page_id);
        assert!(article_config["datetime"].as_i64().unwrap() > 0);
        let article_javascript = get(app.clone(), &format!("/js/config.js?page_id={page_id}"))
            .await
            .text();
        assert!(article_javascript.contains(&format!("\"pageId\":\"{page_id}\"")));

        let api_config = get(app.clone(), "/config.json?page_id=api").await.json();
        assert_eq!(api_config["editable"], false);
    }

    #[tokio::test]
    async fn migrate_is_available_under_the_browser_api_namespace() {
        let (_directory, app) = app();
        let migrated = form(app, "/api/migrate", &[("migrate_hash", "test")])
            .await
            .json();
        assert_eq!(migrated["migrated_count"], 0);
    }
}

#[cfg(test)]
mod upload_config_tests {
    use crate::test_utils::{app_with_uploads, get, multipart, request};
    use axum::http::{Method, StatusCode};

    #[tokio::test]
    async fn disabled_uploads_return_forbidden_without_writing_files() {
        let (directory, app) = app_with_uploads(false);
        let (body, content_type) = multipart(&[], ("file", "x.png", b"image", "image/png"));
        let response = request(
            app.clone(),
            Method::POST,
            "/upload",
            &[("content-type", &content_type)],
            body,
        )
        .await;
        assert_eq!(response.status, StatusCode::FORBIDDEN);
        assert_eq!(response.json()["error"], "UPLOADS_DISABLED");
        assert_eq!(
            std::fs::read_dir(directory.path().join("uploads"))
                .unwrap()
                .count(),
            0
        );

        let config = get(app, "/config.json").await.json();
        assert_eq!(config["uploadEnabled"], false);
    }
}

#[cfg(test)]
mod deduplicated_upload_tests {
    use crate::test_utils::{app, multipart, request};
    use axum::http::Method;

    #[tokio::test]
    async fn identical_uploads_share_a_file_and_bad_media_is_rejected() {
        let (directory, app) = app();
        let image = image::ImageBuffer::<image::Rgb<u8>, Vec<u8>>::new(1, 1);
        let mut png = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let mut sources = Vec::new();
        for _ in 0..2 {
            let (body, content_type) = multipart(&[], ("file", "x.png", &png, "image/png"));
            let response = request(
                app.clone(),
                Method::POST,
                "/api/upload",
                &[("content-type", &content_type)],
                body,
            )
            .await
            .json();
            sources.push(response[0]["src"].as_str().unwrap().to_string());
        }
        assert_eq!(sources[0], sources[1]);
        assert_eq!(
            std::fs::read_dir(directory.path().join("uploads"))
                .unwrap()
                .count(),
            1
        );

        let (body, content_type) = multipart(&[], ("file", "x.png", b"%PDF-1.4\n", "image/png"));
        let rejected = request(
            app.clone(),
            Method::POST,
            "/api/upload",
            &[("content-type", &content_type)],
            body,
        )
        .await
        .json();
        assert_eq!(rejected["error"], "INVALID_MEDIA");

        let mut jpeg = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut jpeg),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        let (body, content_type) = multipart(&[], ("file", "x.png", &jpeg, "image/png"));
        let rejected = request(
            app,
            Method::POST,
            "/api/upload",
            &[("content-type", &content_type)],
            body,
        )
        .await
        .json();
        assert_eq!(rejected["error"], "MIME_MISMATCH");
        assert_eq!(
            std::fs::read_dir(directory.path().join("uploads"))
                .unwrap()
                .count(),
            1
        );
    }
}

#[cfg(test)]
mod editor_api_contract_tests {
    use crate::test_utils::{app, form, get, multipart, request};
    use axum::http::{Method, StatusCode};
    use serde_json::json;

    async fn publish_page(app: &axum::Router) -> String {
        let content = json!([{"tag":"p","children":["contract"]}]).to_string();
        let (body, content_type) = multipart(
            &[
                ("title", "API Contract"),
                ("author", ""),
                ("author_url", ""),
                ("save_hash", ""),
                ("page_id", "0"),
            ],
            ("Data", "content.html", content.as_bytes(), "text/plain"),
        );
        let saved = request(
            app.clone(),
            Method::POST,
            "/api/save",
            &[("content-type", &content_type)],
            body,
        )
        .await;
        assert_eq!(saved.status, StatusCode::OK);
        let saved = saved.json();
        let path = saved["path"].as_str().expect("published page path");
        let article = get(app.clone(), &format!("/{path}")).await;
        assert_eq!(article.status, StatusCode::OK);
        article
            .text()
            .split("page_id=")
            .nth(1)
            .expect("page ID in article configuration")
            .split('"')
            .next()
            .expect("nonempty page ID")
            .to_string()
    }

    #[tokio::test]
    async fn config_matches_frontend_contract() {
        let (_directory, app) = app();
        let response = get(app.clone(), "/config.json").await;
        assert_eq!(response.status, StatusCode::OK);
        let config = response.json();
        assert_eq!(config["apiUrl"], "/api");
        assert_eq!(config["uploadEnabled"], true);
        assert_eq!(config["pageId"], 0);
        assert_eq!(config["editable"], true);
        assert_eq!(config["reportEnabled"], true);
        assert_eq!(config["datetime"], 0);

        let javascript = get(app.clone(), "/js/config.js").await.text();
        assert!(javascript.starts_with("var T="));
        assert!(javascript.contains(r#""apiUrl":"/api""#));
        assert!(javascript.contains(r#""pageId":0"#));

        let page_id = publish_page(&app).await;
        let response = get(app.clone(), &format!("/config.json?page_id={page_id}")).await;
        assert_eq!(response.status, StatusCode::OK);
        let config = response.json();
        assert_eq!(config["pageId"], page_id);
        assert!(config["datetime"].as_i64().expect("page timestamp") > 0);

        let javascript = get(app, &format!("/js/config.js?page_id={page_id}"))
            .await
            .text();
        assert!(javascript.starts_with("var T="));
        assert!(javascript.contains(&format!(r#""pageId":"{page_id}""#)));
    }

    #[tokio::test]
    async fn reporting_and_migration_follow_moderation_contract() {
        let (directory, app) = app();
        let page_id = publish_page(&app).await;

        let checked = form(app.clone(), "/api/check", &[("page_id", &page_id)])
            .await
            .json();
        assert_eq!(checked["can_edit"], false);
        assert!(checked.get("save_hash").is_none());
        assert_eq!(checked["report_hash"], "local");

        let reported = form(
            app.clone(),
            "/api/report",
            &[
                ("page_id", &page_id),
                ("reason", "other"),
                ("reason_text", "bad\0<script>"),
            ],
        )
        .await;
        assert_eq!(reported.status, StatusCode::OK);
        assert_eq!(reported.json()["ok"], true);

        let connection = rusqlite::Connection::open(directory.path().join("test.db")).unwrap();
        let (reason, comment): (String, String) = connection
            .query_row(
                "SELECT reason, comment FROM reports WHERE page_id = ?1",
                [&page_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(reason, "other");
        assert_eq!(comment, "bad<script>");

        let invalid = form(
            app.clone(),
            "/api/report",
            &[
                ("page_id", &page_id),
                ("reason", "invalid"),
                ("reason_text", "comment"),
            ],
        )
        .await
        .json();
        assert_eq!(invalid["error"], "REPORT_INVALID");

        let too_long = "x".repeat(501);
        let rejected = form(
            app.clone(),
            "/api/report",
            &[
                ("page_id", &page_id),
                ("reason", "other"),
                ("reason_text", &too_long),
            ],
        )
        .await
        .json();
        assert_eq!(rejected["error"], "COMMENT_TOO_LONG");

        let migrated = form(app, "/api/migrate", &[("migrate_hash", "local")]).await;
        assert_eq!(migrated.status, StatusCode::OK);
        assert_eq!(migrated.json()["migrated_count"], 0);
    }
}

#[cfg(test)]
mod icon_tests {
    use crate::test_utils::{app, get};
    use axum::http::{StatusCode, header};

    #[tokio::test]
    async fn individual_svg_icons_replace_the_sprite_sheet() {
        let css = include_str!("../static/css/core.min.css");
        let javascript = include_str!("../static/js/core.min.js");
        assert!(css.contains("background-position:center"));
        assert!(css.contains("background-position:center 7.25px"));
        assert!(css.contains("color:#fff"));
        assert!(css.contains("button.active{background-color:#eaeaea;color:#000}"));
        assert!(!css.contains("filter:brightness(0)"));
        for sprite in ["icons.png", "icons_2x.png"] {
            assert!(!css.contains(sprite));
            assert!(!javascript.contains(sprite));
            assert!(!std::path::Path::new("static/images").join(sprite).exists());
        }
        let (_directory, app) = app();
        for file in [
            "bold.svg",
            "italic.svg",
            "link.svg",
            "text-large.svg",
            "text-small.svg",
            "quote-open.svg",
            "quote-pair.svg",
            "camera.svg",
            "code.svg",
            "bold-black.svg",
            "italic-black.svg",
            "link-black.svg",
            "text-large-black.svg",
            "text-small-black.svg",
            "quote-open-black.svg",
            "quote-pair-black.svg",
        ] {
            assert!(
                css.contains(&format!("/images/icons/{file}")),
                "missing {file}"
            );
            let response = get(app.clone(), &format!("/images/icons/{file}")).await;
            assert_eq!(response.status, StatusCode::OK, "{file}");
            assert!(
                response.headers[header::CONTENT_TYPE]
                    .to_str()
                    .unwrap()
                    .starts_with("image/svg+xml")
            );
            assert!(response.body.starts_with(b"<svg"));
            let svg = String::from_utf8(response.body).unwrap();
            if matches!(file, "camera.svg" | "code.svg") {
                assert!(svg.contains("fill=\"currentColor\"") || svg.contains("fill=\"#000\""));
            } else if file.ends_with("-black.svg") {
                assert!(svg.contains("fill=\"#000\""), "{file} should be black");
            } else {
                assert!(svg.contains("fill=\"#fff\""), "{file} should be white");
            }
        }
    }
}

#[cfg(test)]
mod report_config_tests {
    use crate::test_utils::{app_with_reporting, form, get};
    use axum::http::StatusCode;

    #[tokio::test]
    async fn disabled_reporting_hides_banner_and_rejects_reports() {
        let (_directory, app) = app_with_reporting(false);
        let account = form(app.clone(), "/createAccount", &[("short_name", "Reports")])
            .await
            .json();
        let token = account["result"]["access_token"].as_str().unwrap();
        let page = form(
            app.clone(),
            "/createPage",
            &[
                ("access_token", token),
                ("title", "Reports"),
                ("content", "[]"),
            ],
        )
        .await
        .json();
        let path = page["result"]["path"].as_str().unwrap();
        let html = get(app.clone(), &format!("/{path}")).await.text();
        let page_id = html
            .split("page_id=")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .to_string();

        let checked = form(app.clone(), "/check", &[("page_id", &page_id)])
            .await
            .json();
        assert!(checked.get("report_hash").is_none());
        assert_eq!(checked["can_edit"], false);

        let response = form(
            app.clone(),
            "/report",
            &[
                ("page_id", &page_id),
                ("reason", "other"),
                ("reason_text", "test"),
            ],
        )
        .await;
        assert_eq!(response.status, StatusCode::FORBIDDEN);
        assert_eq!(response.json()["error"], "REPORTING_DISABLED");

        let config = get(app, "/config.json").await.json();
        assert_eq!(config["reportEnabled"], false);
    }

    #[tokio::test]
    async fn enabled_reporting_exposes_banner_for_visitors() {
        let (_directory, app) = app_with_reporting(true);
        let account = form(app.clone(), "/createAccount", &[("short_name", "Reports")])
            .await
            .json();
        let token = account["result"]["access_token"].as_str().unwrap();
        let page = form(
            app.clone(),
            "/createPage",
            &[
                ("access_token", token),
                ("title", "Reports"),
                ("content", "[]"),
            ],
        )
        .await
        .json();
        let path = page["result"]["path"].as_str().unwrap();
        let html = get(app.clone(), &format!("/{path}")).await.text();
        let page_id = html
            .split("page_id=")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .to_string();
        let checked = form(app, "/check", &[("page_id", &page_id)]).await.json();
        assert!(checked.get("report_hash").is_some());
        assert_eq!(checked["can_edit"], false);
    }
}
