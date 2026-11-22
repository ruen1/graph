use crate::db::Store;
use crate::model::{Account, Node, Page, random_hex};
use axum::extract::{Form, Path, Query, State};
use axum::{Json, Router, routing::get};
use chrono::Utc;
use rusqlite::params;
use serde_json::{Map, Value, json};
use std::sync::Arc;

const TAGS: &[&str] = crate::handlers::sanitize::CONTENT_TAGS;

type ApiResult = Result<Value, String>;

fn respond(result: ApiResult) -> Json<Value> {
    match result {
        Ok(result) => Json(json!({"ok": true, "result": result})),
        Err(error) => Json(json!({"ok": false, "error": error})),
    }
}

macro_rules! api_method {
    ($get_name:ident, $post_name:ident, $params:ty, $handler:ident) => {
        async fn $get_name(
            State(store): State<Arc<Store>>,
            Query(params): Query<$params>,
        ) -> Json<Value> {
            respond($handler(&store, &params))
        }

        async fn $post_name(
            State(store): State<Arc<Store>>,
            Form(params): Form<$params>,
        ) -> Json<Value> {
            respond($handler(&store, &params))
        }
    };
}

macro_rules! api_path_method {
    ($get_name:ident, $post_name:ident, $params:ty, $handler:ident) => {
        async fn $get_name(
            State(store): State<Arc<Store>>,
            Path(path): Path<String>,
            Query(params): Query<$params>,
        ) -> Json<Value> {
            respond($handler(&store, Some(&path), &params))
        }

        async fn $post_name(
            State(store): State<Arc<Store>>,
            Path(path): Path<String>,
            Form(params): Form<$params>,
        ) -> Json<Value> {
            respond($handler(&store, Some(&path), &params))
        }
    };
}

fn required<'a>(value: &'a Option<String>, name: &str) -> Result<&'a str, String> {
    value.as_deref().ok_or_else(|| format!("{name}_REQUIRED"))
}

fn validate_length(value: &str, name: &str, min: usize, max: usize) -> Result<(), String> {
    let length = value.chars().count();
    if length < min {
        Err(format!("{name}_REQUIRED"))
    } else if length > max {
        Err(format!("{name}_TOO_LONG"))
    } else {
        Ok(())
    }
}

pub(crate) fn parse_content(value: &Option<String>, max_bytes: usize) -> Result<Vec<Node>, String> {
    parse_content_inner(value, false, max_bytes)
}

pub(crate) fn parse_editor_content(
    value: &Option<String>,
    max_bytes: usize,
) -> Result<Vec<Node>, String> {
    parse_content_inner(value, true, max_bytes)
}

fn parse_content_inner(
    value: &Option<String>,
    editor: bool,
    max_bytes: usize,
) -> Result<Vec<Node>, String> {
    let value = required(value, "CONTENT")?;
    if value.len() > max_bytes {
        return Err("CONTENT_TOO_BIG".into());
    }
    let nodes: Vec<Node> = serde_json::from_str(value).map_err(|error| {
        log::error!("invalid content JSON: {error}");
        "CONTENT_INVALID"
    })?;
    validate_nodes(&nodes, editor)?;
    Ok(nodes)
}

fn validate_nodes(nodes: &[Node], editor: bool) -> Result<(), String> {
    for node in nodes {
        if let Node::Element(element) = node {
            let editor_wrapper = editor && matches!(element.tag.as_str(), "div" | "span");
            if !TAGS.contains(&element.tag.as_str()) && !editor_wrapper {
                log::error!("unsupported content tag: {}", element.tag);
                return Err("CONTENT_INVALID".into());
            }
            if let Some(children) = &element.children {
                validate_nodes(children, editor)?;
            }
        }
    }
    Ok(())
}

fn boolean(value: Option<&String>) -> bool {
    value.is_some_and(|value| matches!(value.as_str(), "true" | "1"))
}

