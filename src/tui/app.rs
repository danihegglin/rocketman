//! Terminal UI state and input handling. Collections, drafts and every file
//! operation go through the shared [`Session`]; this module only adds the
//! editor, the response view and keyboard navigation.

use crate::text::{Edit, LineInput, TextArea};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;
use rocketman::http::{self, Response};
use rocketman::openapi::{Method, Param, RequestSpec, path_templates};
use rocketman::session::{BODY_TYPES, DraftKey, Notice, Row, Session};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

const NOTICE_TTL: Duration = Duration::from_secs(6);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Sidebar,
    Request,
    Response,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReqTab {
    Params,
    Headers,
    Body,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Table {
    Path,
    Query,
    Headers,
}

/// The focused control in the request editor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    Name,
    Method,
    Url,
    Tabs,
    Cell { table: Table, row: usize, value: bool },
    ContentType,
    Body,
}

impl Field {
    /// Same control, ignoring which half of a key/value row is focused.
    fn same_row(self, other: Field) -> bool {
        match (self, other) {
            (Field::Cell { table: a, row: r, .. }, Field::Cell { table: b, row: s, .. }) => {
                a == b && r == s
            }
            (a, b) => a == b,
        }
    }
}

/// Editable key/value rows. Unless the keys are fixed (path variables), there
/// is always one empty row at the end to type a new entry into.
#[derive(Default)]
pub struct KvTable {
    pub rows: Vec<(LineInput, LineInput)>,
    fixed: bool,
}

impl KvTable {
    fn new(fixed: bool) -> Self {
        let mut t = KvTable { rows: Vec::new(), fixed };
        t.set(&[]);
        t
    }

    fn set(&mut self, params: &[Param]) {
        self.rows = params
            .iter()
            .map(|p| (LineInput::new(&p.name), LineInput::new(&p.value)))
            .collect();
        if !self.fixed {
            self.rows.push(Default::default());
        }
    }

    pub fn params(&self) -> Vec<Param> {
        self.rows
            .iter()
            .map(|(k, v)| Param::new(k.value(), v.value()))
            .filter(|p| !p.name.trim().is_empty())
            .collect()
    }

    pub fn filled(&self) -> usize {
        self.rows.iter().filter(|(k, _)| !k.value().trim().is_empty()).count()
    }

    fn after_edit(&mut self) {
        if !self.fixed && self.rows.last().is_none_or(|(k, v)| !k.is_empty() || !v.is_empty()) {
            self.rows.push(Default::default());
        }
    }

    /// Removes a row; the trailing empty row can't be removed.
    fn remove(&mut self, ix: usize) -> bool {
        if self.fixed || ix + 1 >= self.rows.len() {
            return false;
        }
        self.rows.remove(ix);
        self.after_edit();
        true
    }
}

pub struct Editor {
    pub name: LineInput,
    pub method: Method,
    pub url: LineInput,
    pub path: KvTable,
    pub query: KvTable,
    pub headers: KvTable,
    pub content_type: String,
    pub body: TextArea,
    pub tab: ReqTab,
    pub field: Field,
}

impl Default for Editor {
    fn default() -> Self {
        Editor {
            name: LineInput::default(),
            method: Method::Get,
            url: LineInput::default(),
            path: KvTable::new(true),
            query: KvTable::new(false),
            headers: KvTable::new(false),
            content_type: String::new(),
            body: TextArea::default(),
            tab: ReqTab::Params,
            field: Field::Url,
        }
    }
}

impl Editor {
    fn load(&mut self, spec: &RequestSpec) {
        self.name.set(&spec.name);
        self.method = spec.method;
        self.url.set(&spec.url);
        self.path.set(&spec.path_params);
        self.query.set(&spec.query);
        self.headers.set(&spec.headers);
        self.content_type = spec.content_type.clone();
        self.body.set(&spec.body);
        if spec.content_type.is_empty() && self.tab == ReqTab::Body && !spec.method.has_body() {
            self.tab = ReqTab::Params;
        }
        if !self.fields().iter().any(|f| f.same_row(self.field)) {
            self.field = Field::Url;
        }
    }

    pub fn snapshot(&self) -> RequestSpec {
        RequestSpec {
            name: self.name.value().to_string(),
            method: self.method,
            url: self.url.value().to_string(),
            path_params: self.path.params(),
            query: self.query.params(),
            headers: self.headers.params(),
            content_type: self.content_type.clone(),
            body: if self.content_type.is_empty() {
                String::new()
            } else {
                self.body.text()
            },
        }
    }

    /// Keeps the path variable rows in step with the `{var}`s in the URL.
    fn sync_path_params(&mut self) {
        let names = path_templates(self.url.value());
        let current = self.path.params();
        if current.iter().map(|p| &p.name).eq(names.iter()) {
            return;
        }
        let params: Vec<_> = names
            .into_iter()
            .map(|n| {
                let value = current
                    .iter()
                    .find(|p| p.name == n)
                    .map(|p| p.value.clone())
                    .unwrap_or_default();
                Param::new(n, value)
            })
            .collect();
        self.path.set(&params);
    }

    pub fn table(&self, table: Table) -> &KvTable {
        match table {
            Table::Path => &self.path,
            Table::Query => &self.query,
            Table::Headers => &self.headers,
        }
    }

    fn table_mut(&mut self, table: Table) -> &mut KvTable {
        match table {
            Table::Path => &mut self.path,
            Table::Query => &mut self.query,
            Table::Headers => &mut self.headers,
        }
    }

