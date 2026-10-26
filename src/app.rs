use rocketman::http::{self, Response, format_duration, format_size};
use crate::kv::{KvEditor, KvEvent};
use rocketman::openapi::{Method, OpKey, RequestSpec, path_templates};
use rocketman::session::{
    BODY_TYPES, DraftKey, Level, Notice, Row, Session, body_language, content_type_label,
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, WindowExt as _,
    h_flex,
    button::{Button, ButtonVariants as _},
    input::{Editor, EditorState, Input, InputEvent, InputState},
    menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem},
    notification::Notification,
    resizable::{h_resizable, resizable_panel, v_resizable},
    spinner::Spinner,
    tab::{Tab, TabBar},
    tooltip::Tooltip,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

actions!(
    rocketman,
    [
        Send,
        Save,
        NewRequest,
        NewCollection,
        ImportOpenApi,
        OpenFolder,
        Reload,
        FocusUrl,
        Cancel,
        Quit
    ]
);

enum ResponseState {
    Idle,
    Loading { started: Instant, _task: Task<()> },
    Done(Arc<Response>),
    Failed(SharedString),
}

pub struct RocketApp {
    focus: FocusHandle,
    session: Session,

    filter: Entity<InputState>,
    rows: Arc<Vec<Row>>,
    list_scroll: UniformListScrollHandle,

    active: Option<DraftKey>,
    dirty: bool,
    /// True while the editor is being filled programmatically.
    loading: bool,

    method: Method,
    content_type: String,
    name: Entity<InputState>,
    url: Entity<InputState>,
    path_params: Entity<KvEditor>,
    query: Entity<KvEditor>,
    headers: Entity<KvEditor>,
    body: Entity<EditorState>,
    request_tab: usize,

    response: ResponseState,
    response_body: Entity<EditorState>,
    response_tab: usize,
    request_seq: u64,

    allow_close: bool,
    _subs: Vec<Subscription>,
}

