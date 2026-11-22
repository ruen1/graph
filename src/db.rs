use crate::handlers::sanitize::{SanitizedContent, SanitizedReport};
use crate::model::{Account, Node, Page, random_hex};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

pub struct Store {
    pub db: Mutex<Connection>,
    pub base_url: String,
    pub upload_dir: PathBuf,
    pub frontend: FrontendConfig,
    pub validation: ValidationConfig,
    pub privacy: PrivacyConfig,
}

#[derive(Clone, serde::Deserialize)]
#[serde(default)]
pub struct PrivacyConfig {
    pub strip_exif: bool,
    pub strip_utm: bool,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            strip_exif: true,
            strip_utm: false,
        }
    }
}

#[derive(Clone, serde::Deserialize)]
#[serde(default)]
pub struct ValidationConfig {
    pub title_max_chars: usize,
    pub author_name_max_chars: usize,
    pub author_url_max_chars: usize,
    pub content_max_bytes: usize,
}

impl Default for ValidationConfig {
    fn default() -> Self {
        Self {
            title_max_chars: 256,
            author_name_max_chars: 128,
            author_url_max_chars: 512,
            content_max_bytes: 65_536,
        }
    }
}

impl ValidationConfig {
    pub fn content_max_bytes(&self) -> usize {
        self.content_max_bytes.max(32 * 1024)
    }
}

#[derive(Clone, serde::Deserialize)]
#[serde(default)]
pub struct FrontendConfig {
    pub upload_enabled: bool,
    pub editable: bool,
    pub report_enabled: bool,
    pub name: String,
    pub description: String,
}

impl Default for FrontendConfig {
    fn default() -> Self {
        Self {
            upload_enabled: true,
            editable: true,
            report_enabled: true,
            name: "Telegraph".into(),
            description: "Telegraph is a minimalist publishing tool that allows you to create richly formatted posts and push them to the Web in just a click. Telegraph posts also get beautiful Instant View pages on Telegram.".into(),
        }
    }
}

#[derive(Clone, serde::Deserialize)]
#[serde(default)]
pub(crate) struct StoreConfig {
    pub(crate) database: String,
    pub(crate) base_url: String,
    pub(crate) upload_dir: PathBuf,
    pub(crate) wal_checkpoint_interval_secs: u64,
    pub(crate) upload_cleanup_interval_secs: u64,
    pub(crate) upload_max_age_secs: u64,
    pub(crate) frontend: FrontendConfig,
    pub(crate) validation: ValidationConfig,
    pub(crate) privacy: PrivacyConfig,
    pub(crate) session_max_age_days: u64,
    pub(crate) orphaned_account_cleanup_hours: u64,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            database: "telegraph.db".into(),
            base_url: "http://localhost:8000".into(),
            upload_dir: "uploads".into(),
            wal_checkpoint_interval_secs: 30,
            upload_cleanup_interval_secs: 60,
            upload_max_age_secs: 86_400,
            frontend: FrontendConfig::default(),
            validation: ValidationConfig::default(),
            privacy: PrivacyConfig::default(),
            session_max_age_days: 30,
            orphaned_account_cleanup_hours: 24,
        }
    }
}

impl StoreConfig {
    fn apply_environment(&mut self) {
        if let Ok(value) = std::env::var("TELEGRAPH_DATABASE") {
            self.database = value;
        }
        if let Ok(value) = std::env::var("TELEGRAPH_BASE_URL") {
            self.base_url = value;
        }
        if let Ok(value) = std::env::var("TELEGRAPH_UPLOAD_DIR") {
            self.upload_dir = value.into();
        }
        self.wal_checkpoint_interval_secs = environment_seconds(
            "TELEGRAPH_WAL_CHECKPOINT_INTERVAL_SECS",
            self.wal_checkpoint_interval_secs,
        );
        self.upload_cleanup_interval_secs = environment_seconds(
            "TELEGRAPH_UPLOAD_CLEANUP_INTERVAL_SECS",
            self.upload_cleanup_interval_secs,
        );
        self.upload_max_age_secs =
            environment_seconds("TELEGRAPH_UPLOAD_MAX_AGE_SECS", self.upload_max_age_secs);
    }
}

impl Store {
    pub fn new(config: &StoreConfig) -> Result<Self, String> {
        Self::open(config, true, true)
    }