fn account_result(account: &Account, base_url: &str, with_credentials: bool) -> Value {
    let value = account.to_json(base_url);
    let mut object = value.as_object().cloned().expect("account is an object");
    if !with_credentials {
        object.remove("access_token");
        object.remove("auth_url");
    }
    Value::Object(object)
}

#[derive(serde::Deserialize)]
pub struct CreateAccountParams {
    short_name: Option<String>,
    author_name: Option<String>,
    author_url: Option<String>,
}

fn create_account(store: &Store, form: &CreateAccountParams) -> ApiResult {
    let short_name = required(&form.short_name, "SHORT_NAME")?;
    validate_length(short_name, "SHORT_NAME", 1, 32)?;
    let author_name = form.author_name.as_deref().unwrap_or("");
    let author_url = form.author_url.as_deref().unwrap_or("");
    validate_length(
        author_name,
        "AUTHOR_NAME",
        0,
        store.validation.author_name_max_chars,
    )?;
    validate_length(
        author_url,
        "AUTHOR_URL",
        0,
        store.validation.author_url_max_chars,
    )?;
    let account = store.create_account(short_name, author_name, author_url)?;
    Ok(account_result(&account, &store.base_url, true))
}

api_method!(
    create_account_get,
    create_account_post,
    CreateAccountParams,
    create_account
);

#[derive(serde::Deserialize)]
pub struct EditAccountParams {
    access_token: Option<String>,
    short_name: Option<String>,
    author_name: Option<String>,
    author_url: Option<String>,
}

fn edit_account(store: &Store, form: &EditAccountParams) -> ApiResult {
    let token = required(&form.access_token, "ACCESS_TOKEN")?;
    let account = store
        .account_by_token(token)?
        .ok_or("ACCESS_TOKEN_INVALID")?;
    if let Some(short_name) = &form.short_name {
        validate_length(short_name, "SHORT_NAME", 1, 32)?;
    }
    if let Some(author_name) = &form.author_name {
        validate_length(
            author_name,
            "AUTHOR_NAME",
            0,
            store.validation.author_name_max_chars,
        )?;
    }
    if let Some(author_url) = &form.author_url {
        validate_length(
            author_url,
            "AUTHOR_URL",
            0,
            store.validation.author_url_max_chars,
        )?;
    }
    let account = store.update_account(
        &account,
        form.short_name.as_deref(),
        form.author_name.as_deref(),
        form.author_url.as_deref(),
    )?;
    Ok(account_result(&account, &store.base_url, false))
}

api_method!(
    edit_account_get,
    edit_account_post,
    EditAccountParams,
    edit_account
);

#[derive(serde::Deserialize)]
pub struct GetAccountParams {
    access_token: Option<String>,
    fields: Option<String>,
}

fn get_account(store: &Store, form: &GetAccountParams) -> ApiResult {
    let token = required(&form.access_token, "ACCESS_TOKEN")?;
    let account = store
        .account_by_token(token)?
        .ok_or("ACCESS_TOKEN_INVALID")?;
    let fields = match &form.fields {
        Some(fields) => {
            serde_json::from_str::<Vec<String>>(fields).map_err(|_| "FIELDS_INVALID")?
        }
        None => ["short_name", "author_name", "author_url"]
            .map(str::to_string)
            .to_vec(),
    };
    let mut result = Map::new();
    for field in fields {
        match field.as_str() {
            "short_name" => result.insert(field, json!(account.short_name)),
            "author_name" => result.insert(field, json!(account.author_name)),
            "author_url" => result.insert(field, json!(account.author_url)),
            "auth_url" => result.insert(
                field,
                account
                    .auth_token
                    .as_ref()
                    .map(|token| json!(format!("{}/auth/{token}", store.base_url)))
                    .unwrap_or(Value::Null),
            ),
            "page_count" => {
                let count: i64 = store
                    .conn()?
                    .query_row(
                        "SELECT COUNT(*) FROM pages WHERE account_id = ?1",
                        [account.id],
                        |row| row.get(0),
                    )
                    .map_err(|_| "DATABASE_ERROR")?;
                result.insert(field, json!(count))
            }
            _ => return Err("FIELDS_INVALID".into()),
        };
    }
    Ok(Value::Object(result))
}

