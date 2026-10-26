//! Drawing. Everything here reads [`App`] and writes to the frame; the only
//! state it touches is scroll positions that depend on the screen size.

use crate::app::{App, ConfirmKind, Dialog, Field, Pane, ReqTab, ResponseState, Table};
use crate::text::LineInput;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, Paragraph, Wrap};
use rocketman::http::{format_duration, format_size};
use rocketman::openapi::Method;
use rocketman::session::{Level, Row, content_type_label};

const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;

fn method_color(method: Method) -> Color {
    match method {
        Method::Get => Color::Green,
        Method::Post => Color::Yellow,
        Method::Put => Color::Blue,
        Method::Patch => Color::Magenta,
        Method::Delete => Color::Red,
        _ => Color::Cyan,
    }
}

fn pane_block(title: impl Into<Line<'static>>, focused: bool) -> Block<'static> {
    Block::bordered()
        .border_type(if focused { BorderType::Thick } else { BorderType::Rounded })
        .border_style(Style::new().fg(if focused { ACCENT } else { MUTED }))
        .title(title.into())
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [main, status] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(frame.area());
    let sidebar_width = (main.width / 3).clamp(24, 44);
    let [sidebar, right] =
        Layout::horizontal([Constraint::Length(sidebar_width), Constraint::Min(20)]).areas(main);

    let mut cursor = None;
    draw_sidebar(frame, app, sidebar, &mut cursor);
    if app.active.is_some() {
        let [request, response] =
            Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(right);
        draw_request(frame, app, request, &mut cursor);
        draw_response(frame, app, response);
    } else {
        let block = pane_block(" Request ", app.pane == Pane::Request);
        let text = vec![
            Line::from("No request selected").bold(),
            Line::from("Pick one from the sidebar or press Ctrl-N to create one.").fg(MUTED),
        ];
        let inner = block.inner(right);
        frame.render_widget(block, right);
        let mid = Rect { y: inner.y + inner.height / 2 - 1, height: 2, ..inner };
        frame.render_widget(Paragraph::new(text).centered(), mid);
    }
    draw_status(frame, app, status);

    if app.dialog.is_some() {
        cursor = None;
        draw_dialog(frame, app, &mut cursor);
    }
    if let Some(pos) = cursor {
        frame.set_cursor_position(pos);
    }
}

// ----- sidebar ---------------------------------------------------------------

fn draw_sidebar(frame: &mut Frame, app: &mut App, area: Rect, cursor: &mut Option<Position>) {
    let focused = app.pane == Pane::Sidebar;
    // The end of the folder path is the informative part.
    let dir = app.session.dir().display().to_string();
    let room = area.width.saturating_sub(4) as usize;
    let count = dir.chars().count();
    let dir = if count > room {
        format!("…{}", dir.chars().skip(count + 1 - room).collect::<String>())
    } else {
        dir
    };
    let block = pane_block(" Collections ", focused)
        .title_bottom(Line::from(format!(" {dir} ")).fg(MUTED));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [filter_area, list_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);

    // Filter line.
    if app.filtering || !app.filter.is_empty() {
        let width = filter_area.width.saturating_sub(2) as usize;
        let (text, col) = app.filter.view(width);
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::from("/ ").fg(ACCENT), Span::from(text)])),
            filter_area,
        );
        if app.filtering && focused {
            *cursor = Some(Position::new(filter_area.x + 2 + col as u16, filter_area.y));
        }
    } else {
        frame.render_widget(
            Paragraph::new("/ filter   n new   ? help").fg(MUTED),
            filter_area,
        );
    }

    if app.rows.is_empty() {
        let text = vec![
            Line::from("No collections yet"),
            Line::from(""),
            Line::from("N  new collection").fg(MUTED),
            Line::from("i  import OpenAPI").fg(MUTED),
        ];
        frame.render_widget(Paragraph::new(text).centered(), list_area);
        return;
    }

    let width = list_area.width as usize;
    let items: Vec<ListItem> = app
        .rows
        .iter()
        .map(|row| match row {
            Row::Collection { title, count, collapsed, .. } => {
                let arrow = if *collapsed { "▸ " } else { "▾ " };
                let count = count.to_string();
                let title = truncate(title, width.saturating_sub(3 + count.len()));
                let pad = width.saturating_sub(2 + title.chars().count() + count.len());
                ListItem::new(Line::from(vec![
                    Span::from(arrow).fg(MUTED),
                    Span::from(title).bold(),
                    Span::from(" ".repeat(pad)),
                    Span::from(count).fg(MUTED),
                ]))
            }
            Row::Request { file, key, label } => {
                let target = (file.clone(), key.clone());
                let active = app.active.as_ref() == Some(&target);
                let dirty = app.is_draft(&target);
                let label = truncate(label, width.saturating_sub(12));
                let mut spans = vec![
                    Span::from("  "),
                    Span::from(format!("{:<7}", key.method.as_str()))
                        .fg(method_color(key.method))
                        .bold(),
                    Span::from(label).add_modifier(if active { Modifier::BOLD } else { Modifier::empty() }),
                ];
                if dirty {
                    spans.push(Span::from(" ●").fg(ACCENT));
                }
                ListItem::new(Line::from(spans))
            }
        })
        .collect();
    let highlight = if focused {
        Style::new().bg(Color::Indexed(237)).add_modifier(Modifier::BOLD)
    } else {
        Style::new().bg(Color::Indexed(235))
    };
    frame.render_stateful_widget(
        List::new(items).highlight_style(highlight),
        list_area,
        &mut app.list,
    );
}

fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

// ----- request ---------------------------------------------------------------

/// Draws a text input, placing the cursor when it is focused.
fn input_line(
    input: &LineInput,
    placeholder: &str,
    area: Rect,
    focused: bool,
    style: Style,
    cursor: &mut Option<Position>,
) -> Paragraph<'static> {
    let width = area.width.max(1) as usize;
    if focused {
        let (text, col) = input.view(width);
        *cursor = Some(Position::new(area.x + col as u16, area.y));
        if input.is_empty() {
            return Paragraph::new(Span::from(placeholder.to_string()).fg(MUTED));
        }
        return Paragraph::new(Span::styled(text, style));
    }
    if input.is_empty() {
        return Paragraph::new(Span::from(placeholder.to_string()).fg(MUTED));
    }
    Paragraph::new(Span::styled(truncate(input.value(), width), style))
}

fn draw_request(frame: &mut Frame, app: &mut App, area: Rect, cursor: &mut Option<Position>) {
    let focused = app.pane == Pane::Request;
    let title = if app.dirty {
        Line::from(vec![Span::from(" Request "), Span::from("● unsaved ").fg(ACCENT)])
    } else {
        Line::from(" Request ")
    };
    let block = pane_block(title, focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let ed = &mut app.editor;
    let field = if focused { Some(ed.field) } else { None };
    let sel = |f: Field| field == Some(f);
    let label_style = |f: Field| {
        if sel(f) { Style::new().fg(ACCENT).bold() } else { Style::new().fg(MUTED) }
    };

    let [name_row, url_row, tabs_row, _, content] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);

    // Name.
    let [label, value] =
        Layout::horizontal([Constraint::Length(6), Constraint::Min(1)]).areas(name_row);
    frame.render_widget(Paragraph::new(Span::styled("Name", label_style(Field::Name))), label);
    frame.render_widget(
        input_line(&ed.name, "Untitled request", value, sel(Field::Name), Style::new().bold(), cursor),
        value,
    );

    // Method + URL.
    let [method_area, _, url_area] = Layout::horizontal([
        Constraint::Length(9),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(url_row);
    let mut method_style = Style::new().fg(method_color(ed.method)).bold();
    if sel(Field::Method) {
        method_style = method_style.reversed();
    }
    frame.render_widget(
        Paragraph::new(Span::styled(format!(" {:<7}▾", ed.method.as_str()), method_style)),
        method_area,
    );
    frame.render_widget(
        input_line(&ed.url, "https://api.example.com/users/{id}", url_area, sel(Field::Url), Style::new(), cursor),
        url_area,
    );

    // Tabs.
    let count_label = |label: &str, n: usize| {
        if n > 0 { format!("{label} ({n})") } else { label.to_string() }
    };
    let tabs = [
        (ReqTab::Params, count_label("Params", ed.query.filled())),
        (ReqTab::Headers, count_label("Headers", ed.headers.filled())),
        (
            ReqTab::Body,
            if ed.content_type.is_empty() {
                "Body".to_string()
            } else {
                format!("Body ({})", content_type_label(&ed.content_type))
            },
        ),
    ];
    let mut spans = Vec::new();
    for (tab, label) in tabs {
        let mut style = if tab == ed.tab {
            Style::new().fg(ACCENT).bold().underlined()
        } else {
            Style::new().fg(MUTED)
        };
        if tab == ed.tab && sel(Field::Tabs) {
            style = style.reversed();
        }
        spans.push(Span::styled(format!(" {label} "), style));
        spans.push(Span::from("  "));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), tabs_row);

    match ed.tab {
        ReqTab::Params | ReqTab::Headers => {
            let mut sections: Vec<(Table, &str, &str, &str)> = Vec::new();
            if ed.tab == ReqTab::Params {
                if !ed.path.rows.is_empty() {
                    sections.push((Table::Path, "Path variables", "Variable", "Value"));
                }
                sections.push((Table::Query, "Query parameters", "Key", "Value"));
            } else {
                sections.push((Table::Headers, "Headers", "Header", "Value"));
            }
            // Lay out every line, then scroll so the focused row is visible.
            enum L<'a> {
                Heading(&'a str),
                Row(Table, usize, &'a str, &'a str),
            }
            let mut lines = Vec::new();
            for (table, heading, kp, vp) in &sections {
                lines.push(L::Heading(heading));
                for row in 0..ed.table(*table).rows.len() {
                    lines.push(L::Row(*table, row, kp, vp));
                }
            }
            let focus_line = lines.iter().position(|l| {
                matches!((l, field), (L::Row(t, r, ..), Some(Field::Cell { table, row, .. })) if *t == table && *r == row)
            });
            let height = content.height as usize;
            let skip = focus_line.map_or(0, |f| (f + 1).saturating_sub(height));
            let key_width = (content.width as usize * 38 / 100).max(8) as u16;
            for (i, line) in lines.iter().skip(skip).take(height).enumerate() {
                let y = content.y + i as u16;
                let row_area = Rect { y, height: 1, ..content };
                match line {
                    L::Heading(text) => frame.render_widget(
                        Paragraph::new(Span::from(text.to_string()).fg(MUTED).bold()),
                        row_area,
                    ),
                    L::Row(table, row, kp, vp) => {
                        let (k, v) = &ed.table(*table).rows[*row];
                        let [_, ka, sep, va] = Layout::horizontal([
                            Constraint::Length(2),
                            Constraint::Length(key_width),
                            Constraint::Length(3),
                            Constraint::Min(1),
                        ])
                        .areas(row_area);
                        let fk = field == Some(Field::Cell { table: *table, row: *row, value: false });
                        let fv = field == Some(Field::Cell { table: *table, row: *row, value: true });
                        let key_style = if *table == Table::Path { Style::new().fg(MUTED) } else { Style::new() };
                        frame.render_widget(input_line(k, kp, ka, fk, key_style, cursor), ka);
                        frame.render_widget(Paragraph::new(" │ ").fg(MUTED), sep);
                        frame.render_widget(input_line(v, vp, va, fv, Style::new(), cursor), va);
                    }
                }
            }
        }
        ReqTab::Body => {
            let [ct_row, _, body_area] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .areas(content);
            let mut ct = Style::new().bold();
            if sel(Field::ContentType) {
                ct = ct.reversed();
            }
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("Content type  ", label_style(Field::ContentType)),
                    Span::styled(format!("‹ {} ›", content_type_label(&ed.content_type)), ct),
                ])),
                ct_row,
            );
            if ed.content_type.is_empty() {
                frame.render_widget(
                    Paragraph::new("This request has no body. Pick a content type to add one.").fg(MUTED),
                    body_area,
                );
                return;
            }
            let block = Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(if sel(Field::Body) { ACCENT } else { MUTED }));
            let body_inner = block.inner(body_area);
            frame.render_widget(block, body_area);
            let (w, h) = (body_inner.width as usize, body_inner.height as usize);
            ed.body.scroll_to_cursor(w, h);
            let (top, left) = ed.body.scroll;
            let json = rocketman::session::body_language(&ed.content_type) == "json";
            let lines: Vec<Line> = ed
                .body
                .lines()
                .iter()
                .skip(top)
                .take(h)
                .map(|l| highlight_line(l, json, left, w))
                .collect();
            frame.render_widget(Paragraph::new(lines), body_inner);
            if sel(Field::Body) {
                let (row, col) = ed.body.cursor();
                *cursor = Some(Position::new(
                    body_inner.x + (col - left) as u16,
                    body_inner.y + (row - top) as u16,
                ));
            }
        }
    }
}

