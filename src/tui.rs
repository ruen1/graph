use crate::db::Store;
use crate::model::{Node, NodeElement, Page, ReportedPage};
use chrono::{DateTime, Utc};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Row, Table, TableState, Wrap};
use ratatui::{Frame, Terminal};
use std::error::Error;
use std::io::{self, Stdout};
use std::sync::Arc;
use std::time::Duration;

pub fn run(store: Arc<Store>) -> Result<(), Box<dyn Error>> {
    let mut app = App::new(store.list_pages()?).with_reports(store.list_reported_pages()?);
    let mut session = TerminalSession::new()?;
    loop {
        session.terminal.draw(|frame| app.draw(frame))?;
        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
            && app.handle_key(key, &store)
        {
            break;
        }
    }
    Ok(())
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalSession {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        let terminal = Terminal::new(CrosstermBackend::new(stdout));
        match terminal {
            Ok(terminal) => Ok(Self { terminal }),
            Err(error) => {
                let _ = disable_raw_mode();
                let _ = execute!(io::stdout(), LeaveAlternateScreen);
                Err(error)
            }
        }
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

#[derive(Clone, Copy)]
enum Panel {
    Pages,
    Reports,
}

struct App {
    pages: Vec<Page>,
    reports: Vec<ReportedPage>,
    panel: Panel,
    table_state: TableState,
    report_state: TableState,
    post_scroll: u16,
    status: Option<String>,
    pending_delete: Option<String>,
}

impl App {
    fn new(pages: Vec<Page>) -> Self {
        let mut table_state = TableState::default();
        if !pages.is_empty() {
            table_state.select(Some(0));
        }
        Self {
            pages,
            reports: Vec::new(),
            panel: Panel::Pages,
            table_state,
            report_state: TableState::default(),
            post_scroll: 0,
            status: None,
            pending_delete: None,
        }
    }

    fn with_reports(mut self, reports: Vec<ReportedPage>) -> Self {
        if !reports.is_empty() {
            self.report_state.select(Some(0));
        }
        self.reports = reports;
        self
    }

    fn selected_index(&self) -> Option<usize> {
        match self.panel {
            Panel::Pages => self.table_state.selected(),
            Panel::Reports => self.report_state.selected(),
        }
    }

    fn selected_page(&self) -> Option<&Page> {
        let index = self.selected_index()?;
        match self.panel {
            Panel::Pages => self.pages.get(index),
            Panel::Reports => self.reports.get(index).map(|report| &report.page),
        }
    }

    fn current_len(&self) -> usize {
        match self.panel {
            Panel::Pages => self.pages.len(),
            Panel::Reports => self.reports.len(),
        }
    }

    fn select(&mut self, index: usize) {
        match self.panel {
            Panel::Pages => select_row(&mut self.table_state, self.pages.len(), index),
            Panel::Reports => select_row(&mut self.report_state, self.reports.len(), index),
        }
        self.post_scroll = 0;
        self.pending_delete = None;
    }

    fn next(&mut self) {
        let index = self.selected_index().unwrap_or(0);
        self.select(index.saturating_add(1));
    }

    fn previous(&mut self) {
        let index = self.selected_index().unwrap_or(0);
        self.select(index.saturating_sub(1));
    }

    fn toggle_panel(&mut self) {
        self.panel = match self.panel {
            Panel::Pages => Panel::Reports,
            Panel::Reports => Panel::Pages,
        };
        self.post_scroll = 0;
        self.pending_delete = None;
        self.status = Some(match self.panel {
            Panel::Pages => "Pages panel".into(),
            Panel::Reports => "Reports panel".into(),
        });
    }

    fn reload(&mut self, store: &Store) {
        let page_id = self
            .table_state
            .selected()
            .and_then(|index| self.pages.get(index))
            .map(|page| page.id.clone());
        let report_id = self
            .report_state
            .selected()
            .and_then(|index| self.reports.get(index))
            .map(|report| report.page.id.clone());
        let result = store
            .list_pages()
            .and_then(|pages| store.list_reported_pages().map(|reports| (pages, reports)));
        match result {
            Ok((pages, reports)) => {
                let page_index = page_id
                    .and_then(|id| pages.iter().position(|page| page.id == id))
                    .unwrap_or(0);
                let report_index = report_id
                    .and_then(|id| reports.iter().position(|report| report.page.id == id))
                    .unwrap_or(0);
                self.pages = pages;
                self.reports = reports;
                select_row(&mut self.table_state, self.pages.len(), page_index);
                select_row(&mut self.report_state, self.reports.len(), report_index);
                self.post_scroll = 0;
                self.pending_delete = None;
                self.status = Some(format!(
                    "Reloaded {} pages and {} reported pages",
                    self.pages.len(),
                    self.reports.len()
                ));
            }
            Err(error) => self.status = Some(format!("Reload failed: {error}")),
        }
    }

    fn toggle_hidden(&mut self, store: &Store) {
        let Some(page) = self.selected_page() else {
            self.status = Some("No page selected".into());
            return;
        };
        let (id, title, hidden) = (page.id.clone(), page.title.clone(), !page.hidden);
        match store.set_page_hidden(&id, hidden) {
            Ok(()) => {
                for page in self
                    .pages
                    .iter_mut()
                    .chain(self.reports.iter_mut().map(|report| &mut report.page))
                    .filter(|page| page.id == id)
                {
                    page.hidden = hidden;
                }
                self.status = Some(if hidden {
                    format!("Hidden \"{title}\" from the public")
                } else {
                    format!("Restored \"{title}\" to the public")
                });
            }
            Err(error) => self.status = Some(format!("Could not change visibility: {error}")),
        }
    }

    fn delete_selected(&mut self, store: &Store) {
        let Some(page) = self.selected_page() else {
            self.status = Some("No page selected".into());
            return;
        };
        let (id, title) = (page.id.clone(), page.title.clone());
        if self.pending_delete.as_deref() != Some(id.as_str()) {
            self.pending_delete = Some(id);
            self.status = Some(format!(
                "Delete \"{title}\" forever? Ctrl+D to confirm; Esc to cancel"
            ));
            return;
        }
        match store.delete_page(&id) {
            Ok(()) => {
                let page_index = self.table_state.selected().unwrap_or(0);
                let report_index = self.report_state.selected().unwrap_or(0);
                self.pages.retain(|page| page.id != id);
                self.reports.retain(|report| report.page.id != id);
                select_row(&mut self.table_state, self.pages.len(), page_index);
                select_row(&mut self.report_state, self.reports.len(), report_index);
                self.post_scroll = 0;
                self.pending_delete = None;
                self.status = Some(format!("Deleted \"{title}\" forever"));
            }
            Err(error) => {
                self.pending_delete = None;
                self.status = Some(format!("Could not delete page: {error}"));
            }
        }
    }

    fn discard_reports(&mut self, store: &Store) {
        if !matches!(self.panel, Panel::Reports) {
            self.status = Some("Press m to open Reports before discarding reports".into());
            return;
        }
        let Some(page) = self.selected_page() else {
            self.status = Some("No reported page selected".into());
            return;
        };
        let (id, title) = (page.id.clone(), page.title.clone());
        match store.discard_reports(&id) {
            Ok(count) => {
                let index = self.report_state.selected().unwrap_or(0);
                self.reports.retain(|report| report.page.id != id);
                select_row(&mut self.report_state, self.reports.len(), index);
                self.post_scroll = 0;
                self.pending_delete = None;
                let noun = if count == 1 { "report" } else { "reports" };
                self.status = Some(format!("Discarded {count} {noun} for \"{title}\""));
            }
            Err(error) => self.status = Some(format!("Could not discard reports: {error}")),
        }
    }

    fn handle_key(&mut self, key: KeyEvent, store: &Store) -> bool {
        if key.kind != KeyEventKind::Press {
            return false;
        }
        if key.code == KeyCode::Backspace {
            self.pending_delete = None;
            self.toggle_hidden(store);
            return false;
        }
        if key.code == KeyCode::Tab {
            self.pending_delete = None;
            self.discard_reports(store);
            return false;
        }
        let control = crossterm::event::KeyModifiers::CONTROL;
        if key.modifiers.contains(control) {
            match key.code {
                KeyCode::Char('h') => {
                    self.pending_delete = None;
                    self.toggle_hidden(store);
                    return false;
                }
                KeyCode::Char('d') => {
                    self.delete_selected(store);
                    return false;
                }
                KeyCode::Char('i') => {
                    self.pending_delete = None;
                    self.discard_reports(store);
                    return false;
                }
                _ => {}
            }
        }
        if key.code == KeyCode::Esc && self.pending_delete.take().is_some() {
            self.status = Some("Deletion cancelled".into());
            return false;
        }
        if self.pending_delete.take().is_some() {
            self.status = Some("Deletion cancelled".into());
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => return true,
            KeyCode::Down | KeyCode::Char('j') => self.next(),
            KeyCode::Up | KeyCode::Char('k') => self.previous(),
            KeyCode::Home => self.select(0),
            KeyCode::End => self.select(self.current_len().saturating_sub(1)),
            KeyCode::PageDown => self.post_scroll = self.post_scroll.saturating_add(5),
            KeyCode::PageUp => self.post_scroll = self.post_scroll.saturating_sub(5),
            KeyCode::Char('r') => self.reload(store),
            KeyCode::Char('m') => self.toggle_panel(),
            _ => {}
        }
        false
    }

    fn draw(&mut self, frame: &mut Frame) {
        let vertical =
            Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(frame.area());
        let panes = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(vertical[0]);

        match self.panel {
            Panel::Pages => {
                let rows = self.pages.iter().map(|page| {
                    Row::new(vec![
                        page_date(page).format("%Y-%m-%d").to_string(),
                        display_title(page),
                        display_author(page),
                        page.views.to_string(),
                        page.content_char_count().to_string(),
                    ])
                });
                let table = Table::new(
                    rows,
                    [
                        Constraint::Length(12),
                        Constraint::Fill(3),
                        Constraint::Fill(1),
                        Constraint::Length(7),
                        Constraint::Length(7),
                    ],
                )
                .header(
                    Row::new(vec!["Date", "Title", "Author", "Views", "Chars"])
                        .style(Style::default().add_modifier(Modifier::BOLD)),
                )
                .block(Block::bordered().title(format!(" Pages ({}) ", self.pages.len())))
                .row_highlight_style(selected_row_style())
                .highlight_symbol(" ");
                frame.render_stateful_widget(table, panes[0], &mut self.table_state);
            }
            Panel::Reports => {
                let rows = self.reports.iter().map(|report| {
                    Row::new(vec![
                        display_reasons(report),
                        display_title(&report.page),
                        display_author(&report.page),
                        report.page.views.to_string(),
                        report.report_count.to_string(),
                    ])
                });
                let table = Table::new(
                    rows,
                    [
                        Constraint::Fill(3),
                        Constraint::Fill(2),
                        Constraint::Fill(1),
                        Constraint::Length(7),
                        Constraint::Length(8),
                    ],
                )
                .header(
                    Row::new(vec!["Reason", "Title", "Author", "Views", "Reports"])
                        .style(Style::default().add_modifier(Modifier::BOLD)),
                )
                .block(Block::bordered().title(format!(" Reports ({}) ", self.reports.len())))
                .row_highlight_style(selected_row_style())
                .highlight_symbol(" ");
                frame.render_stateful_widget(table, panes[0], &mut self.report_state);
            }
        }

        let mut lines = self.selected_page().map(post_lines).unwrap_or_else(|| {
            vec![Line::from(Span::styled(
                match self.panel {
                    Panel::Pages if self.pages.is_empty() => "No pages yet.",
                    Panel::Reports if self.reports.is_empty() => "No reported pages.",
                    _ => "Select a page.",
                },
                Style::default().fg(Color::DarkGray),
            ))]
        });
        let viewport_height = panes[1].height.saturating_sub(2) as usize;
        let max_scroll = lines.len().saturating_sub(viewport_height);
        self.post_scroll = self.post_scroll.min(max_scroll as u16);
        let preview = Paragraph::new(std::mem::take(&mut lines))
            .block(Block::bordered().title(" Page contents "))
            .wrap(Wrap { trim: false })
            .scroll((self.post_scroll, 0));
        frame.render_widget(preview, panes[1]);

        let footer = self.status.clone().unwrap_or_else(|| {
            "Up/Down or j/k: select | PgUp/PgDn: scroll | m: panels | r: reload".into()
        });
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(footer),
                Line::from(
                    "Ctrl+H: hide/restore | Ctrl+D: delete | Ctrl+I: discard reports | q/Esc: quit",
                ),
            ])
            .style(Style::default().fg(Color::DarkGray)),
            vertical[1],
        );
    }
}

