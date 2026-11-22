use crate::db::{Store, StoreConfig};
use axum::body::Body;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::{Router, middleware};
use http_body_util::BodyExt;
use serde_json::Value;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;
use tower_http::{compression::CompressionLayer, services::ServeDir};

pub struct TestResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl TestResponse {
    pub fn json(self) -> Value {
        serde_json::from_slice(&self.body).expect("response should be JSON")
    }

    pub fn text(self) -> String {
        String::from_utf8(self.body).expect("response should be UTF-8")
    }
}

pub fn store() -> (TempDir, Arc<Store>) {
    let directory = tempfile::tempdir().expect("temporary test directory");
    let config = StoreConfig {
        database: directory
            .path()
            .join("test.db")
            .to_string_lossy()
            .into_owned(),
        base_url: "http://localhost:8001".into(),
        upload_dir: directory.path().join("uploads"),
        ..StoreConfig::default()
    };
    let store = Store::open(&config, false, false).expect("test database");
    (directory, Arc::new(store))
}

pub fn app() -> (TempDir, Router) {
    app_with_uploads(true)
}

pub fn app_with_uploads(upload_enabled: bool) -> (TempDir, Router) {
    let (directory, mut store) = store();
    Arc::get_mut(&mut store)
        .expect("unique test store")
        .frontend
        .upload_enabled = upload_enabled;
    build_app(directory, store)
}

pub fn app_with_identity(name: &str, description: &str) -> (TempDir, Router) {
    let (directory, mut store) = store();
    let frontend = &mut Arc::get_mut(&mut store)
        .expect("unique test store")
        .frontend;
    frontend.name = name.into();
    frontend.description = description.into();
    build_app(directory, store)
}

pub fn app_with_privacy(strip_utm: bool) -> (TempDir, Router) {
    let (directory, mut store) = store();
    Arc::get_mut(&mut store)
        .expect("unique test store")
        .privacy
        .strip_utm = strip_utm;
    build_app(directory, store)
}

pub fn app_with_reporting(report_enabled: bool) -> (TempDir, Router) {
    let (directory, mut store) = store();
    Arc::get_mut(&mut store)
        .expect("unique test store")
        .frontend
        .report_enabled = report_enabled;
    build_app(directory, store)
}

pub fn app_with_validation(validation: crate::db::ValidationConfig) -> (TempDir, Router) {
    let (directory, mut store) = store();
    Arc::get_mut(&mut store)
        .expect("unique test store")
        .validation = validation;
    build_app(directory, store)
}

fn build_app(directory: TempDir, store: Arc<Store>) -> (TempDir, Router) {
    let app = Router::new()
        .nest(
            "/api",
            crate::api::routes().route_layer(DefaultBodyLimit::max(128 * 1024)),
        )
        .nest(
            "/api",
            crate::web::browser_api_routes().route_layer(DefaultBodyLimit::max(10 * 1024 * 1024)),
        )
        .merge(crate::api::routes().route_layer(DefaultBodyLimit::max(128 * 1024)))
        .merge(crate::web::routes().route_layer(DefaultBodyLimit::max(10 * 1024 * 1024)))
        .nest_service("/css", ServeDir::new("static/css"))
        .nest_service("/js", ServeDir::new("static/js"))
        .nest_service("/images", ServeDir::new("static/images"))
        .fallback(crate::web::not_found_catcher)
        .layer(middleware::map_response(crate::security_headers))
        .layer(CompressionLayer::new())
        .layer(middleware::from_fn(crate::i18n::middleware))
        .with_state(store);
    (directory, app)
}

pub async fn request(
    app: Router,
    method: Method,
    uri: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> TestResponse {
    let mut request = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = app
        .oneshot(request.body(Body::from(body)).expect("test request"))
        .await
        .expect("application response");
    let (parts, body) = response.into_parts();
    let body = body
        .collect()
        .await
        .expect("response body")
        .to_bytes()
        .to_vec();
    TestResponse {
        status: parts.status,
        headers: parts.headers,
        body,
    }
}

pub async fn get(app: Router, uri: &str) -> TestResponse {
    request(app, Method::GET, uri, &[], Vec::new()).await
}

pub async fn form(app: Router, uri: &str, fields: &[(&str, &str)]) -> TestResponse {
    let body = serde_urlencoded::to_string(fields).expect("form fields");
    request(
        app,
        Method::POST,
        uri,
        &[("content-type", "application/x-www-form-urlencoded")],
        body.into_bytes(),
    )
    .await
}

pub fn multipart(fields: &[(&str, &str)], file: (&str, &str, &[u8], &str)) -> (Vec<u8>, String) {
    let boundary = "------------------------TelegraphTestBoundary";
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    let (name, filename, bytes, content_type) = file;
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (body, format!("multipart/form-data; boundary={boundary}"))
}