// ----- response --------------------------------------------------------------

fn draw_response(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.pane == Pane::Response;
    let block = pane_block(" Response ", focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    match &mut app.response {
        ResponseState::Idle => {
            let text = vec![
                Line::from("Send a request to see the response").fg(MUTED),
                Line::from("Enter in the URL · F5 · Ctrl/Alt-Enter").fg(MUTED),
            ];
            let mid = Rect { y: inner.y + inner.height.saturating_sub(2) / 2, height: 2.min(inner.height), ..inner };
            frame.render_widget(Paragraph::new(text).centered(), mid);
        }
        ResponseState::Loading { started } => {
            const SPINNER: [&str; 8] = ["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];
            let frame_ix = (started.elapsed().as_millis() / 100) as usize % SPINNER.len();
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::from(SPINNER[frame_ix]).fg(ACCENT),
                    Span::from(format!(" Sending… {}", format_duration(started.elapsed()))),
                    Span::from("   Esc to cancel").fg(MUTED),
                ])),
                inner,
            );
        }
        ResponseState::Failed(msg) => {
            frame.render_widget(
                Paragraph::new(vec![Line::from("Error").fg(Color::Red).bold(), Line::from(msg.clone())])
                    .wrap(Wrap { trim: false }),
                inner,
            );
        }
        ResponseState::Done(view) => {
            let resp = view.resp.clone();
            let status_color = match resp.status {
                200..=299 => Color::Green,
                300..=399 => Color::Blue,
                400..=499 => Color::Yellow,
                _ => Color::Red,
            };
            let tab = |label: String, on: bool| {
                if on {
                    Span::from(format!(" {label} ")).fg(ACCENT).bold().underlined()
                } else {
                    Span::from(format!(" {label} ")).fg(MUTED)
                }
            };
            let header = Line::from(vec![
                Span::from(format!("{} {}", resp.status, resp.reason)).fg(status_color).bold(),
                Span::from(format!("  {}  {}   ", format_duration(resp.elapsed), format_size(resp.body.len()))).fg(MUTED),
                tab("Body".into(), !view.show_headers),
                tab(format!("Headers ({})", resp.headers.len()), view.show_headers),
            ]);
            let note_height = u16::from(resp.note.is_some());
            let [header_row, note_row, body] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(note_height),
                Constraint::Min(0),
            ])
            .areas(inner);
            frame.render_widget(Paragraph::new(header), header_row);
            if let Some(note) = &resp.note {
                frame.render_widget(Paragraph::new(note.clone()).fg(Color::Yellow), note_row);
            }
            let body = Rect { y: body.y + 1.min(body.height), height: body.height.saturating_sub(1), ..body };
            let h = body.height as usize;
            let w = body.width as usize;
            if view.show_headers {
                let key_width = resp.headers.iter().map(|(k, _)| k.len()).max().unwrap_or(0).min(w / 2);
                let skip = (view.header_sel + 1).saturating_sub(h);
                let lines: Vec<Line> = resp
                    .headers
                    .iter()
                    .enumerate()
                    .skip(skip)
                    .take(h)
                    .map(|(i, (k, v))| {
                        let line = Line::from(vec![
                            Span::from(format!("{:<key_width$}  ", truncate(k, key_width))).bold(),
                            Span::from(v.clone()),
                        ]);
                        if i == view.header_sel && focused {
                            line.style(Style::new().bg(Color::Indexed(237)))
                        } else {
                            line
                        }
                    })
                    .collect();
                frame.render_widget(Paragraph::new(lines), body);
            } else {
                let max_scroll = view.lines.len().saturating_sub(h);
                view.scroll = view.scroll.min(max_scroll);
                let json = resp.language == "json";
                let lines: Vec<Line> = view
                    .lines
                    .iter()
                    .skip(view.scroll)
                    .take(h)
                    .map(|l| highlight_line(l, json, view.hscroll, w))
                    .collect();
                frame.render_widget(Paragraph::new(lines), body);
            }
        }
    }
}