    pub(crate) fn open(
        config: &StoreConfig,
        start_workers: bool,
        apply_environment: bool,
    ) -> Result<Self, String> {
        let mut config = config.clone();
        if apply_environment {
            config.apply_environment();
        }
        let StoreConfig {
            database,
            base_url,
            upload_dir,
            wal_checkpoint_interval_secs,
            upload_cleanup_interval_secs,
            upload_max_age_secs,
            frontend,
            validation,
            privacy,
            session_max_age_days,
            orphaned_account_cleanup_hours,
        } = config;
        let base_url = base_url.trim_end_matches('/').to_string();
        std::fs::create_dir_all(&upload_dir).map_err(|_| "UPLOAD_DIRECTORY_ERROR".to_string())?;
        let connection = Connection::open(&database).map_err(|_| "DATABASE_ERROR".to_string())?;
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 PRAGMA journal_mode = WAL;
                 CREATE TABLE IF NOT EXISTS accounts (
                   id INTEGER PRIMARY KEY,
                   access_token TEXT NOT NULL UNIQUE,
                   short_name TEXT NOT NULL,
                   author_name TEXT NOT NULL DEFAULT '',
                   author_url TEXT NOT NULL DEFAULT '',
                   auth_token TEXT UNIQUE,
                   auth_created INTEGER NOT NULL DEFAULT 0,
                   created_at INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE TABLE IF NOT EXISTS sessions (
                   token TEXT PRIMARY KEY,
                   account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                   created_at INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE TABLE IF NOT EXISTS pages (
                   id TEXT PRIMARY KEY,
                   path TEXT NOT NULL UNIQUE,
                   account_id INTEGER NOT NULL REFERENCES accounts(id),
                   title TEXT NOT NULL,
                   author_name TEXT NOT NULL DEFAULT '',
                   author_url TEXT NOT NULL DEFAULT '',
                   content TEXT NOT NULL,
                   created_at INTEGER NOT NULL,
                   updated_at INTEGER NOT NULL,
                   views INTEGER NOT NULL DEFAULT 0,
                   save_hash TEXT NOT NULL,
                   hidden INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE INDEX IF NOT EXISTS pages_account ON pages(account_id, created_at DESC);
                 CREATE TABLE IF NOT EXISTS view_events (
                   page_id TEXT NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
                   viewed_at INTEGER NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS views_page_time ON view_events(page_id, viewed_at);
                 CREATE TABLE IF NOT EXISTS reports (
                   id INTEGER PRIMARY KEY,
                   page_id TEXT NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
                   reason TEXT NOT NULL,
                   comment TEXT NOT NULL,
                   created_at INTEGER NOT NULL
                 );",
            )
            .map_err(|_| "DATABASE_ERROR".to_string())?;
        let has_created_at: bool = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'created_at'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| "DATABASE_ERROR")?;
        if !has_created_at {
            connection
                .execute_batch(
                    "ALTER TABLE sessions ADD COLUMN created_at INTEGER NOT NULL DEFAULT 0;
                     UPDATE sessions SET created_at = strftime('%s', 'now') WHERE created_at = 0;",
                )
                .map_err(|_| "DATABASE_ERROR")?;
        }
        let has_account_created_at: bool = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('accounts') WHERE name = 'created_at'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| "DATABASE_ERROR")?;
        if !has_account_created_at {
            connection
                .execute_batch(
                    "ALTER TABLE accounts ADD COLUMN created_at INTEGER NOT NULL DEFAULT 0;
                     UPDATE accounts SET created_at = auth_created WHERE created_at = 0;",
                )
                .map_err(|_| "DATABASE_ERROR")?;
        }
        let has_hidden: bool = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('pages') WHERE name = 'hidden'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| "DATABASE_ERROR")?;
        if !has_hidden {
            connection
                .execute_batch("ALTER TABLE pages ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0")
                .map_err(|_| "DATABASE_ERROR")?;
        }
        if start_workers {
            start_account_cleanup(&database, orphaned_account_cleanup_hours)?;
            start_session_cleanup(&database, session_max_age_days)?;
            start_wal_checkpoints(&database, wal_checkpoint_interval_secs)?;
            start_upload_cleanup(
                &database,
                &upload_dir,
                upload_max_age_secs,
                upload_cleanup_interval_secs,
            )?;
        }
        Ok(Self {
            db: Mutex::new(connection),
            base_url,
            upload_dir,
            frontend,
            validation,
            privacy,
        })
    }