api_method!(
    get_account_get,
    get_account_post,
    GetAccountParams,
    get_account
);

#[derive(serde::Deserialize)]
pub struct AccessTokenParams {
    access_token: Option<String>,
}

fn revoke_token(store: &Store, form: &AccessTokenParams) -> ApiResult {
    let token = required(&form.access_token, "ACCESS_TOKEN")?;
    let account = store
        .account_by_token(token)?
        .ok_or("ACCESS_TOKEN_INVALID")?;
    let new_token = random_hex(32);
    let auth_token = random_hex(16);
    let connection = store.conn()?;
    connection
        .execute(
            "UPDATE accounts SET access_token = ?2, auth_token = ?3, auth_created = ?4 WHERE id = ?1",
            params![account.id, new_token, auth_token, Utc::now().timestamp()],
        )
        .map_err(|_| "DATABASE_ERROR")?;
    connection
        .execute("DELETE FROM sessions WHERE account_id = ?1", [account.id])
        .map_err(|_| "DATABASE_ERROR")?;
    drop(connection);
    let account = store
        .account_by_token(&new_token)?
        .ok_or("ACCESS_TOKEN_INVALID")?;
    Ok(account_result(&account, &store.base_url, true))
}

api_method!(
    revoke_token_get,
    revoke_token_post,
    AccessTokenParams,
    revoke_token
);

#[derive(serde::Deserialize)]
pub struct CreatePageParams {
    access_token: Option<String>,
    title: Option<String>,
    author_name: Option<String>,
    author_url: Option<String>,
    content: Option<String>,
    return_content: Option<String>,
}

fn create_page(store: &Store, form: &CreatePageParams) -> ApiResult {
    let token = required(&form.access_token, "ACCESS_TOKEN")?;
    let account = store
        .account_by_token(token)?
        .ok_or("ACCESS_TOKEN_INVALID")?;
    let title = required(&form.title, "TITLE")?;
    validate_length(title, "TITLE", 1, store.validation.title_max_chars)?;
    let author_name = form.author_name.as_deref().unwrap_or(&account.author_name);
    let author_url = form.author_url.as_deref().unwrap_or(&account.author_url);
    validate_length(
        author_name,
        "AUTHOR_NAME",
        0,
        store.validation.author_name_max_chars,
    )?;
    validate_length(
        author_url,
        "AUTHOR_URL",
        0,
        store.validation.author_url_max_chars,
    )?;
    let content = parse_content(&form.content, store.validation.content_max_bytes())?;
    let page = store.create_page(
        &account,
        title,
        author_name,
        author_url,
        crate::handlers::sanitize::SanitizedContent::new(&content, store.privacy.strip_utm),
    )?;
    Ok(page.to_json(&store.base_url, boolean(form.return_content.as_ref()), None))
}

api_method!(
    create_page_get,
    create_page_post,
    CreatePageParams,
    create_page
);

#[derive(serde::Deserialize)]
pub struct EditPageParams {
    access_token: Option<String>,
    path: Option<String>,
    title: Option<String>,
    author_name: Option<String>,
    author_url: Option<String>,
    content: Option<String>,
    return_content: Option<String>,
}