impl RocketApp {
    pub fn new(dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter requests"));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Untitled request"));
        let url = cx.new(|cx| {
            InputState::new(window, cx).placeholder("https://api.example.com/users/{id}")
        });
        let path_params =
            cx.new(|cx| KvEditor::new("path-params", "Variable", "Value", true, window, cx));
        let query = cx.new(|cx| KvEditor::new("query", "Key", "Value", false, window, cx));
        let headers = cx.new(|cx| KvEditor::new("headers", "Header", "Value", false, window, cx));
        let body = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("json")
                .placeholder("Request body")
        });
        let response_body = cx.new(|cx| EditorState::new(window, cx).language("json"));

        let mut subs = vec![
            cx.subscribe_in(&filter, window, |this, _, e: &InputEvent, _, cx| {
                if matches!(e, InputEvent::Change) {
                    this.rebuild_rows(cx);
                }
            }),
            cx.subscribe_in(&url, window, |this, _, e: &InputEvent, window, cx| match e {
                InputEvent::Change => {
                    this.sync_path_params(window, cx);
                    this.mark_dirty(cx);
                }
                InputEvent::PressEnter { .. } => this.send(&Send, window, cx),
                _ => {}
            }),
            cx.subscribe_in(&name, window, |this, _, e: &InputEvent, _, cx| {
                if matches!(e, InputEvent::Change) {
                    this.mark_dirty(cx);
                }
            }),
            cx.subscribe_in(&body, window, |this, _, e: &InputEvent, _, cx| {
                if matches!(e, InputEvent::Change) {
                    this.mark_dirty(cx);
                }
            }),
        ];
        for kv in [&path_params, &query, &headers] {
            subs.push(cx.subscribe(kv, |this, _, _: &KvEvent, cx| this.mark_dirty(cx)));
        }

        let mut this = Self {
            focus: cx.focus_handle(),
            session: Session::open(&dir),
            filter,
            rows: Arc::default(),
            list_scroll: UniformListScrollHandle::new(),
            active: None,
            dirty: false,
            loading: false,
            method: Method::Get,
            content_type: String::new(),
            name,
            url,
            path_params,
            query,
            headers,
            body,
            request_tab: 0,
            response: ResponseState::Idle,
            response_body,
            response_tab: 0,
            request_seq: 0,
            allow_close: false,
            _subs: subs,
        };
        this.after_workspace_load(window, cx);
        this.install_close_guard(window, cx);
        this
    }

    fn after_workspace_load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for notice in self.session.load_errors() {
            notify(window, notice, cx);
        }
        self.rebuild_rows(cx);
        // Open the first request so the window is immediately useful.
        if let Some((file, key)) = self.session.first_request() {
            self.open_request(file, key, window, cx);
        }
    }

    fn install_close_guard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            let Some(this) = this.upgrade() else {
                return true;
            };
            this.update(cx, |this, cx| this.confirm_close(window, cx))
        });
    }

    /// Returns true when the window may close right away; otherwise asks first.
    fn confirm_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.stash_active(cx);
        if self.allow_close || self.session.drafts.is_empty() {
            return true;
        }
        let count = self.session.drafts.len();
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!(
                "You have {count} unsaved request{}.",
                if count == 1 { "" } else { "s" }
            ),
            Some("Your changes will be lost if you don't save them."),
            &["Save All", "Don't Save", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(choice) = answer.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                match choice {
                    0 => {
                        if !this.save_all(window, cx) {
                            return;
                        }
                    }
                    1 => {}
                    _ => return,
                }
                this.allow_close = true;
                window.remove_window();
            });
        })
        .detach();
        false
    }

    // ----- sidebar ---------------------------------------------------------

    fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        let rows = self.session.rows(&self.filter.read(cx).value());
        self.rows = Arc::new(rows);
        cx.notify();
    }

    fn render_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let Some(row) = self.rows.get(ix).cloned() else {
            return div().into_any_element();
        };
        match row {
            Row::Collection {
                file,
                title,
                count,
                collapsed,
            } => {
                let toggle_file = file.clone();
                let menu_file = file.clone();
                let this = cx.entity().downgrade();
                h_flex()
                    .id(("collection", ix))
                    .h(px(30.))
                    .px_2()
                    .gap_1()
                    .w_full()
                    .rounded(theme.radius)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.sidebar_accent))
                    .child(
                        Icon::new(if collapsed {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronDown
                        })
                        .xsmall()
                        .text_color(theme.muted_foreground),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_semibold()
                            .child(title),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(count.to_string()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.session.toggle_collapsed(&toggle_file);
                        this.rebuild_rows(cx);
                    }))
                    .context_menu(move |menu, _, _| {
                        let item = |label: &'static str, f: fn(&mut RocketApp, &Path, &mut Window, &mut Context<RocketApp>)| {
                            let this = this.clone();
                            let file = menu_file.clone();
                            PopupMenuItem::new(label).on_click(move |_, window, cx| {
                                if let Some(this) = this.upgrade() {
                                    this.update(cx, |this, cx| f(this, &file, window, cx));
                                }
                            })
                        };
                        menu.item(item("New Request", |this, file, window, cx| {
                            this.new_request_in(file, window, cx)
                        }))
                        .item(item("Rename…", |this, file, window, cx| {
                            this.rename_collection(file, window, cx)
                        }))
                        .item(item("Reveal in Finder", |_, file, _, cx| cx.reveal_path(file)))
                        .separator()
                        .item(item("Delete Collection…", |this, file, window, cx| {
                            this.delete_collection(file, window, cx)
                        }))
                    })
                    .into_any_element()
            }
            Row::Request { file, key, label } => {
                let draft_key = (file.clone(), key.clone());
                let selected = self.active.as_ref() == Some(&draft_key);
                let dirty = self.session.is_dirty(&draft_key) || (selected && self.dirty);
                let method = key.method;
                let open_file = file.clone();
                let open_key = key.clone();
                let this = cx.entity().downgrade();
                h_flex()
                    .id(("request", ix))
                    .h(px(28.))
                    .pl(px(26.))
                    .pr_2()
                    .gap_2()
                    .w_full()
                    .rounded(theme.radius)
                    .cursor_pointer()
                    .when(selected, |s| {
                        s.bg(theme.sidebar_accent)
                            .text_color(theme.sidebar_accent_foreground)
                    })
                    .when(!selected, |s| s.hover(|s| s.bg(theme.sidebar_accent.opacity(0.5))))
                    .child(
                        div()
                            .w(px(46.))
                            .flex_none()
                            .text_xs()
                            .font_semibold()
                            .text_color(method_color(method, cx))
                            .child(method.as_str()),
                    )
                    .child(div().flex_1().min_w_0().truncate().text_sm().child(label))
                    .when(dirty, |s| {
                        s.child(
                            div()
                                .size(px(7.))
                                .flex_none()
                                .rounded_full()
                                .bg(theme.primary),
                        )
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_request(open_file.clone(), open_key.clone(), window, cx)
                    }))
                    .context_menu(move |menu, _, _| {
                        let item = |label: &'static str, f: fn(&mut RocketApp, DraftKey, &mut Window, &mut Context<RocketApp>)| {
                            let this = this.clone();
                            let key = draft_key.clone();
                            PopupMenuItem::new(label).on_click(move |_, window, cx| {
                                if let Some(this) = this.upgrade() {
                                    this.update(cx, |this, cx| f(this, key.clone(), window, cx));
                                }
                            })
                        };
                        menu.item(item("Duplicate", |this, key, window, cx| {
                            this.duplicate_request(key, window, cx)
                        }))
                        .separator()
                        .item(item("Delete…", |this, key, window, cx| {
                            this.delete_request(key, window, cx)
                        }))
                    })
                    .into_any_element()
            }
        }
    }

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let count = self.rows.len();
        let dir = self.session.dir().to_string_lossy().to_string();
        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground)
            .child(
                h_flex()
                    .p_2()
                    .gap_1()
                    .child(div().flex_1().child(Input::new(&self.filter).small().cleanable(true)))
                    .child(
                        Button::new("add-collection")
                            .ghost()
                            .small()
                            .icon(IconName::Plus)
                            .tooltip("New or import collection")
                            .dropdown_menu({
                                let this = cx.entity().downgrade();
                                move |menu, _, _| {
                                    let item = |label: &'static str, f: fn(&mut RocketApp, &mut Window, &mut Context<RocketApp>)| {
                                        let this = this.clone();
                                        PopupMenuItem::new(label).on_click(move |_, window, cx| {
                                            if let Some(this) = this.upgrade() {
                                                this.update(cx, |this, cx| f(this, window, cx));
                                            }
                                        })
                                    };
                                    menu.item(item("New Collection…", |this, window, cx| {
                                        this.new_collection(&NewCollection, window, cx)
                                    }))
                                    .item(item("Import OpenAPI File…", |this, window, cx| {
                                        this.import_openapi(&ImportOpenApi, window, cx)
                                    }))
                                }
                            }),
                    ),
            )
            .child(
                div().flex_1().min_h_0().px_1().child(
                    uniform_list(
                        "sidebar",
                        count,
                        cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                            range.map(|ix| this.render_row(ix, cx)).collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(&self.list_scroll)
                    .size_full(),
                ),
            )
            .when(count == 0, |this| {
                this.child(
                    v_flex()
                        .absolute()
                        .top(px(60.))
                        .left_0()
                        .right_0()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("No collections yet")
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new("create-first")
                                        .small()
                                        .label("New collection")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.new_collection(&NewCollection, window, cx)
                                        })),
                                )
                                .child(
                                    Button::new("import-first")
                                        .small()
                                        .label("Import OpenAPI")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.import_openapi(&ImportOpenApi, window, cx)
                                        })),
                                ),
                        ),
                )
            })
            .child(
                h_flex()
                    .id("workspace-dir")
                    .px_2()
                    .py_1()
                    .gap_1()
                    .border_t_1()
                    .border_color(theme.sidebar_border)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .cursor_pointer()
                    .child(Icon::new(IconName::FolderOpen).xsmall())
                    .child(div().flex_1().min_w_0().truncate().child(dir))
                    .tooltip(|window, cx| Tooltip::new("Open another folder (⌘O)").build(window, cx))
                    .on_click(cx.listener(|this, _, window, cx| this.open_folder(&OpenFolder, window, cx))),
            )
    }

    // ----- request editor --------------------------------------------------

    fn mark_dirty(&mut self, cx: &mut Context<Self>) {
        if self.loading || self.active.is_none() {
            return;
        }
        if !self.dirty {
            self.dirty = true;
            cx.notify();
        }
    }

    fn sync_path_params(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self.url.read(cx).value();
        let names = path_templates(&url);
        let current = self.path_params.read(cx).params(cx);
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
                rocketman::openapi::Param::new(n, value)
            })
            .collect();
        self.path_params
            .update(cx, |kv, cx| kv.set_params(&params, window, cx));
    }

    fn snapshot(&self, cx: &App) -> RequestSpec {
        RequestSpec {
            name: self.name.read(cx).value().to_string(),
            method: self.method,
            url: self.url.read(cx).value().to_string(),
            path_params: self.path_params.read(cx).params(cx),
            query: self.query.read(cx).params(cx),
            headers: self.headers.read(cx).params(cx),
            content_type: self.content_type.clone(),
            body: if self.content_type.is_empty() {
                String::new()
            } else {
                self.body.read(cx).value().to_string()
            },
        }
    }

    fn load_into_editor(&mut self, spec: &RequestSpec, window: &mut Window, cx: &mut Context<Self>) {
        self.loading = true;
        self.method = spec.method;
        self.content_type = spec.content_type.clone();
        self.name
            .update(cx, |s, cx| s.set_value(spec.name.clone(), window, cx));
        self.url
            .update(cx, |s, cx| s.set_value(spec.url.clone(), window, cx));
        self.path_params
            .update(cx, |kv, cx| kv.set_params(&spec.path_params, window, cx));
        self.query
            .update(cx, |kv, cx| kv.set_params(&spec.query, window, cx));
        self.headers
            .update(cx, |kv, cx| kv.set_params(&spec.headers, window, cx));
        let language = body_language(&spec.content_type);
        self.body.update(cx, |s, cx| {
            if s.language_name() != language {
                s.set_highlighter(language, cx);
            }
            s.set_value(spec.body.clone(), window, cx);
        });
        self.loading = false;
        cx.notify();
    }

    /// Remembers unsaved edits of the open request so switching away never loses them.
    fn stash_active(&mut self, cx: &mut Context<Self>) {
        if let Some(key) = self.active.clone() {
            if self.dirty {
                let spec = self.snapshot(cx);
                self.session.drafts.insert(key, spec);
            }
        }
    }

    fn open_request(&mut self, file: PathBuf, key: OpKey, window: &mut Window, cx: &mut Context<Self>) {
        let target = (file, key);
        if self.active.as_ref() == Some(&target) {
            return;
        }
        self.stash_active(cx);
        let Some((spec, dirty)) = self.session.take_request(&target) else {
            return;
        };
        self.active = Some(target);
        self.load_into_editor(&spec, window, cx);
        self.dirty = dirty;
        if spec.content_type.is_empty() && self.request_tab == 2 && !spec.method.has_body() {
            self.request_tab = 0;
        }
        cx.notify();
    }

    fn save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        let Some((file, key)) = self.active.clone() else {
            return;
        };
        let spec = self.snapshot(cx);
        match self.session.save_request(&file, &key, &spec) {
            Ok(new_key) => {
                self.dirty = false;
                self.active = Some((file.clone(), new_key.clone()));
                // Reload: the stored form is canonical (e.g. URL query moved to params).
                if let Some(saved) = self.session.stored_request(&file, &new_key) {
                    if saved != spec {
                        self.load_into_editor(&saved, window, cx);
                    }
                }
                self.rebuild_rows(cx);
            }
            Err(e) => window.push_notification(Notification::error(format!("{e:#}")), cx),
        }
    }

    fn save_all(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.stash_active(cx);
        let errors = self.session.save_drafts();
        let ok = errors.is_empty();
        for e in errors {
            window.push_notification(Notification::error(format!("{e:#}")), cx);
        }
        if ok {
            self.dirty = false;
        }
        self.rebuild_rows(cx);
        ok
    }

    fn new_request(&mut self, _: &NewRequest, window: &mut Window, cx: &mut Context<Self>) {
        match self.session.default_collection(self.active.as_ref()) {
            Some(file) => self.new_request_in(&file, window, cx),
            None => self.new_collection(&NewCollection, window, cx),
        }
    }

    fn new_request_in(&mut self, file: &Path, window: &mut Window, cx: &mut Context<Self>) {
        match self.session.new_request(file) {
            Ok(key) => {
                self.rebuild_rows(cx);
                self.open_request(file.to_path_buf(), key, window, cx);
                // Deferred so a closing dialog can't restore focus over it.
                cx.defer_in(window, |this, window, cx| this.focus_url(&FocusUrl, window, cx));
            }
            Err(e) => window.push_notification(Notification::error(format!("{e:#}")), cx),
        }
    }

    fn duplicate_request(&mut self, (file, key): DraftKey, window: &mut Window, cx: &mut Context<Self>) {
        match self.session.duplicate_request(&file, &key) {
            Ok(new_key) => {
                self.rebuild_rows(cx);
                self.open_request(file, new_key, window, cx);
            }
            Err(e) => window.push_notification(Notification::error(format!("{e:#}")), cx),
        }
    }

    fn delete_request(&mut self, (file, key): DraftKey, window: &mut Window, cx: &mut Context<Self>) {
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Delete {} {}?", key.method, key.path),
            Some("The operation is removed from the OpenAPI file."),
            &["Delete", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                if let Err(e) = this.session.delete_request(&file, &key) {
                    window.push_notification(Notification::error(format!("{e:#}")), cx);
                    return;
                }
                let draft_key = (file, key);
                if this.active.as_ref() == Some(&draft_key) {
                    this.close_editor(window, cx);
                }
                this.rebuild_rows(cx);
            });
        })
        .detach();
    }

    fn close_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.active = None;
        self.dirty = false;
        self.load_into_editor(&RequestSpec::default(), window, cx);
    }

    fn prompt_name(
        &mut self,
        title: &'static str,
        initial: String,
        ok_label: &'static str,
        on_ok: impl Fn(&mut RocketApp, String, &mut Window, &mut Context<RocketApp>) -> bool + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("Collection name");
            s.set_value(initial, window, cx);
            s
        });
        let this = cx.entity().downgrade();
        let on_ok = std::rc::Rc::new(on_ok);
        let submit = {
            let input = input.clone();
            let this = this.clone();
            let on_ok = on_ok.clone();
            move |window: &mut Window, cx: &mut App| {
                let value = input.read(cx).value().trim().to_string();
                if value.is_empty() {
                    return;
                }
                // Deferred: Enter arrives through a subscription that runs while
                // RocketApp is already being updated.
                let this = this.clone();
                let on_ok = on_ok.clone();
                window.defer(cx, move |window, cx| {
                    let done = this
                        .upgrade()
                        .map(|this| this.update(cx, |this, cx| on_ok(this, value, window, cx)))
                        .unwrap_or(true);
                    if done {
                        window.close_dialog(cx);
                    }
                });
            }
        };
        let submit = std::rc::Rc::new(submit);
        let sub_submit = submit.clone();
        let sub = cx.subscribe_in(&input, window, move |_, _, e: &InputEvent, window, cx| {
            if matches!(e, InputEvent::PressEnter { .. }) {
                sub_submit(window, cx);
            }
        });
        // The dialog hands focus back to whatever had it before it opened, once
        // its exit animation ends. Make that the URL field, where work continues.
        self.focus_url(&FocusUrl, window, cx);
        let initial_focus = std::cell::Cell::new(true);
        let sub = std::cell::RefCell::new(Some(sub));
        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                input.update(cx, |s, cx| {
                    s.focus(window, cx);
                    s.select_all(window, cx);
                });
            }
            let _keep_alive = &sub;
            let submit = submit.clone();
            dialog
                .title(title)
                .w(px(420.))
                .child(Input::new(&input))
                .footer(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("ok")
                                .primary()
                                .label(ok_label)
                                .on_click(move |_, window, cx| submit(window, cx)),
                        ),
                )
        });
    }

    fn new_collection(&mut self, _: &NewCollection, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt_name(
            "New collection",
            String::new(),
            "Create",
            |this, name, window, cx| match this.session.create_collection(&name) {
                Ok(file) => {
                    this.rebuild_rows(cx);
                    this.new_request_in(&file, window, cx);
                    true
                }
                Err(e) => {
                    window.push_notification(Notification::error(format!("{e:#}")), cx);
                    false
                }
            },
            window,
            cx,
        );
    }

    fn rename_collection(&mut self, file: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some(title) = self.session.collection_title(file) else {
            return;
        };
        let file = file.to_path_buf();
        self.prompt_name(
            "Rename collection",
            title,
            "Rename",
            move |this, name, window, cx| {
                if let Err(e) = this.session.rename_collection(&file, &name) {
                    window.push_notification(Notification::error(format!("{e:#}")), cx);
                    return false;
                }
                this.rebuild_rows(cx);
                true
            },
            window,
            cx,
        );
    }

    fn delete_collection(&mut self, file: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some(title) = self.session.collection_title(file) else {
            return;
        };
        let answer = window.prompt(
            PromptLevel::Critical,
            &format!("Delete collection \"{title}\"?"),
            Some(&format!("This deletes {}.", file.display())),
            &["Delete", "Cancel"],
            cx,
        );
        let file = file.to_path_buf();
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                if let Err(e) = this.session.delete_collection(&file) {
                    window.push_notification(Notification::error(format!("{e:#}")), cx);
                    return;
                }
                if this.active.as_ref().is_some_and(|(f, _)| f == &file) {
                    this.close_editor(window, cx);
                }
                this.rebuild_rows(cx);
            });
        })
        .detach();
    }

    fn import_openapi(&mut self, _: &ImportOpenApi, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Import".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| this.import_files(&paths, window, cx));
        })
        .detach();
    }

    fn import_files(&mut self, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        let (notices, open) = self.session.import_files(paths);
        for notice in notices {
            notify(window, notice, cx);
        }
        self.rebuild_rows(cx);
        if let Some((file, key)) = open {
            self.open_request(file, key, window, cx);
        }
    }

    fn open_folder(&mut self, _: &OpenFolder, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(dir) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| this.switch_workspace(dir, window, cx));
        })
        .detach();
    }

    fn switch_workspace(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if !self.save_all(window, cx) {
            return;
        }
        self.session.switch_workspace(&dir);
        self.close_editor(window, cx);
        self.after_workspace_load(window, cx);
    }

    fn reload(&mut self, _: &Reload, window: &mut Window, cx: &mut Context<Self>) {
        self.stash_active(cx);
        let active = self.active.take();
        self.dirty = false;
        self.session.reload();
        for notice in self.session.load_errors() {
            notify(window, notice, cx);
        }
        self.rebuild_rows(cx);
        match active {
            Some(target) if self.session.contains(&target) => {
                self.open_request(target.0, target.1, window, cx)
            }
            _ => self.close_editor(window, cx),
        }
    }

    fn quit(&mut self, _: &Quit, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm_close(window, cx) {
            cx.quit();
        }
    }

    fn focus_url(&mut self, _: &FocusUrl, window: &mut Window, cx: &mut Context<Self>) {
        self.url.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
    }

    fn set_method(&mut self, method: Method, cx: &mut Context<Self>) {
        if self.method != method {
            self.method = method;
            self.mark_dirty(cx);
            cx.notify();
        }
    }

    fn set_content_type(&mut self, ct: &str, cx: &mut Context<Self>) {
        if self.content_type == ct {
            return;
        }
        self.content_type = ct.to_string();
        let language = body_language(ct);
        self.body.update(cx, |s, cx| {
            if s.language_name() != language {
                s.set_highlighter(language, cx);
            }
        });
        self.mark_dirty(cx);
        cx.notify();
    }

    // ----- sending ---------------------------------------------------------

    fn send(&mut self, _: &Send, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.response, ResponseState::Loading { .. }) {
            return;
        }
        let spec = self.snapshot(cx);
        let prepared = match http::prepare(&spec) {
            Ok(p) => p,
            Err(e) => {
                self.response = ResponseState::Failed(format!("{e:#}").into());
                cx.notify();
                return;
            }
        };
        self.request_seq += 1;
        let seq = self.request_seq;
        let (tx, rx) = futures::channel::oneshot::channel();
        if let Err(e) = http::spawn(prepared, move |result| {
            let _ = tx.send(result);
        }) {
            self.response = ResponseState::Failed(format!("Could not start request: {e}").into());
            cx.notify();
            return;
        }
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = rx.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.request_seq != seq {
                    return;
                }
                match result {
                    Ok(Ok(resp)) => this.show_response(resp, window, cx),
                    Ok(Err(e)) => this.response = ResponseState::Failed(format!("{e:#}").into()),
                    Err(_) => this.response = ResponseState::Failed("Request aborted".into()),
                }
                cx.notify();
            });
        });
        self.response = ResponseState::Loading {
            started: Instant::now(),
            _task: task,
        };
        cx.notify();
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.response, ResponseState::Loading { .. }) {
            self.request_seq += 1;
            self.response = ResponseState::Failed("Request cancelled".into());
            cx.notify();
        } else {
            cx.propagate();
        }
    }

    fn show_response(&mut self, resp: Response, window: &mut Window, cx: &mut Context<Self>) {
        let language = resp.language;
        let text = resp.text.clone();
        self.response_body.update(cx, |s, cx| {
            if s.language_name() != language {
                s.set_highlighter(language, cx);
            }
            s.set_value(text, window, cx);
        });
        self.response = ResponseState::Done(Arc::new(resp));
    }

    fn copy_response(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let ResponseState::Done(resp) = &self.response {
            let text = if resp.text.is_empty() {
                String::from_utf8_lossy(&resp.body).into_owned()
            } else {
                resp.text.clone()
            };
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            window.push_notification(Notification::success("Response copied"), cx);
        }
    }

    fn save_response(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ResponseState::Done(resp) = &self.response else {
            return;
        };
        let body = resp.body.clone();
        let ext = match resp.language {
            "json" => "json",
            _ => "txt",
        };
        let dir = dirs::download_dir().unwrap_or_else(|| self.session.dir().to_path_buf());
        let path = cx.prompt_for_new_path(&dir, Some(&format!("response.{ext}")));
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(path))) = path.await else {
                return;
            };
            let result = std::fs::write(&path, body.as_slice());
            let _ = cx.update(|window, cx| match result {
                Ok(()) => window.push_notification(
                    Notification::success(format!("Saved to {}", path.display())),
                    cx,
                ),
                Err(e) => window.push_notification(Notification::error(e.to_string()), cx),
            });
        })
        .detach();
    }

    // ----- rendering -------------------------------------------------------

    fn render_request(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let method = self.method;
        let this = cx.entity().downgrade();
        let loading = matches!(self.response, ResponseState::Loading { .. });
        let query_count = self.query.read(cx).filled_count(cx);
        let header_count = self.headers.read(cx).filled_count(cx);
        let has_path_params = !self.path_params.read(cx).is_empty();
        let ct = self.content_type.clone();
        let ct_label = SharedString::from(content_type_label(&ct));

        let tab_label = |label: &str, count: usize| -> SharedString {
            if count > 0 {
                format!("{label} ({count})").into()
            } else {
                label.to_string().into()
            }
        };

        v_flex()
            .size_full()
            .p_3()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div().flex_1().child(
                            Input::new(&self.name)
                                .appearance(false)
                                .text_lg()
                                .font_semibold(),
                        ),
                    )
                    .child(
                        Button::new("save")
                            .small()
                            .ghost()
                            .label(if self.dirty { "Save •" } else { "Save" })
                            .tooltip("Save (⌘S)")
                            .disabled(!self.dirty)
                            .on_click(cx.listener(|this, _, window, cx| this.save(&Save, window, cx))),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("method")
                            .outline()
                            .w(px(104.))
                            .child(
                                div()
                                    .flex_1()
                                    .font_semibold()
                                    .text_color(method_color(method, cx))
                                    .child(method.as_str()),
                            )
                            .dropdown_caret(true)
                            .dropdown_menu({
                                let this = this.clone();
                                move |mut menu, _, _| {
                                    for m in Method::PICKER {
                                        let this = this.clone();
                                        menu = menu.item(
                                            PopupMenuItem::new(m.as_str())
                                                .checked(m == method)
                                                .on_click(move |_, _, cx| {
                                                    if let Some(this) = this.upgrade() {
                                                        this.update(cx, |this, cx| this.set_method(m, cx));
                                                    }
                                                }),
                                        );
                                    }
                                    menu
                                }
                            }),
                    )
                    .child(div().flex_1().child(Input::new(&self.url).font_family(cx.theme().mono_font_family.clone())))
                    .child(if loading {
                        Button::new("cancel-send")
                            .w(px(90.))
                            .label("Cancel")
                            .tooltip("Cancel (Esc)")
                            .on_click(cx.listener(|this, _, window, cx| this.cancel(&Cancel, window, cx)))
                    } else {
                        Button::new("send")
                            .primary()
                            .w(px(90.))
                            .label("Send")
                            .tooltip("Send (⌘↵)")
                            .on_click(cx.listener(|this, _, window, cx| this.send(&Send, window, cx)))
                    }),
            )
            .child(
                TabBar::new("request-tabs")
                    .underline()
                    .small()
                    .selected_index(self.request_tab)
                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                        this.request_tab = *ix;
                        cx.notify();
                    }))
                    .child(Tab::new().label(tab_label("Params", query_count)))
                    .child(Tab::new().label(tab_label("Headers", header_count)))
                    .child(Tab::new().label(if ct.is_empty() {
                        SharedString::from("Body")
                    } else {
                        SharedString::from(format!("Body ({ct_label})"))
                    })),
            )
            .child(
                div()
                    .id("request-tab-content")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .map(|el| match self.request_tab {
                        0 => el.child(
                            v_flex()
                                .gap_3()
                                .when(has_path_params, |this| {
                                    this.child(section_label("Path variables", cx))
                                        .child(self.path_params.clone())
                                })
                                .child(section_label("Query parameters", cx))
                                .child(self.query.clone()),
                        ),
                        1 => el.child(self.headers.clone()),
                        _ => el.child(
                            v_flex()
                                .size_full()
                                .gap_2()
                                .child(
                                    h_flex().gap_2().child(section_label("Content type", cx)).child(
                                        Button::new("content-type")
                                            .small()
                                            .outline()
                                            .label(ct_label.clone())
                                            .dropdown_caret(true)
                                            .dropdown_menu({
                                                let this = this.clone();
                                                let ct = ct.clone();
                                                move |mut menu, _, _| {
                                                    let entries = std::iter::once(("None", ""))
                                                        .chain(BODY_TYPES.iter().copied());
                                                    for (label, value) in entries {
                                                        let this = this.clone();
                                                        menu = menu.item(
                                                            PopupMenuItem::new(label)
                                                                .checked(ct == value)
                                                                .on_click(move |_, _, cx| {
                                                                    if let Some(this) = this.upgrade() {
                                                                        this.update(cx, |this, cx| {
                                                                            this.set_content_type(value, cx)
                                                                        });
                                                                    }
                                                                }),
                                                        );
                                                    }
                                                    menu
                                                }
                                            }),
                                    ),
                                )
                                .child(if ct.is_empty() {
                                    div()
                                        .text_sm()
                                        .text_color(theme.muted_foreground)
                                        .child("This request has no body.")
                                        .into_any_element()
                                } else {
                                    div()
                                        .flex_1()
                                        .min_h(px(120.))
                                        .child(Editor::new(&self.body).h_full())
                                        .into_any_element()
                                }),
                        ),
                    }),
            )
    }

    fn render_response(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let header = h_flex()
            .h(px(36.))
            .px_3()
            .gap_3()
            .border_b_1()
            .border_color(theme.border)
            .text_sm();
        match &self.response {
            ResponseState::Idle => v_flex()
                .size_full()
                .child(header.child(div().text_color(theme.muted_foreground).child("Response")))
                .child(
                    v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .gap_1()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Send a request to see the response")
                        .child(div().text_xs().child("⌘↵ send  ·  ⌘S save  ·  ⌘N new request")),
                )
                .into_any_element(),
            ResponseState::Loading { started, .. } => {
                let elapsed = started.elapsed();
                v_flex()
                    .size_full()
                    .child(header.child(Spinner::new().small()).child(format!(
                        "Sending… {}",
                        format_duration(elapsed)
                    )))
                    .into_any_element()
            }
            ResponseState::Failed(msg) => v_flex()
                .size_full()
                .child(header.child(div().text_color(theme.danger).child("Error")))
                .child(
                    div()
                        .p_3()
                        .text_sm()
                        .text_color(theme.danger)
                        .child(msg.clone()),
                )
                .into_any_element(),
            ResponseState::Done(resp) => {
                let resp = resp.clone();
                let status_color = match resp.status {
                    200..=299 => theme.green,
                    300..=399 => theme.blue,
                    400..=499 => theme.yellow,
                    _ => theme.red,
                };
                v_flex()
                    .size_full()
                    .child(
                        header
                            .child(
                                div()
                                    .font_semibold()
                                    .text_color(status_color)
                                    .child(format!("{} {}", resp.status, resp.reason)),
                            )
                            .child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .child(format_duration(resp.elapsed)),
                            )
                            .child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .child(format_size(resp.body.len())),
                            )
                            .child(div().flex_1())
                            .child(
                                TabBar::new("response-tabs")
                                    .segmented()
                                    .xsmall()
                                    .selected_index(self.response_tab)
                                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                                        this.response_tab = *ix;
                                        cx.notify();
                                    }))
                                    .child(Tab::new().label("Body"))
                                    .child(Tab::new().label(format!("Headers ({})", resp.headers.len()))),
                            )
                            .child(
                                Button::new("copy-response")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Copy)
                                    .tooltip("Copy body")
                                    .on_click(cx.listener(|this, _, window, cx| this.copy_response(window, cx))),
                            )
                            .child(
                                Button::new("save-response")
                                    .ghost()
                                    .xsmall()
                                    .label("Save")
                                    .tooltip("Save body to file")
                                    .on_click(cx.listener(|this, _, window, cx| this.save_response(window, cx))),
                            ),
                    )
                    .when_some(resp.note.clone(), |this, note| {
                        this.child(
                            div()
                                .px_3()
                                .py_1()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(note),
                        )
                    })
                    .child(if self.response_tab == 0 {
                        div()
                            .flex_1()
                            .min_h_0()
                            .child(
                                Editor::new(&self.response_body)
                                    .readonly(true)
                                    .bordered(false)
                                    .h_full(),
                            )
                            .into_any_element()
                    } else {
                        let headers = resp.headers.clone();
                        div()
                            .id("response-headers")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .p_3()
                            .child(
                                v_flex().gap_1().children(headers.into_iter().enumerate().map(
                                    |(ix, (k, v))| {
                                        let copy = v.clone();
                                        h_flex()
                                            .id(("header", ix))
                                            .gap_3()
                                            .items_start()
                                            .text_sm()
                                            .cursor_pointer()
                                            .rounded(theme.radius)
                                            .hover(|s| s.bg(theme.muted))
                                            .on_click(move |_, window, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    copy.clone(),
                                                ));
                                                window.push_notification(
                                                    Notification::success("Header value copied"),
                                                    cx,
                                                );
                                            })
                                            .child(
                                                div()
                                                    .w(px(220.))
                                                    .flex_none()
                                                    .font_semibold()
                                                    .text_color(theme.muted_foreground)
                                                    .child(k),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .font_family(cx.theme().mono_font_family.clone())
                                                    .child(v),
                                            )
                                    },
                                )),
                            )
                            .into_any_element()
                    })
                    .into_any_element()
            }
        }
    }
}