    pub fn conn(&self) -> Result<MutexGuard<'_, Connection>, String> {
        self.db.lock().map_err(|_| "DATABASE_ERROR".into())
    }

    pub fn account_by_token(&self, token: &str) -> Result<Option<Account>, String> {
        self.conn()?
            .query_row(
                "SELECT * FROM accounts WHERE access_token = ?1",
                [token],
                Account::from_row,
            )
            .optional()
            .map_err(|_| "DATABASE_ERROR".into())
    }

    pub fn account_by_session(&self, token: &str) -> Result<Option<Account>, String> {
        self.conn()?
            .query_row(
                "SELECT a.* FROM accounts a JOIN sessions s ON s.account_id = a.id WHERE s.token = ?1",
                [token],
                Account::from_row,
            )
            .optional()
            .map_err(|_| "DATABASE_ERROR".into())
    }

    pub fn create_account(
        &self,
        short_name: &str,
        author_name: &str,
        author_url: &str,
    ) -> Result<Account, String> {
        let short_name = crate::handlers::sanitize::sanitize_text(short_name);
        let author_name = crate::handlers::sanitize::sanitize_text(author_name);
        let author_url =
            crate::handlers::urls::sanitize_author_url(author_url, self.privacy.strip_utm);
        let now = Utc::now().timestamp();
        let account = Account {
            id: 0,
            access_token: random_hex(32),
            short_name,
            author_name,
            author_url,
            auth_token: Some(random_hex(16)),
            auth_created: now,
            created_at: now,
        };
        self.conn()?
            .execute(
                "INSERT INTO accounts(access_token, short_name, author_name, author_url, auth_token, auth_created, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    account.access_token,
                    account.short_name,
                    account.author_name,
                    account.author_url,
                    account.auth_token,
                    account.auth_created,
                    account.created_at,
                ],
            )
            .map_err(|_| "DATABASE_ERROR")?;
        self.account_by_token(&account.access_token)?
            .ok_or_else(|| "DATABASE_ERROR".into())
    }

    pub fn update_account(
        &self,
        account: &Account,
        short_name: Option<&str>,
        author_name: Option<&str>,
        author_url: Option<&str>,
    ) -> Result<Account, String> {
        let short_name = short_name.map(crate::handlers::sanitize::sanitize_text);
        let author_name = author_name.map(crate::handlers::sanitize::sanitize_text);
        let author_url = author_url
            .map(|value| crate::handlers::urls::sanitize_author_url(value, self.privacy.strip_utm));
        self.conn()?
            .execute(
                "UPDATE accounts SET short_name = COALESCE(?2, short_name), author_name = COALESCE(?3, author_name), author_url = COALESCE(?4, author_url) WHERE id = ?1",
                params![account.id, short_name, author_name, author_url],
            )
            .map_err(|_| "DATABASE_ERROR")?;
        self.account_by_token(&account.access_token)?
            .ok_or_else(|| "DATABASE_ERROR".into())
    }

    pub fn create_report(&self, report: &SanitizedReport) -> Result<(), String> {
        self.conn()?
            .execute(
                "INSERT INTO reports(page_id, reason, comment, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![
                    report.page_id(),
                    report.reason(),
                    report.comment(),
                    Utc::now().timestamp()
                ],
            )
            .map_err(|_| "DATABASE_ERROR")?;
        Ok(())
    }

    pub fn discard_reports(&self, page_id: &str) -> Result<usize, String> {
        self.conn()?
            .execute("DELETE FROM reports WHERE page_id = ?1", [page_id])
            .map_err(|_| "DATABASE_ERROR".into())
    }

    pub fn create_session(&self, account_id: i64) -> Result<String, String> {
        let token = random_hex(32);
        self.conn()?
            .execute(
                "INSERT INTO sessions(token, account_id, created_at) VALUES (?1, ?2, ?3)",
                params![token, account_id, Utc::now().timestamp()],
            )
            .map_err(|_| "DATABASE_ERROR")?;
        Ok(token)
    }

    pub fn page_by_id(&self, id: &str) -> Result<Option<Page>, String> {
        self.conn()?
            .query_row("SELECT * FROM pages WHERE id = ?1", [id], Page::from_row)
            .optional()
            .map_err(|_| "DATABASE_ERROR".into())
    }

    pub fn page_by_path(&self, path: &str) -> Result<Option<Page>, String> {
        self.conn()?
            .query_row(
                "SELECT * FROM pages WHERE path = ?1 AND hidden = 0",
                [path],
                Page::from_row,
            )
            .optional()
            .map_err(|_| "DATABASE_ERROR".into())
    }

    pub fn set_page_hidden(&self, id: &str, hidden: bool) -> Result<(), String> {
        let changed = self
            .conn()?
            .execute(
                "UPDATE pages SET hidden = ?2 WHERE id = ?1",
                params![id, hidden],
            )
            .map_err(|_| "DATABASE_ERROR")?;
        if changed == 1 {
            Ok(())
        } else {
            Err("PAGE_NOT_FOUND".into())
        }
    }

    pub fn delete_page(&self, id: &str) -> Result<(), String> {
        let deleted = self
            .conn()?
            .execute("DELETE FROM pages WHERE id = ?1", [id])
            .map_err(|_| "DATABASE_ERROR")?;
        if deleted == 1 {
            Ok(())
        } else {
            Err("PAGE_NOT_FOUND".into())
        }
    }

    pub fn list_pages(&self) -> Result<Vec<Page>, String> {
        let connection = self.conn()?;
        let mut statement = connection
            .prepare("SELECT * FROM pages ORDER BY created_at DESC, id DESC")
            .map_err(|_| "DATABASE_ERROR")?;
        let pages = statement
            .query_map([], Page::from_row)
            .map_err(|_| "DATABASE_ERROR")?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "DATABASE_ERROR")?;
        Ok(pages)
    }

    pub fn list_reported_pages(&self) -> Result<Vec<crate::model::ReportedPage>, String> {
        let connection = self.conn()?;
        let mut statement = connection
            .prepare(
                "SELECT pages.*, GROUP_CONCAT(DISTINCT reports.reason) AS reasons,
                        COUNT(reports.id) AS report_count, MAX(reports.created_at) AS last_report
                 FROM pages
                 JOIN reports ON reports.page_id = pages.id
                 GROUP BY pages.id
                 ORDER BY report_count DESC, last_report DESC, pages.id DESC",
            )
            .map_err(|_| "DATABASE_ERROR")?;
        let pages = statement
            .query_map([], crate::model::ReportedPage::from_row)
            .map_err(|_| "DATABASE_ERROR")?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "DATABASE_ERROR")?;
        Ok(pages)
    }

    pub fn create_page(
        &self,
        account: &Account,
        title: &str,
        author_name: &str,
        author_url: &str,
        content: SanitizedContent,
    ) -> Result<Page, String> {
        let title = crate::handlers::sanitize::sanitize_text(title);
        let author_name = crate::handlers::sanitize::sanitize_text(author_name);
        let id = random_hex(10);
        let now = Utc::now();
        let path = self.unique_path(&title, now)?;
        let author_url =
            crate::handlers::urls::sanitize_author_url(author_url, self.privacy.strip_utm);
        let content_json =
            serde_json::to_string(content.as_slice()).map_err(|_| "CONTENT_INVALID")?;
        let save_hash = random_hex(16);
        self.conn()?
            .execute(
                "INSERT INTO pages(id, path, account_id, title, author_name, author_url, content, created_at, updated_at, save_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9)",
                params![
                    id,
                    path,
                    account.id,
                    title,
                    author_name,
                    author_url,
                    content_json,
                    now.timestamp(),
                    save_hash,
                ],
            )
            .map_err(|_| "DATABASE_ERROR")?;
        self.page_by_id(&id)?.ok_or_else(|| "DATABASE_ERROR".into())
    }

    pub fn update_page(
        &self,
        page: &Page,
        title: &str,
        author_name: &str,
        author_url: &str,
        content: SanitizedContent,
    ) -> Result<Page, String> {
        let title = crate::handlers::sanitize::sanitize_text(title);
        let author_name = crate::handlers::sanitize::sanitize_text(author_name);
        let author_url =
            crate::handlers::urls::sanitize_author_url(author_url, self.privacy.strip_utm);
        let content_json =
            serde_json::to_string(content.as_slice()).map_err(|_| "CONTENT_INVALID")?;
        self.conn()?
            .execute(
                "UPDATE pages SET title = ?2, author_name = ?3, author_url = ?4, content = ?5, updated_at = ?6 WHERE id = ?1",
                params![
                    page.id,
                    title,
                    author_name,
                    author_url,
                    content_json,
                    Utc::now().timestamp(),
                ],
            )
            .map_err(|_| "DATABASE_ERROR")?;
        self.page_by_id(&page.id)?
            .ok_or_else(|| "PAGE_NOT_FOUND".into())
    }

    pub fn record_view(&self, page: &Page) -> Result<Page, String> {
        let now = Utc::now().timestamp();
        let connection = self.conn()?;
        connection
            .execute(
                "INSERT INTO view_events(page_id, viewed_at) VALUES (?1, ?2)",
                params![page.id, now],
            )
            .map_err(|_| "DATABASE_ERROR")?;
        connection
            .execute(
                "UPDATE pages SET views = views + 1 WHERE id = ?1",
                [page.id.as_str()],
            )
            .map_err(|_| "DATABASE_ERROR")?;
        drop(connection);
        self.page_by_id(&page.id)?
            .ok_or_else(|| "PAGE_NOT_FOUND".into())
    }

    fn unique_path(&self, title: &str, now: chrono::DateTime<Utc>) -> Result<String, String> {
        let slug = slugify(title);
        let base = format!("{}-{}", slug, now.format("%m-%d"));
        let connection = self.conn()?;
        for suffix in 1.. {
            let path = if suffix == 1 {
                base.clone()
            } else {
                format!("{base}-{suffix}")
            };
            let exists: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM pages WHERE path = ?1)",
                    [&path],
                    |row| row.get(0),
                )
                .map_err(|_| "DATABASE_ERROR")?;
            if !exists {
                return Ok(path);
            }
        }
        unreachable!()
    }
}