    /// Focusable controls, top to bottom.
    pub fn fields(&self) -> Vec<Field> {
        let mut fields = vec![Field::Name, Field::Method, Field::Url, Field::Tabs];
        let cells = |table: Table, t: &KvTable| {
            (0..t.rows.len()).map(move |row| Field::Cell {
                table,
                row,
                value: table == Table::Path,
            })
        };
        match self.tab {
            ReqTab::Params => {
                fields.extend(cells(Table::Path, &self.path));
                fields.extend(cells(Table::Query, &self.query));
            }
            ReqTab::Headers => fields.extend(cells(Table::Headers, &self.headers)),
            ReqTab::Body => {
                fields.push(Field::ContentType);
                if !self.content_type.is_empty() {
                    fields.push(Field::Body);
                }
            }
        }
        fields
    }

    fn move_focus(&mut self, delta: isize) {
        let fields = self.fields();
        let Some(ix) = fields.iter().position(|f| f.same_row(self.field)) else {
            self.field = Field::Url;
            return;
        };
        let next = ix.saturating_add_signed(delta).min(fields.len() - 1);
        let mut field = fields[next];
        // Stay in the same column when moving between key/value rows.
        if let (Field::Cell { table: from, value, .. }, Field::Cell { table, row, .. }) =
            (self.field, field)
        {
            field = Field::Cell { table, row, value: (from == table && value) || table == Table::Path };
        }
        self.field = field;
    }

    fn cycle_tab(&mut self, delta: isize) {
        let tabs = [ReqTab::Params, ReqTab::Headers, ReqTab::Body];
        let ix = tabs.iter().position(|t| *t == self.tab).unwrap_or(0);
        self.tab = tabs[(ix as isize + delta).rem_euclid(3) as usize];
    }

    fn cycle_method(&mut self, delta: isize) {
        let all = Method::PICKER;
        let ix = all.iter().position(|m| *m == self.method).map_or(-1, |i| i as isize);
        let ix = if ix < 0 && delta < 0 { 0 } else { ix + delta };
        self.method = all[ix.rem_euclid(all.len() as isize) as usize];
    }

    fn cycle_content_type(&mut self, delta: isize) {
        let mut options: Vec<&str> = std::iter::once("")
            .chain(BODY_TYPES.iter().map(|(_, v)| *v))
            .collect();
        // A custom type from the file stays selectable.
        let custom = self.content_type.clone();
        if !options.contains(&custom.as_str()) {
            options.push(&custom);
        }
        let ix = options.iter().position(|v| *v == custom).unwrap_or(0) as isize;
        let next = options[(ix + delta).rem_euclid(options.len() as isize) as usize].to_string();
        self.content_type = next;
    }

    /// Handles a key in the request pane. Returns whether the request changed,
    /// or `None` when the key wasn't used.
    fn handle_key(&mut self, key: KeyEvent) -> Option<bool> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match self.field {
            Field::Name | Field::Url => {
                let url = self.field == Field::Url;
                let input = if url { &mut self.url } else { &mut self.name };
                match input.handle_key(key) {
                    Edit::Changed => {
                        if url {
                            self.sync_path_params();
                        }
                        return Some(true);
                    }
                    Edit::Moved => return Some(false),
                    Edit::Ignored => {}
                }
                if key.code == KeyCode::Enter && !url {
                    self.move_focus(1);
                    return Some(false);
                }
            }
            Field::Method => match key.code {
                KeyCode::Left | KeyCode::Char('h') => {
                    self.cycle_method(-1);
                    return Some(true);
                }
                KeyCode::Right | KeyCode::Char('l' | ' ') | KeyCode::Enter => {
                    self.cycle_method(1);
                    return Some(true);
                }
                KeyCode::Char(c) if !ctrl => {
                    // Type the first letter; repeat to step through POST, PUT, PATCH.
                    let c = c.to_ascii_uppercase();
                    let matching: Vec<_> =
                        Method::PICKER.into_iter().filter(|m| m.as_str().starts_with(c)).collect();
                    if let Some(first) = matching.first() {
                        let ix = matching.iter().position(|m| *m == self.method);
                        self.method = ix.map_or(*first, |i| matching[(i + 1) % matching.len()]);
                        return Some(true);
                    }
                }
                _ => {}
            },
            Field::Tabs => match key.code {
                KeyCode::Left | KeyCode::Char('h') => {
                    self.cycle_tab(-1);
                    return Some(false);
                }
                KeyCode::Right | KeyCode::Char('l' | ' ') => {
                    self.cycle_tab(1);
                    return Some(false);
                }
                KeyCode::Enter => {
                    self.move_focus(1);
                    return Some(false);
                }
                _ => {}
            },
            Field::Cell { table, row, value } => {
                let t = self.table_mut(table);
                let fixed = t.fixed;
                if ctrl && key.code == KeyCode::Char('d') {
                    if t.remove(row) {
                        let rows = t.rows.len();
                        self.field = Field::Cell { table, row: row.min(rows - 1), value };
                        return Some(true);
                    }
                    return Some(false);
                }
                let Some((k, v)) = t.rows.get_mut(row) else {
                    return None;
                };
                let input = if value { &mut *v } else { &mut *k };
                // Path variable names come from the URL and can't be edited.
                let locked = fixed && !value;
                match key.code {
                    KeyCode::Right if !value && input.cursor() == input.len() => {
                        v.set_cursor(0);
                        self.field = Field::Cell { table, row, value: true };
                        return Some(false);
                    }
                    KeyCode::Left if value && !fixed && input.cursor() == 0 => {
                        self.field = Field::Cell { table, row, value: false };
                        return Some(false);
                    }
                    KeyCode::Enter if !value => {
                        self.field = Field::Cell { table, row, value: true };
                        return Some(false);
                    }
                    KeyCode::Enter => {
                        self.move_focus(1);
                        if let Field::Cell { table: t2, row: r2, .. } = self.field {
                            self.field = Field::Cell { table: t2, row: r2, value: t2 == Table::Path };
                        }
                        return Some(false);
                    }
                    _ if locked => {}
                    _ => match input.handle_key(key) {
                        Edit::Changed => {
                            t.after_edit();
                            return Some(true);
                        }
                        Edit::Moved => return Some(false),
                        Edit::Ignored => {}
                    },
                }
            }
            Field::ContentType => match key.code {
                KeyCode::Left | KeyCode::Char('h') => {
                    self.cycle_content_type(-1);
                    return Some(true);
                }
                KeyCode::Right | KeyCode::Char('l' | ' ') => {
                    self.cycle_content_type(1);
                    return Some(true);
                }
                KeyCode::Enter if !self.content_type.is_empty() => {
                    self.field = Field::Body;
                    return Some(false);
                }
                _ => {}
            },
            Field::Body => match self.body.handle_key(key) {
                Edit::Changed => return Some(true),
                Edit::Moved => return Some(false),
                Edit::Ignored => {}
            },
        }
        match key.code {
            KeyCode::Up => {
                self.move_focus(-1);
                Some(false)
            }
            KeyCode::Down => {
                self.move_focus(1);
                Some(false)
            }
            _ => None,
        }
    }

    fn paste(&mut self, text: &str) -> bool {
        match self.field {
            Field::Name => self.name.insert_str(text),
            Field::Url => {
                self.url.insert_str(text);
                self.sync_path_params();
            }
            Field::Cell { table, row, value } => {
                let t = self.table_mut(table);
                if t.fixed && !value {
                    return false;
                }
                let Some((k, v)) = t.rows.get_mut(row) else {
                    return false;
                };
                if value { v } else { k }.insert_str(text);
                t.after_edit();
            }
            Field::Body => self.body.insert_str(text),
            _ => return false,
        }
        true
    }
}

