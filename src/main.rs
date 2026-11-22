mod api;
mod db;
mod handlers;
mod i18n;
mod model;
#[cfg(test)]
mod test_utils;
mod tui;
mod web;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware;
use axum::response::Response;
use db::Store;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tower_http::{compression::CompressionLayer, services::ServeDir};

#[derive(serde::Deserialize)]
#[serde(default)]
struct AppConfig {
    address: IpAddr,
    port: u16,
    limits: LimitsConfig,
    backend: db::StoreConfig,
    frontend: db::FrontendConfig,
    privacy: db::PrivacyConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 8000,
            limits: LimitsConfig::default(),
            backend: db::StoreConfig::default(),
            frontend: db::FrontendConfig::default(),
            privacy: db::PrivacyConfig::default(),
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(default)]
struct LimitsConfig {
    file: ubyte::ByteUnit,
    #[serde(rename = "data-form")]
    data_form: ubyte::ByteUnit,
    form: ubyte::ByteUnit,
    string: ubyte::ByteUnit,
    #[serde(flatten)]
    validation: db::ValidationConfig,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            file: ubyte::ByteUnit::Mebibyte(10),
            data_form: ubyte::ByteUnit::Mebibyte(10),
            form: ubyte::ByteUnit::Kibibyte(128),
            string: ubyte::ByteUnit::Kibibyte(128),
            validation: db::ValidationConfig::default(),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let config_path = std::env::var("TELEGRAPH_CONFIG").unwrap_or_else(|_| "Config.toml".into());
    let mut config: AppConfig = toml::from_str(&std::fs::read_to_string(config_path)?)?;
    if let Ok(address) = std::env::var("ROCKET_ADDRESS") {
        config.address = address.parse()?;
    }
    if let Ok(port) = std::env::var("ROCKET_PORT") {
        config.port = port.parse()?;
    }
    let address = SocketAddr::new(config.address, config.port);
    let api_limit = config
        .limits
        .form
        .as_u64()
        .max(config.limits.string.as_u64()) as usize;
    let web_limit = config
        .limits
        .data_form
        .as_u64()
        .max(config.limits.file.as_u64()) as usize;
    let store_config = db::StoreConfig {
        database: config.backend.database,
        base_url: config.backend.base_url,
        upload_dir: config.backend.upload_dir,
        wal_checkpoint_interval_secs: config.backend.wal_checkpoint_interval_secs,
        upload_cleanup_interval_secs: config.backend.upload_cleanup_interval_secs,
        upload_max_age_secs: config.backend.upload_max_age_secs,
        session_max_age_days: config.backend.session_max_age_days,
        orphaned_account_cleanup_hours: config.backend.orphaned_account_cleanup_hours,
        frontend: config.frontend,
        validation: config.limits.validation,
        privacy: config.privacy,
    };
    let store = Arc::new(Store::new(&store_config)?);
    if std::env::args().skip(1).any(|argument| argument == "--tui") {
        tui::run(store)?;
        return Ok(());
    }
    let public_url = store.base_url.clone();
    let app = Router::new()
        .nest(
            "/api",
            api::routes().route_layer(DefaultBodyLimit::max(api_limit)),
        )
        .nest(
            "/api",
            web::browser_api_routes().route_layer(DefaultBodyLimit::max(web_limit)),
        )
        .merge(api::routes().route_layer(DefaultBodyLimit::max(api_limit)))
        .merge(web::routes().route_layer(DefaultBodyLimit::max(web_limit)))
        .nest_service("/css", ServeDir::new("static/css"))
        .nest_service("/js", ServeDir::new("static/js"))
        .nest_service("/images", ServeDir::new("static/images"))
        .fallback(web::not_found_catcher)
        .layer(middleware::map_response(security_headers))
        .layer(CompressionLayer::new())
        .layer(middleware::from_fn(i18n::middleware))
        .with_state(store);
    let listener = tokio::net::TcpListener::bind(address).await?;
    println!("Server running at {public_url}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn security_headers(mut response: Response) -> Response {
    let is_not_found = response.status() == StatusCode::NOT_FOUND;
    let headers = response.headers_mut();
    headers.insert(
        header::X_FRAME_OPTIONS,
        HeaderValue::from_static("SAMEORIGIN"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("interest-cohort=()"),
    );
    if is_not_found {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_uses_backend_privacy_frontend_and_combined_limits() {
        let config: AppConfig = toml::from_str(include_str!("../Config.toml")).unwrap();
        assert_eq!(config.backend.database, "telegraph.db");
        assert_eq!(config.backend.base_url, "http://localhost:8000");
        assert!(config.privacy.strip_exif);
        assert!(config.privacy.strip_utm);
        assert_eq!(config.backend.orphaned_account_cleanup_hours, 24);
        assert_eq!(config.frontend.name, "Telegraph");
        assert!(
            config
                .frontend
                .description
                .contains("minimalist publishing tool")
        );
        assert!(config.frontend.upload_enabled);
        assert_eq!(config.limits.validation.title_max_chars, 256);
        assert!(config.limits.validation.content_max_bytes() >= 32 * 1024);
    }
}