fn slugify(title: &str) -> String {
    let transliterated = deunicode::deunicode(title);
    let mut slug = String::new();
    let mut separator = false;
    for character in transliterated.chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(character);
            separator = false;
        } else {
            separator = true;
        }
        if slug.len() >= 120 {
            break;
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "Page".into()
    } else {
        slug.into()
    }
}

fn start_wal_checkpoints(database: &str, interval: u64) -> Result<(), String> {
    let connection = Connection::open(database).map_err(|_| "DATABASE_ERROR".to_string())?;
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(interval));
            if let Err(error) =
                connection.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()))
            {
                log::warn!("WAL checkpoint failed: {error}");
            }
        }
    });
    Ok(())
}

fn start_session_cleanup(database: &str, max_age_days: u64) -> Result<(), String> {
    if max_age_days == 0 {
        return Ok(());
    }
    let connection = Connection::open(database).map_err(|_| "DATABASE_ERROR".to_string())?;
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(86_400));
            if let Err(error) = cleanup_sessions(&connection, max_age_days) {
                log::warn!("session cleanup failed: {error}");
            }
        }
    });
    Ok(())
}

fn cleanup_sessions(connection: &Connection, max_age_days: u64) -> Result<(), String> {
    if max_age_days == 0 {
        return Ok(());
    }
    let oldest = Utc::now().timestamp()
        - i64::try_from(max_age_days.saturating_mul(86_400)).unwrap_or(i64::MAX);
    connection
        .execute("DELETE FROM sessions WHERE created_at < ?1", [oldest])
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn start_account_cleanup(database: &str, max_age_hours: u64) -> Result<(), String> {
    if max_age_hours == 0 {
        return Ok(());
    }
    let connection = Connection::open(database).map_err(|_| "DATABASE_ERROR".to_string())?;
    cleanup_accounts(&connection, max_age_hours)?;
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3_600));
            if let Err(error) = cleanup_accounts(&connection, max_age_hours) {
                log::warn!("account cleanup failed: {error}");
            }
        }
    });
    Ok(())
}

