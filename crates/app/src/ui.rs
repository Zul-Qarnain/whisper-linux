//! Visual system shared by the overlay and settings window.
//! Palette: neutral charcoal surfaces, one soft indigo accent, semantic states.

use gpui::prelude::*;
use gpui::{div, px, rgb, Context, Div, ElementId, Hsla, Pixels, SharedString, Stateful, Window};

pub const BG: u32 = 0x14161b;
pub const SURFACE: u32 = 0x1c1f26;
pub const RAISED: u32 = 0x232730;
pub const BORDER: u32 = 0x2e333e;
pub const TEXT: u32 = 0xe6e8ec;
pub const MUTED: u32 = 0x8a91a0;
pub const ACCENT: u32 = 0x7c9eff;
pub const ACCENT_DIM: u32 = 0x2c3550;
pub const OK: u32 = 0x4ade80;
pub const WARN: u32 = 0xfbbf24;
pub const REC: u32 = 0xf87171;

pub fn card() -> Div {
    div().flex().flex_col().gap_2().p_3().rounded(px(12.)).bg(rgb(SURFACE)).border_1().border_color(rgb(BORDER))
}

pub fn section_label(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(10.5))
        .text_color(rgb(MUTED))
        .mt_1()
        .child(text.into())
}

/// Pill-shaped interactive button. `primary` fills with the accent,
/// `danger` tints red text; both brighten on hover.
pub fn pill<C: 'static>(
    id: &str,
    label: impl Into<SharedString>,
    primary: bool,
    danger: bool,
    active: bool,
    cx: &mut Context<C>,
    on_click: impl Fn(&mut C, &gpui::ClickEvent, &mut Window, &mut Context<C>) + 'static,
) -> Stateful<Div> {
    let bg = rgb(if primary { ACCENT_DIM } else { RAISED });
    let fg = rgb(if primary { ACCENT } else if danger { 0xe08a86 } else { TEXT });
    div()
        .id(ElementId::Name(SharedString::from(format!("pill-{id}"))))
        .px_3()
        .py_1()
        .rounded(px(999.))
        .bg(bg)
        .when(active, |d| d.border_1().border_color(rgb(ACCENT)))
        .text_color(fg)
        .text_size(px(12.))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(if primary { 0x39466b } else { 0x2c313c })))
        .child(label.into())
        .on_click(cx.listener(on_click))
}

/// Five-segment level meter driven by RMS.
pub fn level_meter(level: f32, max: f32) -> Div {
    let heights = [8.0f32, 14.0, 18.0, 14.0, 8.0];
    let lit = (level / max * 5.0).clamp(0.0, 5.0);
    div().flex().items_end().gap(px(2.)).children(
        heights
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let on = (i as f32) < lit;
                div()
                    .w(px(3.))
                    .h(px(if on { *h } else { 3.0 }))
                    .rounded(px(2.))
                    .bg(rgb(if on { ACCENT } else { BORDER }))
            })
            .collect::<Vec<_>>(),
    )
}

/// Horizontal capacity gauge (0..1) with a budget threshold marker.
pub fn gauge(frac: f32, width: Pixels) -> Div {
    let frac = frac.clamp(0.0, 1.0);
    let color = rgb(if frac > 0.95 { REC } else if frac > 0.8 { WARN } else { OK });
    div()
        .h(px(5.))
        .w(width)
        .rounded(px(3.))
        .bg(rgb(BG))
        .child(div().h(px(5.)).w(px(180.0 * frac)).rounded(px(3.)).bg(color))
}
