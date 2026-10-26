//! Editable key/value table used for query parameters, path variables and headers.

use rocketman::openapi::Param;
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, h_flex,
    button::{Button, ButtonVariants as _},
    input::{Input, InputEvent, InputState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

pub enum KvEvent {
    Changed,
}

struct Row {
    key: Entity<InputState>,
    value: Entity<InputState>,
    _subs: [Subscription; 2],
}

pub struct KvEditor {
    id: SharedString,
    rows: Vec<Row>,
    /// Keys come from elsewhere (path variables): no adding, removing or renaming.
    fixed_keys: bool,
    key_placeholder: SharedString,
    value_placeholder: SharedString,
}

impl EventEmitter<KvEvent> for KvEditor {}

impl KvEditor {
    pub fn new(
        id: impl Into<SharedString>,
        key_placeholder: impl Into<SharedString>,
        value_placeholder: impl Into<SharedString>,
        fixed_keys: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            id: id.into(),
            rows: Vec::new(),
            fixed_keys,
            key_placeholder: key_placeholder.into(),
            value_placeholder: value_placeholder.into(),
        };
        this.set_params(&[], window, cx);
        this
    }

    fn new_row(&self, window: &mut Window, cx: &mut Context<Self>) -> Row {
        let key = cx.new(|cx| InputState::new(window, cx).placeholder(self.key_placeholder.clone()));
        let value =
            cx.new(|cx| InputState::new(window, cx).placeholder(self.value_placeholder.clone()));
        let subs = [
            cx.subscribe_in(&key, window, Self::on_input),
            cx.subscribe_in(&value, window, Self::on_input),
        ];
        Row {
            key,
            value,
            _subs: subs,
        }
    }

    fn on_input(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(event, InputEvent::Change) {
            return;
        }
        if !self.fixed_keys && self.last_row_used(cx) {
            let row = self.new_row(window, cx);
            self.rows.push(row);
        }
        cx.emit(KvEvent::Changed);
        cx.notify();
    }

    fn last_row_used(&self, cx: &App) -> bool {
        self.rows.last().is_none_or(|r| {
            !r.key.read(cx).value().is_empty() || !r.value.read(cx).value().is_empty()
        })
    }

    /// Replaces the contents without emitting change events. Existing input
    /// entities are reused so switching requests stays cheap.
    pub fn set_params(&mut self, params: &[Param], window: &mut Window, cx: &mut Context<Self>) {
        let wanted = params.len() + usize::from(!self.fixed_keys);
        while self.rows.len() < wanted {
            let row = self.new_row(window, cx);
            self.rows.push(row);
        }
        self.rows.truncate(wanted);
        for (i, row) in self.rows.iter().enumerate() {
            let (k, v) = params
                .get(i)
                .map(|p| (p.name.clone(), p.value.clone()))
                .unwrap_or_default();
            set_if_changed(&row.key, k, window, cx);
            set_if_changed(&row.value, v, window, cx);
        }
        cx.notify();
    }

    pub fn params(&self, cx: &App) -> Vec<Param> {
        self.rows
            .iter()
            .map(|r| Param::new(r.key.read(cx).value(), r.value.read(cx).value()))
            .filter(|p| !p.name.trim().is_empty())
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn filled_count(&self, cx: &App) -> usize {
        self.rows
            .iter()
            .filter(|r| !r.key.read(cx).value().trim().is_empty())
            .count()
    }

    fn remove_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.rows.len() {
            self.rows.remove(ix);
        }
        if self.last_row_used(cx) {
            let row = self.new_row(window, cx);
            self.rows.push(row);
        }
        cx.emit(KvEvent::Changed);
        cx.notify();
    }
}

fn set_if_changed(
    input: &Entity<InputState>,
    value: String,
    window: &mut Window,
    cx: &mut App,
) {
    input.update(cx, |state, cx| {
        if state.value() != value.as_str() {
            state.set_value(value, window, cx);
        }
    });
}

impl Render for KvEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let last = self.rows.len().saturating_sub(1);
        let fixed = self.fixed_keys;
        v_flex()
            .id(self.id.clone())
            .gap_1()
            .w_full()
            .children(self.rows.iter().enumerate().map(|(ix, row)| {
                let removable = !fixed && ix != last;
                h_flex()
                    .gap_1()
                    .w_full()
                    .child(
                        div()
                            .w(relative(0.38))
                            .min_w_0()
                            .child(Input::new(&row.key).small().disabled(fixed)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&row.value).small()),
                    )
                    .when(!fixed, |this| {
                        this.child(
                            div().w(px(24.)).child(
                                Button::new(("remove", ix))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Close)
                                    .tooltip("Remove")
                                    .when(!removable, |b| b.invisible())
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.remove_row(ix, window, cx)
                                    })),
                            ),
                        )
                    })
            }))
            .when(self.rows.is_empty(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("No path variables. Use {name} in the URL to add one."),
                )
            })
    }
}