pub struct ResponseView {
    pub resp: Arc<Response>,
    /// Display text split into lines once, so scrolling stays cheap.
    pub lines: Vec<String>,
    pub scroll: usize,
    pub hscroll: usize,
    pub show_headers: bool,
    pub header_sel: usize,
}

pub enum ResponseState {
    Idle,
    Loading { started: Instant },
    Done(ResponseView),
    Failed(String),
}

#[derive(Clone, Debug)]
pub enum PromptKind {
    NewCollection,
    RenameCollection(PathBuf),
    Import,
    OpenFolder,
    SaveResponse,
}

#[derive(Clone, Debug)]
pub enum ConfirmKind {
    DeleteRequest(DraftKey),
    DeleteCollection(PathBuf),
    Overwrite(PathBuf),
    Quit,
}

pub enum Dialog {
    Prompt {
        title: &'static str,
        input: LineInput,
        kind: PromptKind,
    },
    Confirm {
        title: String,
        body: String,
        options: &'static [&'static str],
        selected: usize,
        kind: ConfirmKind,
    },
    Help,
}

impl PromptKind {
    fn is_path(&self) -> bool {
        matches!(self, PromptKind::Import | PromptKind::OpenFolder | PromptKind::SaveResponse)
    }
}

pub struct App {
    pub session: Session,
    pub rows: Vec<Row>,
    pub list: ListState,
    pub filter: LineInput,
    pub filtering: bool,

    pub active: Option<DraftKey>,
    pub dirty: bool,
    pub editor: Editor,
    pub pane: Pane,

    pub response: ResponseState,
    request_seq: u64,
    tx: Sender<(u64, anyhow::Result<Response>)>,
    rx: Receiver<(u64, anyhow::Result<Response>)>,

    pub dialog: Option<Dialog>,
    pub notice: Option<(Notice, Instant)>,
    /// Text to put on the clipboard (via OSC 52) after the next draw.
    pub clipboard: Option<String>,
    pub quit: bool,
}

impl App {
    pub fn new(dir: &Path) -> App {
        let (tx, rx) = channel();
        let mut app = App {
            session: Session::open(dir),
            rows: Vec::new(),
            list: ListState::default(),
            filter: LineInput::default(),
            filtering: false,
            active: None,
            dirty: false,
            editor: Editor::default(),
            pane: Pane::Sidebar,
            response: ResponseState::Idle,
            request_seq: 0,
            tx,
            rx,
            dialog: None,
            notice: None,
            clipboard: None,
            quit: false,
        };
        app.after_workspace_load();
        app
    }

    fn after_workspace_load(&mut self) {
        if let Some(n) = self.session.load_errors().into_iter().next() {
            self.notify(n);
        }
        self.rebuild_rows();
        // Open the first request so the screen is immediately useful.
        if let Some((file, key)) = self.session.first_request() {
            self.open_request((file, key));
        }
    }