/// Colors one line of (pretty-printed) JSON, or returns it plain, showing
/// `width` columns starting at column `skip`.
pub fn highlight_line(line: &str, json: bool, skip: usize, width: usize) -> Line<'static> {
    if !json {
        return Line::from(line.chars().skip(skip).take(width).collect::<String>());
    }
    let chars: Vec<char> = line.chars().collect();
    let mut styled: Vec<(char, Color)> = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                i += if chars[i] == '\\' { 2 } else { 1 };
            }
            let end = (i + 1).min(chars.len());
            let is_key = chars[end..].iter().find(|c| !c.is_whitespace()) == Some(&':');
            let color = if is_key { Color::Cyan } else { Color::Green };
            styled.extend(chars[start..end].iter().map(|c| (*c, color)));
            i = end;
            continue;
        }
        let color = match c {
            '0'..='9' | '-' => Color::Yellow,
            't' | 'f' | 'n' | 'r' | 'u' | 'e' | 'a' | 'l' | 's' => Color::Magenta,
            _ => Color::Reset,
        };
        styled.push((c, color));
        i += 1;
    }
    let mut spans: Vec<Span> = Vec::new();
    let mut run = String::new();
    let mut run_color = Color::Reset;
    for (c, color) in styled.into_iter().skip(skip).take(width) {
        if color != run_color && !run.is_empty() {
            spans.push(Span::from(std::mem::take(&mut run)).fg(run_color));
        }
        run_color = color;
        run.push(c);
    }
    if !run.is_empty() {
        spans.push(Span::from(run).fg(run_color));
    }
    Line::from(spans)
}