fn edit_page(store: &Store, path: Option<&str>, form: &EditPageParams) -> ApiResult {
    let token = required(&form.access_token, "ACCESS_TOKEN")?;
    let account = store
        .account_by_token(token)?
        .ok_or("ACCESS_TOKEN_INVALID")?;
    let path = path.or(form.path.as_deref()).ok_or("PATH_REQUIRED")?;
    let page = store.page_by_path(path)?.ok_or("PAGE_NOT_FOUND")?;
    if page.account_id != account.id {
        return Err("ACCESS_DENIED".into());
    }
    let title = required(&form.title, "TITLE")?;
    validate_length(title, "TITLE", 1, store.validation.title_max_chars)?;
    let author_name = form.author_name.as_deref().unwrap_or(&page.author_name);
    let author_url = form.author_url.as_deref().unwrap_or(&page.author_url);
    validate_length(
        author_name,
        "AUTHOR_NAME",
        0,
        store.validation.author_name_max_chars,
    )?;
    validate_length(
        author_url,
        "AUTHOR_URL",
        0,
        store.validation.author_url_max_chars,
    )?;
    let content = parse_content(&form.content, store.validation.content_max_bytes())?;
    let page = store.update_page(
        &page,
        title,
        author_name,
        author_url,
        crate::handlers::sanitize::SanitizedContent::new(&content, store.privacy.strip_utm),
    )?;
    Ok(page.to_json(
        &store.base_url,
        boolean(form.return_content.as_ref()),
        Some(true),
    ))
}

async fn edit_page_get(
    State(store): State<Arc<Store>>,
    Query(params): Query<EditPageParams>,
) -> Json<Value> {
    respond(edit_page(&store, None, &params))
}

async fn edit_page_post(
    State(store): State<Arc<Store>>,
    Form(params): Form<EditPageParams>,
) -> Json<Value> {
    respond(edit_page(&store, None, &params))
}

api_path_method!(
    edit_page_path_get,
    edit_page_path_post,
    EditPageParams,
    edit_page
);

#[derive(serde::Deserialize)]
pub struct GetPageParams {
    path: Option<String>,
    return_content: Option<String>,
    access_token: Option<String>,
}

fn get_page(store: &Store, path: Option<&str>, form: &GetPageParams) -> ApiResult {
    let path = path.or(form.path.as_deref()).ok_or("PATH_REQUIRED")?;
    let page = store.page_by_path(path)?.ok_or("PAGE_NOT_FOUND")?;
    let can_edit = match &form.access_token {
        Some(token) => Some(
            store
                .account_by_token(token)?
                .ok_or("ACCESS_TOKEN_INVALID")?
                .id
                == page.account_id,
        ),
        None => None,
    };
    Ok(page.to_json(
        &store.base_url,
        boolean(form.return_content.as_ref()),
        can_edit,
    ))
}

async fn get_page_get(
    State(store): State<Arc<Store>>,
    Query(params): Query<GetPageParams>,
) -> Json<Value> {
    respond(get_page(&store, None, &params))
}

async fn get_page_post(
    State(store): State<Arc<Store>>,
    Form(params): Form<GetPageParams>,
) -> Json<Value> {
    respond(get_page(&store, None, &params))
}

api_path_method!(
    get_page_path_get,
    get_page_path_post,
    GetPageParams,
    get_page
);

#[derive(serde::Deserialize)]
pub struct PageListParams {
    access_token: Option<String>,
    offset: Option<i64>,
    limit: Option<i64>,
}

fn page_list(store: &Store, form: &PageListParams) -> ApiResult {
    let token = required(&form.access_token, "ACCESS_TOKEN")?;
    let account = store
        .account_by_token(token)?
        .ok_or("ACCESS_TOKEN_INVALID")?;
    let offset = form.offset.unwrap_or(0).max(0);
    let limit = form.limit.unwrap_or(50);
    if !(0..=200).contains(&limit) {
        return Err("LIMIT_INVALID".into());
    }
    let connection = store.conn()?;
    let total_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pages WHERE account_id = ?1",
            [account.id],
            |row| row.get(0),
        )
        .map_err(|_| "DATABASE_ERROR")?;
    let mut statement = connection
        .prepare("SELECT * FROM pages WHERE account_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT ?2 OFFSET ?3")
        .map_err(|_| "DATABASE_ERROR")?;
    let pages = statement
        .query_map(params![account.id, limit, offset], Page::from_row)
        .map_err(|_| "DATABASE_ERROR")?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "DATABASE_ERROR")?;
    let pages = pages
        .iter()
        .map(|page| page.to_json(&store.base_url, false, Some(true)))
        .collect::<Vec<_>>();
    Ok(json!({"total_count": total_count, "pages": pages}))
}