fn select_row(state: &mut TableState, len: usize, index: usize) {
    if len == 0 {
        state.select(None);
    } else {
        state.select(Some(index.min(len - 1)));
    }
}

fn selected_row_style() -> Style {
    Style::default()
        .fg(Color::Black)
        .bg(Color::Gray)
        .add_modifier(Modifier::BOLD)
}

fn display_title(page: &Page) -> String {
    if page.hidden {
        format!("[hidden] {}", page.title)
    } else {
        page.title.clone()
    }
}

fn display_author(page: &Page) -> String {
    if page.author_name.is_empty() {
        "Anonymous".into()
    } else {
        page.author_name.clone()
    }
}

fn display_reasons(report: &ReportedPage) -> String {
    report
        .reasons
        .split(',')
        .map(|reason| match reason {
            "childabuse" => "child abuse".into(),
            "illegal_drugs" => "illegal drugs".into(),
            "personal_details" => "personal details".into(),
            other => other.replace('_', " "),
        })
        .collect::<Vec<String>>()
        .join(", ")
}

fn page_date(page: &Page) -> DateTime<Utc> {
    DateTime::from_timestamp(page.created_at, 0)
        .or_else(|| DateTime::from_timestamp(0, 0))
        .expect("the Unix epoch is a valid date")
}