fn cleanup_accounts(connection: &Connection, max_age_hours: u64) -> Result<(), String> {
    if max_age_hours == 0 {
        return Ok(());
    }
    let oldest = Utc::now().timestamp()
        - i64::try_from(max_age_hours.saturating_mul(3_600)).unwrap_or(i64::MAX);
    connection
        .execute(
            "DELETE FROM accounts WHERE created_at < ?1 AND NOT EXISTS(SELECT 1 FROM pages WHERE pages.account_id = accounts.id)",
            [oldest],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn start_upload_cleanup(
    database: &str,
    upload_dir: &std::path::Path,
    max_age: u64,
    interval: u64,
) -> Result<(), String> {
    let connection = Connection::open(database).map_err(|_| "DATABASE_ERROR".to_string())?;
    let upload_dir = upload_dir.to_path_buf();
    let max_age = Duration::from_secs(max_age);
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(interval));
            if let Err(error) = cleanup_uploads(&connection, &upload_dir, max_age) {
                log::warn!("upload cleanup failed: {error}");
            }
        }
    });
    Ok(())
}

fn environment_seconds(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn cleanup_uploads(
    connection: &Connection,
    upload_dir: &std::path::Path,
    max_age: Duration,
) -> Result<(), String> {
    let referenced = referenced_uploads(connection)?;
    let cutoff = SystemTime::now()
        .checked_sub(max_age)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    for entry in std::fs::read_dir(upload_dir).map_err(|error| error.to_string())? {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let file_name = entry.file_name();
        if referenced.contains(file_name.to_string_lossy().as_ref()) {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|metadata| metadata.modified()) else {
            continue;
        };
        if modified > cutoff {
            continue;
        }
        let path = entry.path();
        if let Err(error) = std::fs::remove_file(&path) {
            log::warn!(
                "failed to delete orphaned upload {}: {error}",
                path.display()
            );
        } else {
            log::info!("deleted orphaned upload {}", path.display());
        }
    }
    Ok(())
}

fn referenced_uploads(connection: &Connection) -> Result<HashSet<String>, String> {
    let mut statement = connection
        .prepare("SELECT content FROM pages")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?;
    let mut referenced = HashSet::new();
    for row in rows {
        let content = row.map_err(|error| error.to_string())?;
        let nodes: Vec<Node> = serde_json::from_str(&content).map_err(|error| error.to_string())?;
        collect_referenced_uploads(&nodes, &mut referenced);
    }
    Ok(referenced)
}

fn collect_referenced_uploads(nodes: &[Node], referenced: &mut HashSet<String>) {
    for node in nodes {
        let Node::Element(element) = node else {
            continue;
        };
        if let Some(attrs) = &element.attrs {
            for name in ["src", "href"] {
                if let Some(file_name) = attrs.get(name).and_then(|url| upload_file_name(url)) {
                    referenced.insert(file_name);
                }
            }
        }
        if let Some(children) = &element.children {
            collect_referenced_uploads(children, referenced);
        }
    }
}

fn upload_file_name(url: &str) -> Option<String> {
    let (_, file_name) = url.rsplit_once("/file/")?;
    let file_name = file_name.split(['?', '#']).next()?;
    if file_name.is_empty() || file_name.contains('/') || matches!(file_name, "." | "..") {
        None
    } else {
        Some(file_name.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub(crate) fn test_store() -> (tempfile::TempDir, Store) {
        let directory = tempfile::tempdir().unwrap();
        let config = StoreConfig {
            database: directory
                .path()
                .join("test.db")
                .to_string_lossy()
                .into_owned(),
            upload_dir: directory.path().join("uploads"),
            ..StoreConfig::default()
        };
        let store = Store::open(&config, false, false).unwrap();
        (directory, store)
    }

    #[test]
    fn dated_page_paths_are_unique() {
        let (_directory, store) = test_store();
        let account = store.create_account("Paths", "", "").unwrap();
        let first = store
            .create_page(
                &account,
                "Same Title",
                "",
                "",
                SanitizedContent::new(&[], false),
            )
            .unwrap();
        let second = store
            .create_page(
                &account,
                "Same Title",
                "",
                "",
                SanitizedContent::new(&[], false),
            )
            .unwrap();
        assert_ne!(first.path, second.path);
        assert!(first.path.starts_with("Same-Title-"));
        assert!(second.path.starts_with("Same-Title-"));
    }

    #[test]
    fn cleanup_removes_only_orphaned_uploads() {
        let (_directory, store) = test_store();
        std::fs::create_dir_all(&store.upload_dir).unwrap();
        let active = store.upload_dir.join("active.png");
        let orphan = store.upload_dir.join("orphan.png");
        std::fs::write(&active, b"active").unwrap();
        std::fs::write(&orphan, b"orphan").unwrap();
        let account = store.create_account("Cleanup", "", "").unwrap();
        let content: Vec<Node> = serde_json::from_value(json!([{
            "tag": "figure",
            "children": [{
                "tag": "img",
                "attrs": {"src": "http://localhost:8001/file/active.png"}
            }]
        }]))
        .unwrap();
        let page = store
            .create_page(
                &account,
                "Cleanup",
                "",
                "",
                SanitizedContent::new(&content, store.privacy.strip_utm),
            )
            .unwrap();

        cleanup_uploads(&store.conn().unwrap(), &store.upload_dir, Duration::ZERO).unwrap();
        assert!(active.exists());
        assert!(!orphan.exists());

        store
            .conn()
            .unwrap()
            .execute("DELETE FROM pages WHERE id = ?1", [page.id])
            .unwrap();
        cleanup_uploads(&store.conn().unwrap(), &store.upload_dir, Duration::ZERO).unwrap();
        assert!(!active.exists());
    }
}

#[cfg(test)]
mod slug_tests {
    #[test]
    fn slugs_transliterate_unicode_and_limit_length() {
        assert_eq!(super::slugify("Héllö"), "Hello");
        let cjk = super::slugify("科技爱好者周刊");
        assert_ne!(cjk, "Page");
        assert!(cjk.is_ascii());
        assert!(!cjk.starts_with('-'));
        assert!(!cjk.ends_with('-'));
        assert_eq!(super::slugify(&"a".repeat(200)).len(), 120);
        assert_eq!(super::slugify("!!!"), "Page");
    }
}

#[cfg(test)]
mod session_cleanup_tests {
    use super::tests::test_store;
    use super::*;

    #[test]
    fn expired_sessions_are_deleted_and_fresh_sessions_survive() {
        let (_directory, store) = test_store();
        let account = store.create_account("Sessions", "", "").unwrap();
        let expired = store.create_session(account.id).unwrap();
        let fresh = store.create_session(account.id).unwrap();
        let old_timestamp = Utc::now().timestamp() - 40 * 86_400;
        store
            .conn()
            .unwrap()
            .execute(
                "UPDATE sessions SET created_at = ?1 WHERE token = ?2",
                params![old_timestamp, expired],
            )
            .unwrap();

        cleanup_sessions(&store.conn().unwrap(), 30).unwrap();
        assert!(store.account_by_session(&fresh).unwrap().is_some());
        assert!(store.account_by_session(&expired).unwrap().is_none());
    }

    #[test]
    fn zero_day_cleanup_is_disabled() {
        let (_directory, store) = test_store();
        let account = store.create_account("Sessions", "", "").unwrap();
        let session = store.create_session(account.id).unwrap();
        store
            .conn()
            .unwrap()
            .execute("UPDATE sessions SET created_at = 0", [])
            .unwrap();
        cleanup_sessions(&store.conn().unwrap(), 0).unwrap();
        assert!(store.account_by_session(&session).unwrap().is_some());
    }
}

#[cfg(test)]
mod privacy_tests {
    use super::tests::test_store;
    use super::*;
    use serde_json::json;

    #[test]
    fn writes_are_sanitized_before_sqlite_storage() {
        let (_directory, mut store) = test_store();
        store.privacy.strip_utm = true;
        let account = store
            .create_account("Privacy", "", "javascript:alert(1)?utm_source=x")
            .unwrap();
        assert_eq!(account.author_url, "");
        let content: Vec<Node> = serde_json::from_value(json!([{
            "tag": "p",
            "attrs": {"onclick": "alert(1)", "dir": "auto"},
            "children": [
                {"tag":"a","attrs":{"href":"https://example.com/?id=1&utm_source=x","target":"_blank","rel":"opener nofollow","ping":"https://evil.example"},"children":["safe"]},
                {"tag":"a","attrs":{"href":"javascript:alert(1)?utm_source=x"},"children":["unsafe"]}
            ]
        }]))
        .unwrap();
        let page = store
            .create_page(
                &account,
                "Clean",
                "",
                "javascript:alert(1)?utm_source=x",
                SanitizedContent::new(&content, store.privacy.strip_utm),
            )
            .unwrap();
        assert_eq!(page.author_url, "");
        let stored: String = store
            .conn()
            .unwrap()
            .query_row(
                "SELECT content FROM pages WHERE id = ?1",
                [&page.id],
                |row| row.get(0),
            )
            .unwrap();
        for forbidden in ["utm_", "javascript:", "onclick", "ping"] {
            assert!(!stored.contains(forbidden), "stored unsafe {forbidden}");
        }
        assert!(stored.contains("https://example.com/?id=1"));
        assert!(stored.contains("nofollow noopener noreferrer"));

        let updated = store
            .update_page(
                &page,
                "Clean",
                "",
                "https://example.com/?id=2&utm_source=x",
                SanitizedContent::new(&content, store.privacy.strip_utm),
            )
            .unwrap();
        assert_eq!(updated.author_url, "https://example.com/?id=2");
    }
}

#[cfg(test)]
mod input_sanitization_tests {
    use super::tests::test_store;
    use super::*;

    #[test]
    fn account_page_and_report_text_is_normalized_before_storage() {
        let (_directory, store) = test_store();
        let account = store
            .create_account("Pri\0vacy", "A\0uthor", "https://example.com")
            .unwrap();
        assert_eq!(account.short_name, "Privacy");
        assert_eq!(account.author_name, "Author");
        let account = store
            .update_account(
                &account,
                Some("Up\0dated"),
                Some("Na\0me"),
                Some("https://example.com/?id=1"),
            )
            .unwrap();
        assert_eq!(account.short_name, "Updated");
        assert_eq!(account.author_name, "Name");

        let page = store
            .create_page(
                &account,
                "Cle\0an",
                "An\0on",
                "https://example.com",
                SanitizedContent::new(&[], false),
            )
            .unwrap();
        assert_eq!(page.title, "Clean");
        assert_eq!(page.author_name, "Anon");

        let report = SanitizedReport::new(&page.id, "other", "bad\0text").unwrap();
        store.create_report(&report).unwrap();
        let comment: String = store
            .conn()
            .unwrap()
            .query_row(
                "SELECT comment FROM reports WHERE page_id = ?1",
                [&page.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(comment, "badtext");
    }
}

#[cfg(test)]
mod account_cleanup_tests {
    use super::tests::test_store;
    use super::*;
    use rusqlite::params;

    #[test]
    fn cleanup_deletes_only_old_orphaned_accounts() {
        let (_directory, store) = test_store();
        let expired = store.create_account("Expired", "", "").unwrap();
        let fresh = store.create_account("Fresh", "", "").unwrap();
        let owner = store.create_account("Owner", "", "").unwrap();
        let page = store
            .create_page(&owner, "Owned", "", "", SanitizedContent::new(&[], false))
            .unwrap();
        store
            .conn()
            .unwrap()
            .execute(
                "UPDATE accounts SET created_at = ?1 WHERE id IN (?2, ?3)",
                params![Utc::now().timestamp() - 90_000, expired.id, owner.id],
            )
            .unwrap();

        cleanup_accounts(&store.conn().unwrap(), 24).unwrap();
        assert!(
            store
                .account_by_token(&expired.access_token)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .account_by_token(&fresh.access_token)
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .account_by_token(&owner.access_token)
                .unwrap()
                .is_some()
        );
        assert_eq!(
            store.page_by_id(&page.id).unwrap().unwrap().account_id,
            owner.id
        );

        let disabled = store.create_account("Disabled", "", "").unwrap();
        store
            .conn()
            .unwrap()
            .execute(
                "UPDATE accounts SET created_at = 0 WHERE id = ?1",
                [disabled.id],
            )
            .unwrap();
        cleanup_accounts(&store.conn().unwrap(), 0).unwrap();
        assert!(
            store
                .account_by_token(&disabled.access_token)
                .unwrap()
                .is_some()
        );
    }
}

#[cfg(test)]
mod moderation_tests {
    use super::*;

    #[test]
    fn existing_databases_gain_the_hidden_column() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("legacy.db");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE pages (
                   id TEXT PRIMARY KEY,
                   path TEXT NOT NULL UNIQUE,
                   account_id INTEGER NOT NULL REFERENCES accounts(id),
                   title TEXT NOT NULL,
                   author_name TEXT NOT NULL DEFAULT '',
                   author_url TEXT NOT NULL DEFAULT '',
                   content TEXT NOT NULL,
                   created_at INTEGER NOT NULL,
                   updated_at INTEGER NOT NULL,
                   views INTEGER NOT NULL DEFAULT 0,
                   save_hash TEXT NOT NULL
                 );",
            )
            .unwrap();
        drop(connection);

        let config = StoreConfig {
            database: database.to_string_lossy().into_owned(),
            upload_dir: directory.path().join("uploads"),
            ..StoreConfig::default()
        };
        let store = Store::open(&config, false, false).unwrap();
        let hidden: i64 = store
            .conn()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('pages') WHERE name = 'hidden'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(hidden, 1);
    }

    #[test]
    fn reported_pages_include_reasons_counts_views_and_hidden_pages() {
        let (_directory, store) = crate::test_utils::store();
        let account = store.create_account("Reports", "", "").unwrap();
        let pages = ["First", "Second", "Unreported"]
            .iter()
            .map(|title| {
                store.create_page(
                    &account,
                    title,
                    "Author",
                    "",
                    SanitizedContent::new(&[], false),
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        for (page, reason) in [
            (&pages[0], "other"),
            (&pages[0], "illegal_drugs"),
            (&pages[1], "copyright"),
        ] {
            let report = SanitizedReport::new(&page.id, reason, "report").unwrap();
            store.create_report(&report).unwrap();
        }
        store.record_view(&pages[0]).unwrap();
        let page = store.page_by_id(&pages[0].id).unwrap().unwrap();
        store.record_view(&page).unwrap();
        store.set_page_hidden(&pages[0].id, true).unwrap();

        let reported = store.list_reported_pages().unwrap();
        assert_eq!(reported.len(), 2);
        assert_eq!(reported[0].page.id, pages[0].id);
        assert_eq!(reported[0].report_count, 2);
        assert_eq!(reported[0].page.views, 2);
        assert!(reported[0].page.hidden);
        assert!(reported[0].reasons.contains("other"));
        assert!(reported[0].reasons.contains("illegal_drugs"));
        assert_eq!(reported[1].page.id, pages[1].id);
        assert_eq!(reported[1].report_count, 1);
        assert!(reported.iter().all(|report| report.page.id != pages[2].id));
    }
}