// ----- status bar and dialogs -----------------------------------------------

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    if let Some((notice, _)) = &app.notice {
        let color = match notice.level {
            Level::Success => Color::Green,
            Level::Warning => Color::Yellow,
            Level::Error => Color::Red,
        };
        let mut spans = Vec::new();
        if let Some(title) = &notice.title {
            spans.push(Span::from(format!("{title}: ")).fg(color).bold());
        }
        spans.push(Span::from(notice.text.clone()).fg(color));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
        return;
    }
    let hints: &[(&str, &str)] = match app.pane {
        Pane::Sidebar if app.filtering => &[("Enter", "done"), ("Esc", "clear")],
        Pane::Sidebar => &[
            ("Enter", "open"),
            ("n/N", "new request/collection"),
            ("d", "duplicate"),
            ("x", "delete"),
            ("r", "rename"),
            ("i", "import"),
            ("o", "open folder"),
        ],
        Pane::Request => &[
            ("F5", "send"),
            ("^S", "save"),
            ("↑↓", "move"),
            ("←→", "change"),
            ("^D", "remove row"),
        ],
        Pane::Response => &[
            ("↑↓←→", "scroll"),
            ("v", "body/headers"),
            ("y", "copy"),
            ("s", "save to file"),
        ],
    };
    let mut spans = vec![Span::from("Tab").fg(ACCENT).bold(), Span::from(" pane  ").fg(MUTED)];
    for (key, what) in hints {
        spans.push(Span::from(*key).fg(ACCENT).bold());
        spans.push(Span::from(format!(" {what}  ")).fg(MUTED));
    }
    spans.push(Span::from("F1").fg(ACCENT).bold());
    spans.push(Span::from(" help").fg(MUTED));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