fn post_lines(page: &Page) -> Vec<Line<'static>> {
    let author = if page.author_name.is_empty() {
        "Anonymous"
    } else {
        &page.author_name
    };
    let visibility = if page.hidden { " · HIDDEN" } else { "" };
    let mut lines = vec![
        Line::from(Span::styled(
            page.title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!(
                "{author} · {}{visibility}",
                page_date(page).format("%b %d, %Y")
            ),
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            format!("/{}", page.path),
            Style::default().fg(Color::DarkGray),
        )),
        Line::default(),
    ];
    for node in &page.content {
        append_block(node, &mut lines);
        lines.push(Line::default());
    }
    if page.content.is_empty() {
        lines.push(Line::from(Span::styled(
            "This page has no content.",
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines
}

fn append_block(node: &Node, lines: &mut Vec<Line<'static>>) {
    match node {
        Node::Text(text) => push_text(lines, text, Style::default()),
        Node::Element(element) => match element.tag.as_str() {
            "h3" | "h4" => push_text(
                lines,
                &inline_text(element.children.as_deref().unwrap_or_default()),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            "blockquote" => push_text(
                lines,
                &format!(
                    "| {}",
                    inline_text(element.children.as_deref().unwrap_or_default())
                ),
                Style::default().add_modifier(Modifier::ITALIC),
            ),
            "pre" => push_text(
                lines,
                &inline_text(element.children.as_deref().unwrap_or_default()),
                Style::default().fg(Color::Cyan),
            ),
            "ol" | "ul" => {
                let ordered = element.tag == "ol";
                for (index, child) in element
                    .children
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .enumerate()
                {
                    let prefix = if ordered {
                        format!("{}. ", index + 1)
                    } else {
                        "- ".into()
                    };
                    push_text(
                        lines,
                        &format!("{prefix}{}", inline_text(std::slice::from_ref(child))),
                        Style::default(),
                    );
                }
            }
            "figure" => {
                for child in element.children.as_deref().unwrap_or_default() {
                    append_block(child, lines);
                }
            }
            "img" | "video" | "iframe" => {
                let label = match element.tag.as_str() {
                    "img" => "Image",
                    "video" => "Video",
                    _ => "Embed",
                };
                let source = attribute(element, "src").unwrap_or("");
                lines.push(Line::from(format!("[{label}: {source}]")));
            }
            "hr" => lines.push(Line::from("--------------------------------")),
            "br" => lines.push(Line::default()),
            _ => push_text(lines, &inline_text(&[node.clone()]), Style::default()),
        },
    }
}

fn attribute<'a>(element: &'a NodeElement, name: &str) -> Option<&'a str> {
    element.attrs.as_ref()?.get(name).map(String::as_str)
}

fn inline_text(nodes: &[Node]) -> String {
    nodes
        .iter()
        .map(|node| match node {
            Node::Text(text) => text.clone(),
            Node::Element(element) if element.tag == "br" => "\n".into(),
            Node::Element(element) => inline_text(element.children.as_deref().unwrap_or_default()),
        })
        .collect()
}

fn push_text(lines: &mut Vec<Line<'static>>, text: &str, style: Style) {
    for part in text.split('\n') {
        lines.push(Line::from(Span::styled(part.to_owned(), style)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::sanitize::SanitizedContent;
    use ratatui::backend::TestBackend;
    use std::collections::BTreeMap;

    fn page(id: &str, title: &str, created_at: i64, content: Vec<Node>) -> Page {
        Page {
            id: id.into(),
            path: format!("{id}-01-01"),
            account_id: 1,
            title: title.into(),
            author_name: "Author".into(),
            author_url: String::new(),
            content,
            created_at,
            updated_at: created_at,
            views: 0,
            save_hash: String::new(),
            hidden: false,
        }
    }

    fn text(value: &str) -> Node {
        Node::Text(value.into())
    }

    fn element(tag: &str, children: Vec<Node>) -> Node {
        Node::Element(NodeElement {
            tag: tag.into(),
            attrs: None,
            children: Some(children),
        })
    }

    #[test]
    fn keyboard_navigation_changes_selection_and_post_scroll() {
        let (_directory, store) = crate::test_utils::store();
        let mut app = App::new(vec![
            page("first", "First", 1, vec![]),
            page("second", "Second", 2, vec![]),
        ]);
        assert_eq!(app.selected_page().unwrap().id, "first");
        app.post_scroll = 10;
        assert!(!app.handle_key(KeyEvent::from(KeyCode::Down), &store));
        assert_eq!(app.selected_page().unwrap().id, "second");
        assert_eq!(app.post_scroll, 0);
        app.next();
        assert_eq!(app.selected_page().unwrap().id, "second");
        app.previous();
        assert_eq!(app.selected_page().unwrap().id, "first");
        assert!(!app.handle_key(KeyEvent::from(KeyCode::End), &store));
        assert_eq!(app.selected_page().unwrap().id, "second");
        assert!(!app.handle_key(KeyEvent::from(KeyCode::Home), &store));
        assert_eq!(app.selected_page().unwrap().id, "first");
        assert!(!app.handle_key(KeyEvent::from(KeyCode::PageDown), &store));
        assert_eq!(app.post_scroll, 5);
        assert!(!app.handle_key(KeyEvent::from(KeyCode::PageUp), &store));
        assert_eq!(app.post_scroll, 0);
        assert!(app.handle_key(KeyEvent::from(KeyCode::Char('q')), &store));
    }

    #[test]
    fn reload_reads_new_pages_from_the_database() {
        let (_directory, store) = crate::test_utils::store();
        let account = store.create_account("TUI", "", "").unwrap();
        let content = SanitizedContent::new(&[element("p", vec![text("From SQLite")])], false);
        store
            .create_page(&account, "Database page", "Author", "", content)
            .unwrap();
        let mut app = App::new(vec![]);
        app.reload(&store);
        assert_eq!(app.pages.len(), 1);
        assert_eq!(app.selected_page().unwrap().title, "Database page");
        assert_eq!(
            app.status.as_deref(),
            Some("Reloaded 1 pages and 0 reported pages")
        );
    }

    #[test]
    fn post_lines_preserve_blocks_breaks_lists_and_media() {
        let image = NodeElement {
            tag: "img".into(),
            attrs: Some(BTreeMap::from([("src".into(), "/file/image.png".into())])),
            children: None,
        };
        let content = vec![
            element("h3", vec![text("Heading")]),
            element(
                "p",
                vec![text("First"), element("br", vec![]), text("Second")],
            ),
            element("ol", vec![element("li", vec![text("One")])]),
            element("blockquote", vec![text("Quote")]),
            Node::Element(image),
        ];
        let lines = post_lines(&page("post", "Title", 1, content));
        let text = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        for expected in [
            "Title",
            "Heading",
            "First",
            "Second",
            "1. One",
            "| Quote",
            "[Image: /file/image.png]",
        ] {
            assert!(text.contains(expected), "missing {expected:?}");
        }
    }

    #[test]
    fn terminal_frame_contains_table_and_selected_post() {
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = App::new(vec![
            page(
                "first",
                "First page",
                1,
                vec![element("p", vec![text("First body")])],
            ),
            page("second", "Second page", 2, vec![]),
        ]);
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut output = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                output.push_str(buffer[(x, y)].symbol());
            }
            output.push('\n');
        }
        for expected in [
            "Date",
            "Title",
            "Author",
            "Views",
            "Chars",
            "First page",
            "Second page",
            "First body",
            "/first-01-01",
            "10",
            "Page contents",
        ] {
            assert!(output.contains(expected), "missing {expected:?}");
        }
    }

    #[test]
    fn moderation_toggles_visibility_and_deletes_only_after_confirmation() {
        let (_directory, store) = crate::test_utils::store();
        let account = store.create_account("TUI", "", "").unwrap();
        let page = store
            .create_page(
                &account,
                "Moderate me",
                "Author",
                "",
                SanitizedContent::new(&[element("p", vec![text("Body")])], false),
            )
            .unwrap();
        let id = page.id.clone();
        let path = page.path.clone();
        store.record_view(&page).unwrap();
        let report =
            crate::handlers::sanitize::SanitizedReport::new(&id, "other", "report").unwrap();
        store.create_report(&report).unwrap();
        let mut app = App::new(vec![page]);
        let control = crossterm::event::KeyModifiers::CONTROL;

        assert!(!app.handle_key(KeyEvent::new(KeyCode::Char('h'), control), &store));
        assert!(store.page_by_path(&path).unwrap().is_none());
        assert!(store.page_by_id(&id).unwrap().unwrap().hidden);
        assert!(app.selected_page().unwrap().hidden);
        assert!(!app.handle_key(KeyEvent::from(KeyCode::Backspace), &store));
        assert!(store.page_by_path(&path).unwrap().is_some());
        assert!(!app.selected_page().unwrap().hidden);

        assert!(!app.handle_key(KeyEvent::new(KeyCode::Char('d'), control), &store));
        assert_eq!(app.pending_delete.as_deref(), Some(id.as_str()));
        assert!(store.page_by_id(&id).unwrap().is_some());
        assert!(!app.handle_key(KeyEvent::from(KeyCode::Esc), &store));
        assert!(app.pending_delete.is_none());
        assert!(store.page_by_id(&id).unwrap().is_some());

        assert!(!app.handle_key(KeyEvent::new(KeyCode::Char('d'), control), &store));
        assert!(store.page_by_id(&id).unwrap().is_some());
        assert!(!app.handle_key(KeyEvent::new(KeyCode::Char('d'), control), &store));
        assert!(store.page_by_id(&id).unwrap().is_none());
        assert!(app.pages.is_empty());
        assert_eq!(
            app.status.as_deref(),
            Some("Deleted \"Moderate me\" forever")
        );

        let connection = store.conn().unwrap();
        let related_rows: i64 = connection
            .query_row(
                "SELECT (SELECT COUNT(*) FROM view_events WHERE page_id = ?1) + (SELECT COUNT(*) FROM reports WHERE page_id = ?1)",
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(related_rows, 0);
    }

    #[test]
    fn reports_panel_renders_and_supports_moderation_commands() {
        let (_directory, store) = crate::test_utils::store();
        let account = store.create_account("TUI", "", "").unwrap();
        let page = store
            .create_page(
                &account,
                "Reported page",
                "Reporter",
                "",
                SanitizedContent::new(&[element("p", vec![text("Reported body")])], false),
            )
            .unwrap();
        let id = page.id.clone();
        for reason in ["other", "illegal_drugs"] {
            let report =
                crate::handlers::sanitize::SanitizedReport::new(&id, reason, "report").unwrap();
            store.create_report(&report).unwrap();
        }
        store.record_view(&page).unwrap();
        let mut app = App::new(store.list_pages().unwrap())
            .with_reports(store.list_reported_pages().unwrap());
        assert!(!app.handle_key(KeyEvent::from(KeyCode::Char('m')), &store));
        assert!(matches!(app.panel, Panel::Reports));
        assert_eq!(app.selected_page().unwrap().id, id);
        assert_eq!(app.reports[0].report_count, 2);
        assert_eq!(app.reports[0].page.views, 1);

        let backend = TestBackend::new(140, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut output = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                output.push_str(buffer[(x, y)].symbol());
            }
            output.push('\n');
        }
        for expected in [
            "Reason",
            "Title",
            "Author",
            "Views",
            "Reports",
            "illegal drugs",
            "Reported page",
            "Reported body",
        ] {
            assert!(output.contains(expected), "missing {expected:?}\n{output}");
        }

        let control = crossterm::event::KeyModifiers::CONTROL;
        assert!(!app.handle_key(KeyEvent::new(KeyCode::Char('h'), control), &store));
        assert!(app.pages[0].hidden);
        assert!(app.reports[0].page.hidden);
        assert!(!app.handle_key(KeyEvent::from(KeyCode::Char('m')), &store));
        assert!(matches!(app.panel, Panel::Pages));
        assert!(app.selected_page().unwrap().hidden);
        assert!(!app.handle_key(KeyEvent::from(KeyCode::Char('m')), &store));
        assert!(!app.handle_key(KeyEvent::new(KeyCode::Char('d'), control), &store));
        assert!(store.page_by_id(&id).unwrap().is_some());
        assert!(!app.handle_key(KeyEvent::new(KeyCode::Char('d'), control), &store));
        assert!(store.page_by_id(&id).unwrap().is_none());
        assert!(app.pages.is_empty());
        assert!(app.reports.is_empty());
    }

    #[test]
    fn ctrl_i_and_tab_discard_only_reports() {
        let (_directory, store) = crate::test_utils::store();
        let account = store.create_account("TUI", "", "").unwrap();
        let first = store
            .create_page(
                &account,
                "First reported",
                "Author",
                "",
                SanitizedContent::new(&[], false),
            )
            .unwrap();
        let second = store
            .create_page(
                &account,
                "Second reported",
                "Author",
                "",
                SanitizedContent::new(&[], false),
            )
            .unwrap();
        for (page, reason) in [
            (&first, "other"),
            (&first, "violence"),
            (&second, "copyright"),
        ] {
            let report =
                crate::handlers::sanitize::SanitizedReport::new(&page.id, reason, "report")
                    .unwrap();
            store.create_report(&report).unwrap();
        }
        store.record_view(&first).unwrap();
        store.set_page_hidden(&first.id, true).unwrap();
        let mut app = App::new(store.list_pages().unwrap())
            .with_reports(store.list_reported_pages().unwrap());
        app.toggle_panel();
        assert_eq!(app.selected_page().unwrap().id, first.id);
        let control = crossterm::event::KeyModifiers::CONTROL;

        assert!(!app.handle_key(KeyEvent::new(KeyCode::Char('i'), control), &store));
        assert_eq!(app.reports.len(), 1);
        assert_eq!(app.pages.len(), 2);
        assert_eq!(
            app.status.as_deref(),
            Some("Discarded 2 reports for \"First reported\"")
        );
        assert!(store.page_by_id(&first.id).unwrap().unwrap().hidden);
        assert_eq!(store.page_by_id(&first.id).unwrap().unwrap().views, 1);
        assert!(app.reports.iter().all(|report| report.page.id != first.id));

        assert!(!app.handle_key(KeyEvent::from(KeyCode::Tab), &store));
        assert!(app.reports.is_empty());
        assert_eq!(app.pages.len(), 2);
        assert!(store.page_by_id(&first.id).unwrap().is_some());
        assert!(store.page_by_id(&second.id).unwrap().is_some());
        let remaining: i64 = store
            .conn()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM reports", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 0);
    }
}