api_method!(page_list_get, page_list_post, PageListParams, page_list);

#[derive(serde::Deserialize)]
pub struct ViewsParams {
    path: Option<String>,
    year: Option<i64>,
    month: Option<i64>,
    day: Option<i64>,
    hour: Option<i64>,
}

fn views(store: &Store, path: Option<&str>, form: &ViewsParams) -> ApiResult {
    let path = path.or(form.path.as_deref()).ok_or("PATH_REQUIRED")?;
    let page = store.page_by_path(path)?.ok_or("PAGE_NOT_FOUND")?;
    if form.month.is_some() && form.year.is_none()
        || form.day.is_some() && form.month.is_none()
        || form.hour.is_some() && form.day.is_none()
    {
        return Err("DATE_INVALID".into());
    }
    if form.year.is_some_and(|year| !(2000..=2100).contains(&year))
        || form.month.is_some_and(|month| !(1..=12).contains(&month))
        || form.day.is_some_and(|day| !(1..=31).contains(&day))
        || form.hour.is_some_and(|hour| !(0..=24).contains(&hour))
    {
        return Err("DATE_INVALID".into());
    }
    if form.year.is_none() {
        return Ok(json!({"views": page.views}));
    }
    let mut query = "SELECT COUNT(*) FROM view_events WHERE page_id = ?1".to_string();
    for (part, value) in [
        ("%Y", form.year),
        ("%m", form.month),
        ("%d", form.day),
        ("%H", form.hour),
    ] {
        if let Some(value) = value {
            query.push_str(&format!(
                " AND CAST(strftime('{part}', viewed_at, 'unixepoch') AS INTEGER) = {value}"
            ));
        }
    }
    let count: i64 = store
        .conn()?
        .query_row(&query, [page.id], |row| row.get(0))
        .map_err(|_| "DATABASE_ERROR")?;
    Ok(json!({"views": count}))
}

async fn views_get(
    State(store): State<Arc<Store>>,
    Query(params): Query<ViewsParams>,
) -> Json<Value> {
    respond(views(&store, None, &params))
}

async fn views_post(
    State(store): State<Arc<Store>>,
    Form(params): Form<ViewsParams>,
) -> Json<Value> {
    respond(views(&store, None, &params))
}

api_path_method!(views_path_get, views_path_post, ViewsParams, views);