const HELP: &[(&str, &str)] = &[
    ("Tab / Shift-Tab", "Next / previous pane"),
    ("F5, Ctrl/Alt-Enter", "Send (Enter in the URL too)"),
    ("Esc", "Cancel the in-flight request"),
    ("Ctrl-S", "Save the request"),
    ("Ctrl-N / Alt-N", "New request / new collection"),
    ("Ctrl-L", "Focus the URL"),
    ("Ctrl-I / Alt-I", "Import an OpenAPI file as a collection"),
    ("Ctrl-O", "Open another folder"),
    ("Ctrl-R", "Reload collections from disk"),
    ("Ctrl-Q", "Quit"),
    ("", ""),
    ("Sidebar", "↑↓ move · Enter open · ←→ fold · / filter"),
    ("", "n new · d duplicate · r rename · x delete"),
    ("Request", "↑↓ fields · ←→ method/tab/type · ^D drop row"),
    ("Response", "↑↓ PgUp PgDn scroll · v body/headers"),
    ("", "y copy · s save body to file"),
];

fn draw_dialog(frame: &mut Frame, app: &App, cursor: &mut Option<Position>) {
    let screen = frame.area();
    match app.dialog.as_ref() {
        Some(Dialog::Help) => {
            let area = centered(screen, 72, HELP.len() as u16 + 4);
            frame.render_widget(Clear, area);
            let block = pane_block(" Keys ", true).title_bottom(Line::from(" any key to close ").fg(MUTED));
            let lines: Vec<Line> = HELP
                .iter()
                .map(|(k, v)| {
                    Line::from(vec![
                        Span::from(format!(" {k:<20}")).fg(ACCENT).bold(),
                        Span::from(*v),
                    ])
                })
                .collect();
            frame.render_widget(Paragraph::new(lines).block(block), area);
        }
        Some(Dialog::Prompt { title, input, kind }) => {
            let area = centered(screen, 70, 5);
            frame.render_widget(Clear, area);
            let path = matches!(
                kind,
                crate::app::PromptKind::Import
                    | crate::app::PromptKind::OpenFolder
                    | crate::app::PromptKind::SaveResponse
            );
            let hint = if path { " Enter ok · Tab complete · Esc cancel " } else { " Enter ok · Esc cancel " };
            let block = pane_block(format!(" {title} "), true).title_bottom(Line::from(hint).fg(MUTED));
            let inner = block.inner(area);
            frame.render_widget(block, area);
            let field = Rect { x: inner.x + 1, y: inner.y + 1, width: inner.width.saturating_sub(2), height: 1 };
            frame.render_widget(input_line(input, "", field, true, Style::new(), cursor), field);
        }
        Some(Dialog::Confirm { title, body, options, selected, kind }) => {
            let width = (title.chars().count().max(body.chars().count()) as u16 + 6).clamp(40, 90);
            let area = centered(screen, width, 7);
            frame.render_widget(Clear, area);
            let danger = matches!(kind, ConfirmKind::DeleteCollection(_) | ConfirmKind::DeleteRequest(_));
            let block = pane_block("", true);
            let mut buttons = Vec::new();
            for (i, label) in options.iter().enumerate() {
                let mut style = if i == 0 && danger { Style::new().fg(Color::Red) } else { Style::new() };
                if i == *selected {
                    style = style.reversed().bold();
                }
                buttons.push(Span::styled(format!(" {label} "), style));
                buttons.push(Span::from("  "));
            }
            let text = vec![
                Line::from(title.clone()).bold(),
                Line::from(body.clone()).fg(MUTED),
                Line::from(""),
                Line::from(buttons),
            ];
            frame.render_widget(
                Paragraph::new(text).block(block.padding(ratatui::widgets::Padding::horizontal(1))).wrap(Wrap { trim: false }),
                area,
            );
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_json() {
        let line = highlight_line(r#"  "id": 12, "ok": true,"#, true, 0, 80);
        let colored: Vec<(String, Option<Color>)> =
            line.spans.iter().map(|s| (s.content.to_string(), s.style.fg)).collect();
        assert!(colored.contains(&("\"id\"".into(), Some(Color::Cyan))));
        assert!(colored.contains(&("12".into(), Some(Color::Yellow))));
        assert!(colored.contains(&("true".into(), Some(Color::Magenta))));
        let clipped = highlight_line("abcdef", false, 2, 3);
        assert_eq!(clipped.to_string(), "cde");
    }
}