    pub fn notify(&mut self, notice: Notice) {
        self.notice = Some((notice, Instant::now()));
    }

    fn error(&mut self, e: impl std::fmt::Display) {
        self.notify(Notice::error(e.to_string()));
    }

    /// Called regularly: picks up finished requests and expires notices.
    pub fn tick(&mut self) {
        while let Ok((seq, result)) = self.rx.try_recv() {
            if seq != self.request_seq {
                continue;
            }
            self.response = match result {
                Ok(resp) => ResponseState::Done(ResponseView {
                    lines: resp.text.lines().map(str::to_string).collect(),
                    resp: Arc::new(resp),
                    scroll: 0,
                    hscroll: 0,
                    show_headers: false,
                    header_sel: 0,
                }),
                Err(e) => ResponseState::Failed(format!("{e:#}")),
            };
        }
        if self.notice.as_ref().is_some_and(|(_, at)| at.elapsed() > NOTICE_TTL) {
            self.notice = None;
        }
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.response, ResponseState::Loading { .. })
    }

    // ----- sidebar ---------------------------------------------------------

    pub fn rebuild_rows(&mut self) {
        let selected = self.list.selected().and_then(|i| self.rows.get(i)).cloned();
        self.rows = self.session.rows(self.filter.value());
        let ix = selected
            .and_then(|sel| self.rows.iter().position(|r| *r == sel))
            .or_else(|| {
                // Fall back to the open request, then to the top.
                let (file, key) = self.active.as_ref()?;
                self.rows.iter().position(|r| {
                    matches!(r, Row::Request { file: f, key: k, .. } if f == file && k == key)
                })
            })
            .or((!self.rows.is_empty()).then_some(0));
        self.list.select(ix.map(|i| i.min(self.rows.len().saturating_sub(1))));
    }

    fn select_request(&mut self, target: &DraftKey) {
        if let Some(ix) = self.rows.iter().position(
            |r| matches!(r, Row::Request { file, key, .. } if *file == target.0 && *key == target.1),
        ) {
            self.list.select(Some(ix));
        }
    }

    fn selected_row(&self) -> Option<Row> {
        self.list.selected().and_then(|i| self.rows.get(i)).cloned()
    }

    fn selected_collection(&self) -> Option<PathBuf> {
        match self.selected_row()? {
            Row::Collection { file, .. } | Row::Request { file, .. } => Some(file),
        }
    }

    fn sidebar_key(&mut self, key: KeyEvent) {
        if self.filtering {
            match key.code {
                KeyCode::Enter | KeyCode::Down => self.filtering = false,
                KeyCode::Esc => {
                    self.filtering = false;
                    self.filter.set("");
                    self.rebuild_rows();
                }
                _ => {
                    if self.filter.handle_key(key) == Edit::Changed {
                        self.list.select(Some(0));
                        self.rebuild_rows();
                    }
                }
            }
            return;
        }
        let len = self.rows.len();
        let sel = self.list.selected().unwrap_or(0);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.list.select(Some(sel.saturating_sub(1))),
            KeyCode::Down | KeyCode::Char('j') => {
                self.list.select(Some((sel + 1).min(len.saturating_sub(1))))
            }
            KeyCode::PageUp => self.list.select(Some(sel.saturating_sub(10))),
            KeyCode::PageDown => self.list.select(Some((sel + 10).min(len.saturating_sub(1)))),
            KeyCode::Home | KeyCode::Char('g') => self.list.select(Some(0)),
            KeyCode::End | KeyCode::Char('G') => self.list.select(Some(len.saturating_sub(1))),
            KeyCode::Char('/') => self.filtering = true,
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.set("");
                self.rebuild_rows();
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l' | ' ') => match self.selected_row() {
                Some(Row::Collection { file, collapsed, .. }) => {
                    if key.code == KeyCode::Enter || key.code == KeyCode::Char(' ') || collapsed {
                        self.session.toggle_collapsed(&file);
                        self.rebuild_rows();
                    }
                }
                Some(Row::Request { file, key: op, .. }) => {
                    self.open_request((file, op));
                    if key.code == KeyCode::Enter {
                        self.pane = Pane::Request;
                    }
                }
                None => {}
            },
            KeyCode::Left | KeyCode::Char('h') => match self.selected_row() {
                Some(Row::Collection { file, collapsed: false, .. }) => {
                    self.session.toggle_collapsed(&file);
                    self.rebuild_rows();
                }
                Some(Row::Request { file, .. }) => {
                    if let Some(ix) = self.rows.iter().position(
                        |r| matches!(r, Row::Collection { file: f, .. } if *f == file),
                    ) {
                        self.list.select(Some(ix));
                    }
                }
                _ => {}
            },
            KeyCode::Char('n') => match self.selected_collection() {
                Some(file) => self.new_request_in(&file),
                None => self.new_request(),
            },
            KeyCode::Char('N') => self.new_collection(),
            KeyCode::Char('r') => {
                if let Some(file) = self.selected_collection() {
                    self.rename_collection(&file);
                }
            }
            KeyCode::Char('d') => {
                if let Some(Row::Request { file, key, .. }) = self.selected_row() {
                    self.duplicate_request((file, key));
                }
            }
            KeyCode::Delete | KeyCode::Char('x') => match self.selected_row() {
                Some(Row::Request { file, key, .. }) => self.delete_request((file, key)),
                Some(Row::Collection { file, .. }) => self.delete_collection(&file),
                None => {}
            },
            KeyCode::Char('i') => self.import_openapi(),
            KeyCode::Char('o') => self.open_folder(),
            _ => {}
        }
    }

    // ----- request editor --------------------------------------------------

    fn mark_dirty(&mut self) {
        if self.active.is_some() {
            self.dirty = true;
        }
    }

    /// Remembers unsaved edits of the open request so switching away never loses them.
    fn stash_active(&mut self) {
        if let Some(key) = self.active.clone() {
            if self.dirty {
                self.session.drafts.insert(key, self.editor.snapshot());
            }
        }
    }

    pub fn is_draft(&self, target: &DraftKey) -> bool {
        self.session.is_dirty(target) || (self.dirty && self.active.as_ref() == Some(target))
    }

    pub fn open_request(&mut self, target: DraftKey) {
        if self.active.as_ref() == Some(&target) {
            return;
        }
        self.stash_active();
        let Some((spec, dirty)) = self.session.take_request(&target) else {
            return;
        };
        self.select_request(&target);
        self.active = Some(target);
        self.editor.load(&spec);
        self.dirty = dirty;
    }

    fn close_editor(&mut self) {
        self.active = None;
        self.dirty = false;
        self.editor.load(&RequestSpec::default());
        if self.pane == Pane::Request {
            self.pane = Pane::Sidebar;
        }
    }

    pub fn save(&mut self) {
        let Some((file, key)) = self.active.clone() else {
            return;
        };
        let spec = self.editor.snapshot();
        match self.session.save_request(&file, &key, &spec) {
            Ok(new_key) => {
                self.dirty = false;
                self.active = Some((file.clone(), new_key.clone()));
                // Reload: the stored form is canonical (e.g. URL query moved to params).
                if let Some(saved) = self.session.stored_request(&file, &new_key) {
                    if saved != spec {
                        self.editor.load(&saved);
                    }
                }
                self.rebuild_rows();
                self.select_request(&(file, new_key));
                self.notify(Notice::success("Saved"));
            }
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    fn save_all(&mut self) -> bool {
        self.stash_active();
        let errors = self.session.save_drafts();
        let ok = errors.is_empty();
        if let Some(e) = errors.first() {
            self.error(format!("{e:#}"));
        }
        if ok {
            self.dirty = false;
        }
        self.rebuild_rows();
        ok
    }

    pub fn new_request(&mut self) {
        match self.session.default_collection(self.active.as_ref()) {
            Some(file) => self.new_request_in(&file),
            None => self.new_collection(),
        }
    }

    fn new_request_in(&mut self, file: &Path) {
        match self.session.new_request(file) {
            Ok(key) => {
                self.rebuild_rows();
                self.open_request((file.to_path_buf(), key));
                self.focus_url();
            }
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    fn duplicate_request(&mut self, (file, key): DraftKey) {
        match self.session.duplicate_request(&file, &key) {
            Ok(new_key) => {
                self.rebuild_rows();
                self.open_request((file, new_key));
            }
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    fn delete_request(&mut self, target: DraftKey) {
        self.dialog = Some(Dialog::Confirm {
            title: format!("Delete {} {}?", target.1.method, target.1.path),
            body: "The operation is removed from the OpenAPI file.".into(),
            options: &["Delete", "Cancel"],
            selected: 1,
            kind: ConfirmKind::DeleteRequest(target),
        });
    }

    fn new_collection(&mut self) {
        self.prompt("New collection", "", PromptKind::NewCollection);
    }

    fn rename_collection(&mut self, file: &Path) {
        if let Some(title) = self.session.collection_title(file) {
            self.prompt("Rename collection", &title, PromptKind::RenameCollection(file.into()));
        }
    }

    fn delete_collection(&mut self, file: &Path) {
        let Some(title) = self.session.collection_title(file) else {
            return;
        };
        self.dialog = Some(Dialog::Confirm {
            title: format!("Delete collection \"{title}\"?"),
            body: format!("This deletes {}.", file.display()),
            options: &["Delete", "Cancel"],
            selected: 1,
            kind: ConfirmKind::DeleteCollection(file.into()),
        });
    }

    fn import_openapi(&mut self) {
        self.prompt("Import OpenAPI file (path)", &home_relative(&cwd()), PromptKind::Import);
    }

    fn open_folder(&mut self) {
        let dir = home_relative(self.session.dir());
        self.prompt("Open folder", &dir, PromptKind::OpenFolder);
    }

    fn switch_workspace(&mut self, dir: PathBuf) {
        if !self.save_all() {
            return;
        }
        self.session.switch_workspace(&dir);
        self.close_editor();
        self.filter.set("");
        self.list.select(None);
        self.after_workspace_load();
    }

    pub fn reload(&mut self) {
        self.stash_active();
        let active = self.active.take();
        self.dirty = false;
        self.session.reload();
        if let Some(n) = self.session.load_errors().into_iter().next() {
            self.notify(n);
        } else {
            self.notify(Notice::success("Reloaded collections"));
        }
        self.rebuild_rows();
        match active {
            Some(target) if self.session.contains(&target) => self.open_request(target),
            _ => self.close_editor(),
        }
    }

    pub fn request_quit(&mut self) {
        self.stash_active();
        if self.session.drafts.is_empty() {
            self.quit = true;
            return;
        }
        let count = self.session.drafts.len();
        self.dialog = Some(Dialog::Confirm {
            title: format!("You have {count} unsaved request{}.", if count == 1 { "" } else { "s" }),
            body: "Your changes will be lost if you don't save them.".into(),
            options: &["Save All", "Don't Save", "Cancel"],
            selected: 0,
            kind: ConfirmKind::Quit,
        });
    }

    fn focus_url(&mut self) {
        if self.active.is_some() {
            self.pane = Pane::Request;
            self.editor.field = Field::Url;
        }
    }

    fn request_key(&mut self, key: KeyEvent) {
        if self.active.is_none() {
            return;
        }
        if self.editor.field == Field::Url && key.code == KeyCode::Enter {
            self.send();
            return;
        }
        if self.editor.handle_key(key) == Some(true) {
            self.mark_dirty();
        }
    }

    // ----- sending ---------------------------------------------------------

    pub fn send(&mut self) {
        if self.is_loading() || self.active.is_none() {
            return;
        }
        let prepared = match http::prepare(&self.editor.snapshot()) {
            Ok(p) => p,
            Err(e) => {
                self.response = ResponseState::Failed(format!("{e:#}"));
                return;
            }
        };
        self.request_seq += 1;
        let seq = self.request_seq;
        let tx = self.tx.clone();
        if let Err(e) = http::spawn(prepared, move |result| {
            let _ = tx.send((seq, result));
        }) {
            self.response = ResponseState::Failed(format!("Could not start request: {e}"));
            return;
        }
        self.response = ResponseState::Loading { started: Instant::now() };
    }

    pub fn cancel(&mut self) -> bool {
        if self.is_loading() {
            self.request_seq += 1;
            self.response = ResponseState::Failed("Request cancelled".into());
            true
        } else {
            false
        }
    }

    fn response_key(&mut self, key: KeyEvent) {
        let ResponseState::Done(view) = &mut self.response else {
            return;
        };
        let rows = if view.show_headers { view.resp.headers.len() } else { view.lines.len() };
        let last = rows.saturating_sub(1);
        let pos = if view.show_headers { &mut view.header_sel } else { &mut view.scroll };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => *pos = pos.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => *pos = (*pos + 1).min(last),
            KeyCode::PageUp => *pos = pos.saturating_sub(20),
            KeyCode::PageDown | KeyCode::Char(' ') => *pos = (*pos + 20).min(last),
            KeyCode::Home | KeyCode::Char('g') => *pos = 0,
            KeyCode::End | KeyCode::Char('G') => *pos = last,
            KeyCode::Left | KeyCode::Char('h') => view.hscroll = view.hscroll.saturating_sub(8),
            KeyCode::Right | KeyCode::Char('l') => view.hscroll += 8,
            KeyCode::Char('v') => view.show_headers = !view.show_headers,
            KeyCode::Char('y') => {
                let text = if view.show_headers {
                    let Some((_, v)) = view.resp.headers.get(view.header_sel) else {
                        return;
                    };
                    v.clone()
                } else if view.resp.text.is_empty() {
                    String::from_utf8_lossy(&view.resp.body).into_owned()
                } else {
                    view.resp.text.clone()
                };
                let what = if view.show_headers { "Header value" } else { "Response" };
                self.clipboard = Some(text);
                self.notify(Notice::success(format!("{what} copied")));
            }
            KeyCode::Char('s') => self.save_response(),
            _ => {}
        }
    }

    fn save_response(&mut self) {
        let ResponseState::Done(view) = &self.response else {
            return;
        };
        let ext = if view.resp.language == "json" { "json" } else { "txt" };
        let dir = dirs::download_dir().unwrap_or_else(|| self.session.dir().to_path_buf());
        let path = home_relative(&dir.join(format!("response.{ext}")));
        self.prompt("Save response body to", &path, PromptKind::SaveResponse);
    }

    fn write_response(&mut self, path: &Path) {
        let ResponseState::Done(view) = &self.response else {
            return;
        };
        match std::fs::write(path, view.resp.body.as_slice()) {
            Ok(()) => self.notify(Notice::success(format!("Saved to {}", path.display()))),
            Err(e) => self.error(e),
        }
    }

    // ----- dialogs ---------------------------------------------------------

    fn prompt(&mut self, title: &'static str, initial: &str, kind: PromptKind) {
        self.dialog = Some(Dialog::Prompt { title, input: LineInput::new(initial), kind });
    }

    fn dialog_key(&mut self, key: KeyEvent) {
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        match dialog {
            Dialog::Help => {
                self.dialog = None;
            }
            Dialog::Prompt { input, kind, .. } => match key.code {
                KeyCode::Esc => self.dialog = None,
                KeyCode::Enter => {
                    let value = input.value().trim().to_string();
                    let kind = kind.clone();
                    if !value.is_empty() {
                        self.dialog = None;
                        self.submit_prompt(kind, value);
                    }
                }
                KeyCode::Tab if kind.is_path() => {
                    if let Some(done) = complete_path(input.value()) {
                        input.set(done);
                    }
                }
                _ => {
                    input.handle_key(key);
                }
            },
            Dialog::Confirm { options, selected, kind, .. } => {
                let choice = match key.code {
                    KeyCode::Esc => Some(options.len() - 1),
                    KeyCode::Left | KeyCode::BackTab | KeyCode::Char('h') => {
                        *selected = selected.saturating_sub(1);
                        None
                    }
                    KeyCode::Right | KeyCode::Tab | KeyCode::Char('l') => {
                        *selected = (*selected + 1).min(options.len() - 1);
                        None
                    }
                    KeyCode::Enter => Some(*selected),
                    // First letter of an option picks it.
                    KeyCode::Char(c) => options
                        .iter()
                        .position(|o| o.to_lowercase().starts_with(c.to_ascii_lowercase())),
                    _ => None,
                };
                if let Some(choice) = choice {
                    let kind = kind.clone();
                    self.dialog = None;
                    self.confirm(kind, choice);
                }
            }
        }
    }

    fn submit_prompt(&mut self, kind: PromptKind, value: String) {
        match kind {
            PromptKind::NewCollection => match self.session.create_collection(&value) {
                Ok(file) => {
                    self.rebuild_rows();
                    self.new_request_in(&file);
                }
                Err(e) => self.error(format!("{e:#}")),
            },
            PromptKind::RenameCollection(file) => {
                match self.session.rename_collection(&file, &value) {
                    Ok(()) => self.rebuild_rows(),
                    Err(e) => self.error(format!("{e:#}")),
                }
            }
            PromptKind::Import => {
                let (notices, open) = self.session.import_files(&[expand_home(&value)]);
                if let Some(n) = notices.into_iter().next() {
                    self.notify(n);
                }
                self.rebuild_rows();
                if let Some(target) = open {
                    self.open_request(target);
                }
            }
            PromptKind::OpenFolder => {
                let dir = expand_home(&value);
                match dir.canonicalize() {
                    Ok(dir) if dir.is_dir() => self.switch_workspace(dir),
                    _ => self.error(format!("{} is not a folder", dir.display())),
                }
            }
            PromptKind::SaveResponse => {
                let path = expand_home(&value);
                if path.exists() {
                    self.dialog = Some(Dialog::Confirm {
                        title: format!("{} already exists.", path.display()),
                        body: "Replace it?".into(),
                        options: &["Replace", "Cancel"],
                        selected: 1,
                        kind: ConfirmKind::Overwrite(path),
                    });
                } else {
                    self.write_response(&path);
                }
            }
        }
    }

    fn confirm(&mut self, kind: ConfirmKind, choice: usize) {
        match kind {
            ConfirmKind::DeleteRequest((file, key)) if choice == 0 => {
                if let Err(e) = self.session.delete_request(&file, &key) {
                    self.error(format!("{e:#}"));
                    return;
                }
                if self.active.as_ref() == Some(&(file, key)) {
                    self.close_editor();
                }
                self.rebuild_rows();
            }
            ConfirmKind::DeleteCollection(file) if choice == 0 => {
                if let Err(e) = self.session.delete_collection(&file) {
                    self.error(format!("{e:#}"));
                    return;
                }
                if self.active.as_ref().is_some_and(|(f, _)| *f == file) {
                    self.close_editor();
                }
                self.rebuild_rows();
            }
            ConfirmKind::Overwrite(path) if choice == 0 => self.write_response(&path),
            ConfirmKind::Quit => match choice {
                0 => self.quit = self.save_all(),
                1 => self.quit = true,
                _ => {}
            },
            _ => {}
        }
    }

    // ----- input -----------------------------------------------------------

    pub fn paste(&mut self, text: &str) {
        match &mut self.dialog {
            Some(Dialog::Prompt { input, .. }) => input.insert_str(text.trim_end_matches('\n')),
            Some(_) => {}
            None if self.pane == Pane::Sidebar && self.filtering => {
                self.filter.insert_str(text);
                self.rebuild_rows();
            }
            None if self.pane == Pane::Request && self.active.is_some() => {
                if self.editor.paste(text) {
                    self.mark_dirty();
                }
            }
            None => {}
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.dialog.is_some() {
            self.dialog_key(key);
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        // Global shortcuts.
        match key.code {
            KeyCode::Enter if ctrl || alt => return self.send(),
            KeyCode::F(5) => return self.send(),
            KeyCode::Char('s') if ctrl => return self.save(),
            KeyCode::Char('n') if ctrl => return self.new_request(),
            KeyCode::Char('n') if alt => return self.new_collection(),
            KeyCode::Char('i') if ctrl || alt => return self.import_openapi(),
            KeyCode::Char('o') if ctrl => return self.open_folder(),
            KeyCode::Char('r') if ctrl => return self.reload(),
            KeyCode::Char('l') if ctrl => return self.focus_url(),
            KeyCode::Char('q' | 'c') if ctrl => return self.request_quit(),
            KeyCode::F(1) => {
                self.dialog = Some(Dialog::Help);
                return;
            }
            KeyCode::Esc if self.cancel() => return,
            KeyCode::Tab | KeyCode::BackTab => {
                let order = [Pane::Sidebar, Pane::Request, Pane::Response];
                let ix = order.iter().position(|p| *p == self.pane).unwrap_or(0);
                let step = if key.code == KeyCode::BackTab { 2 } else { 1 };
                self.pane = order[(ix + step) % 3];
                self.filtering = false;
                return;
            }
            _ => {}
        }
        let typing = match self.pane {
            Pane::Sidebar => self.filtering,
            Pane::Request => true,
            Pane::Response => false,
        };
        if !typing {
            match key.code {
                KeyCode::Char('?') => {
                    self.dialog = Some(Dialog::Help);
                    return;
                }
                KeyCode::Char('q') => return self.request_quit(),
                _ => {}
            }
        }
        match self.pane {
            Pane::Sidebar => self.sidebar_key(key),
            Pane::Request => {
                if key.code == KeyCode::Esc {
                    self.pane = Pane::Sidebar;
                } else {
                    self.request_key(key);
                }
            }
            Pane::Response => {
                if key.code == KeyCode::Esc {
                    self.pane = Pane::Request;
                } else {
                    self.response_key(key);
                }
            }
        }
    }
}

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_default()
}

/// Shows paths under the home folder as `~/...`, ending folders with `/`.
fn home_relative(path: &Path) -> String {
    let mut s = match dirs::home_dir().and_then(|h| path.strip_prefix(h).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    };
    if path.is_dir() && !s.ends_with('/') {
        s.push('/');
    }
    s
}

pub fn expand_home(s: &str) -> PathBuf {
    match (s.strip_prefix("~"), dirs::home_dir()) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with('/') => {
            home.join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(s),
    }
}

/// Shell-style completion: extends the last path component to the longest
/// prefix shared by the matching entries, adding `/` after a single folder.
pub fn complete_path(input: &str) -> Option<String> {
    let (dir_part, prefix) = match input.rfind('/') {
        Some(i) => (&input[..=i], &input[i + 1..]),
        None => ("", input),
    };
    let dir = if dir_part.is_empty() { cwd() } else { expand_home(dir_part) };
    let mut matches: Vec<(String, bool)> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let hidden_ok = prefix.starts_with('.') || !name.starts_with('.');
            (name.starts_with(prefix) && hidden_ok).then(|| (name, e.path().is_dir()))
        })
        .collect();
    matches.sort();
    let (first, is_dir) = matches.first()?.clone();
    let common = matches.iter().fold(first.clone(), |acc, (name, _)| {
        acc.chars()
            .zip(name.chars())
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a)
            .collect()
    });
    let mut out = format!("{dir_part}{common}");
    if matches.len() == 1 && is_dir {
        out.push('/');
    }
    (out != input).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn typed(app: &mut App, text: &str) {
        for c in text.chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rocketman-tui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn edit_and_save_from_keyboard() {
        let dir = temp_dir("edit");
        let mut app = App::new(&dir);
        assert!(app.rows.is_empty());

        // Alt-N: new collection; it gets a first request that opens with the URL focused.
        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::ALT));
        typed(&mut app, "Pets");
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.pane, Pane::Request);
        assert_eq!(app.editor.field, Field::Url);
        assert!(!app.dirty);

        // Replace the URL; the {id} becomes a path variable row.
        app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        typed(&mut app, "https://pets.dev/pets/{id}");
        assert!(app.dirty);
        assert_eq!(app.editor.path.rows.len(), 1);

        // Down past the tab bar lands on the path variable's value.
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.editor.field, Field::Cell { table: Table::Path, row: 0, value: true });
        typed(&mut app, "7");
        // Then a query parameter.
        app.handle_key(key(KeyCode::Down));
        typed(&mut app, "q");
        app.handle_key(key(KeyCode::Enter));
        typed(&mut app, "cats");
        assert_eq!(app.editor.query.rows.len(), 2);

        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert!(!app.dirty);
        let (file, key_) = app.active.clone().unwrap();
        assert_eq!(key_.path, "/pets/{id}");
        let saved = app.session.stored_request(&file, &key_).unwrap();
        assert_eq!(saved.path_params, vec![Param::new("id", "7")]);
        assert_eq!(saved.query, vec![Param::new("q", "cats")]);

        // Unsaved edits survive switching requests and are offered on quit.
        app.handle_key(key(KeyCode::Char('!')));
        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
        assert_eq!(app.session.drafts.len(), 1);
        app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
        assert!(matches!(app.dialog, Some(Dialog::Confirm { kind: ConfirmKind::Quit, .. })));
        app.handle_key(key(KeyCode::Char('d')));
        assert!(app.quit);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn body_and_method() {
        let mut editor = Editor::default();
        editor.field = Field::Method;
        editor.handle_key(key(KeyCode::Char('p')));
        assert_eq!(editor.method, Method::Post);
        editor.handle_key(key(KeyCode::Char('p')));
        assert_eq!(editor.method, Method::Put);
        editor.tab = ReqTab::Body;
        editor.field = Field::ContentType;
        editor.handle_key(key(KeyCode::Right));
        assert_eq!(editor.content_type, "application/json");
        editor.handle_key(key(KeyCode::Enter));
        assert_eq!(editor.field, Field::Body);
        editor.paste("{\n}");
        assert_eq!(editor.snapshot().body, "{\n}");
        // Up from the first body line goes back to the content type.
        editor.handle_key(key(KeyCode::Up));
        editor.handle_key(key(KeyCode::Up));
        assert_eq!(editor.field, Field::ContentType);
    }

    #[test]
    fn completes_paths() {
        let dir = temp_dir("complete");
        std::fs::create_dir(dir.join("specs")).unwrap();
        std::fs::write(dir.join("pets.yaml"), "").unwrap();
        std::fs::write(dir.join("petshop.yaml"), "").unwrap();
        let base = format!("{}/", dir.display());
        assert_eq!(complete_path(&format!("{base}sp")), Some(format!("{base}specs/")));
        assert_eq!(complete_path(&format!("{base}p")), Some(format!("{base}pets")));
        assert_eq!(complete_path(&format!("{base}pets")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
