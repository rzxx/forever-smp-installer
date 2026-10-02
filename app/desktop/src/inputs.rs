use gpui::{prelude::*, *};
use gpui_component::{
    Theme, ThemeMode,
    input::{Copy, Cut, Enter, Input, InputState, MoveEnd, MoveHome, Paste, Redo},
};

pub fn init(cx: &mut App) {
    if cx.has_global::<Theme>() {
        return;
    }
    gpui_component::init(cx);
    Theme::change(ThemeMode::Dark, None, cx);
    // Keep the existing installer palette, including selection and context menus.
    let theme = Theme::global_mut(cx);
    theme.font_family = "Segoe UI".into();
    theme.shadow = false;
    theme.background = rgb(0x142019).into();
    theme.foreground = rgb(0xdae9e1).into();
    theme.input = rgb(0x456052).into();
    theme.caret = rgb(0x80d4b0).into();
    theme.selection = rgba(0x80d4b050).into();
    theme.muted_foreground = rgb(0xa4baad).into();
    theme.popover = rgb(0x24312c).into();
    theme.popover_foreground = rgb(0xdae9e1).into();
    theme.accent = rgb(0x35463e).into();
    theme.accent_foreground = rgb(0xdae9e1).into();
    theme.ring = rgb(0x80d4b0).into();
    theme.primary = rgb(0x80d4b0).into();
    theme.primary_foreground = rgb(0x102019).into();
    theme.primary_hover = rgb(0xa6ebca).into();
    theme.primary_active = rgb(0x69c19c).into();
    theme.secondary = rgb(0x24312c).into();
    theme.secondary_foreground = rgb(0xdae9e1).into();
    theme.secondary_hover = rgb(0x3b5146).into();
    theme.secondary_active = rgb(0x456052).into();
    cx.bind_keys([
        KeyBinding::new("ctrl-shift-z", Redo, Some("Input")),
        KeyBinding::new("ctrl-home", MoveHome, Some("Input")),
        KeyBinding::new("ctrl-end", MoveEnd, Some("Input")),
        KeyBinding::new("shift-insert", Paste, Some("Input")),
        KeyBinding::new("ctrl-insert", Copy, Some("Input")),
        KeyBinding::new("shift-delete", Cut, Some("Input")),
    ]);
}

pub fn field(
    id: &'static str,
    state: &Entity<InputState>,
    window: &Window,
    cx: &App,
) -> Stateful<Div> {
    let focused = state.read(cx).focus_handle(cx).is_focused(window);
    let paste_state = state.clone();
    let input = Input::new(state)
        .focus_bordered(false)
        .p_3()
        .rounded_md()
        .border_2()
        .border_color(rgb(if focused { 0x80d4b0 } else { 0x456052 }))
        .bg(rgb(if focused { 0x1c3026 } else { 0x25352d }));
    div()
        .id(id)
        .debug_selector(move || id.into())
        // Stretch in a column. Percentage width can resolve against the input's
        // intrinsic width (just its padding) before the parent has a definite size.
        .flex()
        .w_auto()
        .min_w_0()
        // Single-line Enter emits PressEnter; don't forward a newline to the OS handler.
        .on_action(|_: &Enter, _, cx| cx.stop_propagation())
        .capture_action(move |_: &Paste, window, cx| {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text())
                && text.contains('\r')
            {
                // The component removes LF on paste; also handle Windows CRLF.
                // Keep clipboard contents intact and use the component's history.
                let text = text.replace(['\r', '\n'], "");
                paste_state.update(cx, |input, cx| {
                    input.replace_text_in_range(None, &text, window, cx)
                });
                cx.stop_propagation();
            }
        })
        .child(Styled::h(input, px(54.)))
}