pub fn routes() -> Router<Arc<Store>> {
    Router::new()
        .route(
            "/createAccount",
            get(create_account_get).post(create_account_post),
        )
        .route(
            "/editAccountInfo",
            get(edit_account_get).post(edit_account_post),
        )
        .route(
            "/getAccountInfo",
            get(get_account_get).post(get_account_post),
        )
        .route(
            "/revokeAccessToken",
            get(revoke_token_get).post(revoke_token_post),
        )
        .route("/createPage", get(create_page_get).post(create_page_post))
        .route("/editPage", get(edit_page_get).post(edit_page_post))
        .route(
            "/editPage/{path}",
            get(edit_page_path_get).post(edit_page_path_post),
        )
        .route("/getPage", get(get_page_get).post(get_page_post))
        .route(
            "/getPage/{path}",
            get(get_page_path_get).post(get_page_path_post),
        )
        .route("/getPageList", get(page_list_get).post(page_list_post))
        .route("/getViews", get(views_get).post(views_post))
        .route(
            "/getViews/{path}",
            get(views_path_get).post(views_path_post),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{app, form, get, request};
    use axum::http::Method;

    async fn create_test_account(app: &Router) -> String {
        let response = form(
            app.clone(),
            "/createAccount",
            &[("short_name", "Test Account"), ("author_name", "Tester")],
        )
        .await;
        response.json()["result"]["access_token"]
            .as_str()
            .expect("access token")
            .to_string()
    }

    #[test]
    fn content_validation_rejects_unsafe_and_oversized_nodes() {
        let valid = Some(json!([{"tag":"p","children":["safe"]}]).to_string());
        assert!(parse_content(&valid, 65_536).is_ok());
        for invalid in [
            json!([{"tag":"script","children":["alert(1)"]}]).to_string(),
            json!([{"tag":"svg","attrs":{"onload":"alert(1)"}}]).to_string(),
            json!([{"tag":"form","children":[]}]).to_string(),
        ] {
            assert_eq!(
                parse_content(&Some(invalid), 65_536).unwrap_err(),
                "CONTENT_INVALID"
            );
        }
        assert_eq!(
            parse_content(&Some(json!(["x".repeat(65_540)]).to_string()), 65_536).unwrap_err(),
            "CONTENT_TOO_BIG"
        );
        let mut nested = "[]".to_string();
        for _ in 0..200 {
            nested = format!("[{{\"tag\":\"p\",\"children\":[{nested}]}}]");
        }
        assert!(parse_content(&Some(nested), 65_536).is_err());
    }

    #[tokio::test]
    async fn account_and_page_api_lifecycle() {
        let (_directory, app) = app();
        let account = form(
            app.clone(),
            "/api/createAccount",
            &[("short_name", "Lifecycle"), ("author_name", "Tester")],
        )
        .await
        .json();
        let token = account["result"]["access_token"].as_str().unwrap();
        let account_info = get(
            app.clone(),
            &format!(
                "/api/getAccountInfo?access_token={token}&fields=%5B%22short_name%22%2C%22page_count%22%5D"
            ),
        )
        .await
        .json();
        assert_eq!(account_info["result"]["short_name"], "Lifecycle");
        assert_eq!(account_info["result"]["page_count"], 0);

        let edited = form(
            app.clone(),
            "/api/editAccountInfo",
            &[("access_token", token), ("short_name", "Edited")],
        )
        .await
        .json();
        assert_eq!(edited["result"]["short_name"], "Edited");
        assert!(edited["result"].get("access_token").is_none());

        let content =
            json!([{"tag":"p","children":["Hello, ",{"tag":"strong","children":["world"]}]}])
                .to_string();
        let created = form(
            app.clone(),
            "/api/createPage",
            &[
                ("access_token", token),
                ("title", "Lifecycle Page"),
                ("content", &content),
                ("return_content", "true"),
            ],
        )
        .await
        .json();
        let path = created["result"]["path"].as_str().unwrap().to_string();
        assert_eq!(
            created["result"]["content"],
            serde_json::from_str::<Value>(&content).unwrap()
        );

        let fetched = get(
            app.clone(),
            &format!("/api/getPage/{path}?return_content=true&access_token={token}"),
        )
        .await
        .json();
        assert_eq!(fetched["result"]["can_edit"], true);

        let updated_content = json!([{"tag":"p","children":["Updated"]}]).to_string();
        let updated = form(
            app.clone(),
            &format!("/api/editPage/{path}"),
            &[
                ("access_token", token),
                ("title", "Updated Lifecycle"),
                ("content", &updated_content),
                ("return_content", "true"),
            ],
        )
        .await
        .json();
        assert_eq!(updated["result"]["title"], "Updated Lifecycle");
        assert_eq!(updated["result"]["path"], path);

        let list = form(
            app.clone(),
            "/api/getPageList",
            &[("access_token", token), ("limit", "1")],
        )
        .await
        .json();
        assert_eq!(list["result"]["total_count"], 1);
        assert_eq!(list["result"]["pages"].as_array().unwrap().len(), 1);

        assert_eq!(
            get(app.clone(), &format!("/api/getViews/{path}"))
                .await
                .json()["result"]["views"],
            0
        );
        assert_eq!(get(app.clone(), &format!("/{path}")).await.status, 200);
        assert_eq!(
            get(app.clone(), &format!("/api/getViews/{path}"))
                .await
                .json()["result"]["views"],
            1
        );
        let now = Utc::now();
        let filtered = get(
            app.clone(),
            &format!(
                "/api/getViews/{path}?year={}&month={}&day={}&hour={}",
                now.format("%Y"),
                now.format("%m"),
                now.format("%d"),
                now.format("%H")
            ),
        )
        .await
        .json();
        assert_eq!(filtered["result"]["views"], 1);

        let revoked = form(
            app.clone(),
            "/api/revokeAccessToken",
            &[("access_token", token)],
        )
        .await
        .json();
        let new_token = revoked["result"]["access_token"].as_str().unwrap();
        assert_ne!(new_token, token);
        assert_eq!(
            form(
                app.clone(),
                "/api/getAccountInfo",
                &[("access_token", token)]
            )
            .await
            .json()["ok"],
            false
        );
        assert_eq!(
            form(
                app.clone(),
                "/api/getAccountInfo",
                &[("access_token", new_token)]
            )
            .await
            .json()["ok"],
            true
        );
    }

    #[tokio::test]
    async fn legacy_api_aliases_and_documentation_remain_available() {
        let (_directory, app) = app();
        for uri in [
            "/createAccount",
            "/editAccountInfo",
            "/getAccountInfo",
            "/revokeAccessToken",
            "/createPage",
            "/editPage",
            "/editPage/test",
            "/getPage",
            "/getPage/test",
            "/getPageList",
            "/getViews",
            "/getViews/test",
        ] {
            let response = get(app.clone(), uri).await;
            assert_ne!(response.status, axum::http::StatusCode::NOT_FOUND, "{uri}");
        }
        let docs = get(app, "/api").await;
        assert_eq!(docs.status, axum::http::StatusCode::OK);
        assert!(docs.text().contains("Telegraph API"));
    }

    #[tokio::test]
    async fn api_writes_are_sanitized_before_storage() {
        let (_directory, app) = crate::test_utils::app_with_privacy(true);
        let account = form(
            app.clone(),
            "/api/createAccount",
            &[
                ("short_name", "Privacy"),
                ("author_url", "javascript:alert(1)?utm_source=x"),
            ],
        )
        .await
        .json();
        let token = account["result"]["access_token"].as_str().unwrap();
        assert_eq!(account["result"]["author_url"], "");

        let edited = form(
            app.clone(),
            "/api/editAccountInfo",
            &[
                ("access_token", token),
                ("author_url", "https://example.com/?id=1&utm_source=x"),
            ],
        )
        .await
        .json();
        assert_eq!(edited["result"]["author_url"], "https://example.com/?id=1");

        let content = json!([
            {"tag":"a","attrs":{"href":"https://example.com/?id=2&utm_source=x","target":"_blank","rel":"opener nofollow","ping":"https://evil.example"},"children":["safe"]},
            {"tag":"a","attrs":{"href":"javascript:alert(1)?utm_source=x","onclick":"alert(1)"},"children":["unsafe"]},
            {"tag":"img","attrs":{"src":"data:text/html;base64,PHNjcmlwdD4=?utm_source=x","onerror":"alert(1)"}},
            {"tag":"iframe","attrs":{"src":"https://www.youtube.com/watch?v=abc&si=tracking&pp=tracking&t=30"}},
            {"tag":"iframe","attrs":{"src":"/embed/youtube?url=https%3A%2F%2Fyoutu.be%2FdQw4w9WgXcQ%3Fsi%3Dx%26t%3D42"}}
        ]);
        let page = form(
            app.clone(),
            "/api/createPage",
            &[
                ("access_token", token),
                ("title", "Privacy"),
                ("author_url", "javascript:alert(1)?utm_source=x"),
                ("content", &content.to_string()),
                ("return_content", "true"),
            ],
        )
        .await
        .json();
        assert!(page["result"].get("author_url").is_none());
        let serialized = page["result"]["content"].to_string();
        for forbidden in [
            "utm_",
            "javascript:",
            "data:text/html",
            "onclick",
            "onerror",
            "ping",
        ] {
            assert!(
                !serialized.contains(forbidden),
                "returned unsafe {forbidden}"
            );
        }
        assert_eq!(
            page["result"]["content"][0]["attrs"]["href"],
            "https://example.com/?id=2"
        );
        assert_eq!(
            page["result"]["content"][0]["attrs"]["rel"],
            "nofollow noopener noreferrer"
        );
        assert_eq!(
            page["result"]["content"][3]["attrs"]["src"],
            "https://www.youtube.com/embed/abc?t=30"
        );
        assert_eq!(
            page["result"]["content"][4]["attrs"]["src"],
            "https://www.youtube.com/embed/dQw4w9WgXcQ?t=42"
        );
    }

    #[tokio::test]
    async fn api_handles_malformed_input_and_sql_injection() {
        let (_directory, app) = app();
        let token = create_test_account(&app).await;
        let invalid_content = json!([{"tag":"script","children":["alert(1)"]}]).to_string();
        let rejected = form(
            app.clone(),
            "/createPage",
            &[
                ("access_token", &token),
                ("title", "Rejected"),
                ("content", &invalid_content),
            ],
        )
        .await
        .json();
        assert_eq!(rejected["error"], "CONTENT_INVALID");

        let title = "'); DROP TABLE pages;--";
        let created = form(
            app.clone(),
            "/createPage",
            &[
                ("access_token", &token),
                ("title", title),
                ("content", "[]"),
            ],
        )
        .await
        .json();
        let path = created["result"]["path"].as_str().unwrap();
        assert_eq!(
            get(app.clone(), &format!("/getPage/{path}")).await.json()["result"]["title"],
            title
        );
        let response = request(
            app.clone(),
            Method::GET,
            "/getViews/anything?year=2026%20OR%201%3D1",
            &[],
            Vec::new(),
        )
        .await;
        assert!(response.status.is_client_error());
        let pages = form(app.clone(), "/getPageList", &[("access_token", &token)])
            .await
            .json();
        assert_eq!(pages["result"]["total_count"], 1);
    }
}

#[cfg(test)]
mod validation_config_tests {
    use super::*;
    use crate::db::ValidationConfig;
    use crate::test_utils::{app_with_validation, form};

    #[test]
    fn content_limit_never_falls_below_32_kibibytes() {
        let config = ValidationConfig {
            content_max_bytes: 1_024,
            ..ValidationConfig::default()
        };
        assert_eq!(config.content_max_bytes(), 32 * 1024);
        let accepted = Some(json!(["x".repeat(32 * 1024 - 100)]).to_string());
        assert!(parse_content(&accepted, config.content_max_bytes()).is_ok());
        let rejected = Some(json!(["x".repeat(32 * 1024 + 100)]).to_string());
        assert_eq!(
            parse_content(&rejected, config.content_max_bytes()).unwrap_err(),
            "CONTENT_TOO_BIG"
        );
    }

    #[tokio::test]
    async fn configured_title_limits_are_enforced() {
        let (_directory, app) = app_with_validation(ValidationConfig {
            title_max_chars: 3,
            author_name_max_chars: 2,
            ..ValidationConfig::default()
        });
        let account = form(app.clone(), "/createAccount", &[("short_name", "Limits")])
            .await
            .json();
        let token = account["result"]["access_token"].as_str().unwrap();
        let rejected = form(
            app.clone(),
            "/createPage",
            &[
                ("access_token", token),
                ("title", "Four"),
                ("content", "[]"),
            ],
        )
        .await
        .json();
        assert_eq!(rejected["error"], "TITLE_TOO_LONG");
        let accepted = form(
            app.clone(),
            "/createPage",
            &[("access_token", token), ("title", "abc"), ("content", "[]")],
        )
        .await
        .json();
        assert_eq!(accepted["ok"], true);

        let rejected = form(
            app,
            "/createAccount",
            &[("short_name", "Limits"), ("author_name", "abc")],
        )
        .await
        .json();
        assert_eq!(rejected["error"], "AUTHOR_NAME_TOO_LONG");
    }
}