impl Focusable for RocketApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for RocketApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Keep the elapsed-time counter ticking while a request is in flight.
        if matches!(self.response, ResponseState::Loading { .. }) {
            window.request_animation_frame();
        }
        let sidebar = self.render_sidebar(cx).into_any_element();
        let main = if self.active.is_some() {
            v_resizable("main-split")
                .child(
                    resizable_panel()
                        .size(px(340.))
                        .size_range(px(160.)..Pixels::MAX)
                        .child(self.render_request(cx)),
                )
                .child(
                    resizable_panel()
                        .size_range(px(120.)..Pixels::MAX)
                        .child(self.render_response(cx)),
                )
                .into_any_element()
        } else {
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(cx.theme().muted_foreground)
                .child(div().text_lg().child("No request selected"))
                .child(div().text_sm().child("Pick one from the sidebar or press ⌘N to create one."))
                .into_any_element()
        };

        let theme = cx.theme();
        div()
            .id("rocketman")
            .key_context("Rocketman")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::send))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::new_request))
            .on_action(cx.listener(Self::new_collection))
            .on_action(cx.listener(Self::import_openapi))
            // Dropping OpenAPI files anywhere on the window imports them.
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.import_files(paths.paths(), window, cx)
            }))
            .drag_over::<ExternalPaths>(|style, _, _, cx| style.bg(cx.theme().drop_target))
            .on_action(cx.listener(Self::open_folder))
            .on_action(cx.listener(Self::reload))
            .on_action(cx.listener(Self::focus_url))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::quit))
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                h_resizable("sidebar-split")
                    .child(
                        resizable_panel()
                            .size(px(270.))
                            .size_range(px(180.)..px(600.))
                            .child(sidebar),
                    )
                    .child(resizable_panel().child(main)),
            )
    }
}

fn section_label(text: &'static str, cx: &App) -> impl IntoElement {
    div()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

fn method_color(method: Method, cx: &App) -> Hsla {
    let t = cx.theme();
    match method {
        Method::Get => t.green,
        Method::Post => t.yellow,
        Method::Put => t.blue,
        Method::Patch => t.magenta,
        Method::Delete => t.red,
        _ => t.cyan,
    }
}

/// Shows a core notice as a toast.
fn notify(window: &mut Window, notice: Notice, cx: &mut App) {
    let mut n = match notice.level {
        Level::Success => Notification::success(notice.text),
        Level::Warning => Notification::warning(notice.text),
        Level::Error => Notification::error(notice.text),
    };
    if let Some(title) = notice.title {
        n = n.title(title);
    }
    window.push_notification(n, cx);
}
