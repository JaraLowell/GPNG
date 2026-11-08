//! One face for every menu and popup.
//!
//! Call these painters instead of egui's stock buttons. A row is a raised
//! area (white along the left and top, black along the bottom and right).
//! The bottom of a form is always Apply. The main window's top buttons
//! use the same edges with a slightly rounder corner.

use egui::{Color32, FontId, Rect, Response, Sense, Stroke, Ui, Vec2};

/// Channel row and clock badges: a little more round than a menu row.
pub const TOP_ROUND: f32 = 4.0;
/// Menu rows and sunken fields.
pub const ROW_ROUND: f32 = 2.0;
pub const ROW_W: f32 = 460.0;
/// Short enough that a line of text sits in the middle, not in a tall well.
pub const ROW_H: f32 = 26.0;

/// Setting you can change here (green mark).
pub const TAB_VALUE: Color32 = Color32::from_rgb(0, 120, 0);
/// Row that opens another box (gold mark).
pub const TAB_OPEN: Color32 = Color32::from_rgb(170, 110, 0);

const FACE: Color32 = Color32::from_rgb(192, 192, 192);
const FACE_HOVER: Color32 = Color32::from_rgb(214, 214, 214);
const FIELD: Color32 = Color32::from_rgb(236, 236, 236);
const SELECT: Color32 = Color32::from_rgb(0, 0, 128);
const EDGE: f32 = 2.0;

/// Raised or sunken face. `pressed` sinks it (black on the left and top).
pub fn paint(painter: &egui::Painter, rect: Rect, fill: Color32, pressed: bool, rounding: f32) {
    paint_edge(painter, rect, fill, pressed, rounding, EDGE);
}

/// Same 3D edges, with a thinner rim for short badges.
pub fn paint_edge(
    painter: &egui::Painter,
    rect: Rect,
    fill: Color32,
    pressed: bool,
    rounding: f32,
    edge: f32,
) {
    if rect.width() < 4.0 || rect.height() < 4.0 {
        return;
    }
    let round = rounding.max(0.0);
    let edge = edge.clamp(1.0, rect.width().min(rect.height()) * 0.3);
    let (top_left, bottom_right) = if pressed {
        (Color32::BLACK, Color32::WHITE)
    } else {
        (Color32::WHITE, Color32::BLACK)
    };
    painter.rect_filled(rect, round, top_left);
    let shadow = Rect::from_min_max(rect.min + Vec2::splat(edge), rect.max);
    if shadow.width() > 1.0 && shadow.height() > 1.0 {
        painter.rect_filled(shadow, (round - edge * 0.35).max(0.0), bottom_right);
    }
    let inner = Rect::from_min_max(rect.min + Vec2::splat(edge), rect.max - Vec2::splat(edge));
    if inner.width() > 1.0 && inner.height() > 1.0 {
        painter.rect_filled(inner, (round - edge).max(0.0), fill);
    }
}

/// One menu line. Label on the left, value on the right. A coloured mark
/// shows a setting (green) or a row that opens another box (gold).
pub fn entry(
    ui: &mut Ui,
    label: &str,
    value: &str,
    tab: Color32,
    selected: bool,
    width: f32,
    font: FontId,
) -> Response {
    let width = width.max(160.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, ROW_H), Sense::click());
    let down = selected || resp.is_pointer_button_down_on();
    let fill = if selected {
        SELECT
    } else if resp.hovered() {
        FACE_HOVER
    } else {
        FACE
    };
    paint(ui.painter(), rect, fill, down, ROW_ROUND);
    let fg = if selected {
        Color32::WHITE
    } else {
        Color32::BLACK
    };
    let shift = if down {
        Vec2::new(1.0, 1.0)
    } else {
        Vec2::ZERO
    };
    let show_tab = tab.a() > 0;
    let text_left = if show_tab { 24.0 } else { 10.0 };
    // Glyphs sit low in the face; 3px up puts them on the middle line.
    let mid_y = rect.center().y + shift.y - 3.0;
    if show_tab {
        let tab_rect = Rect::from_center_size(
            egui::pos2(rect.left() + 12.0, mid_y) + Vec2::new(shift.x, 0.0),
            Vec2::splat(10.0),
        );
        ui.painter().rect_filled(tab_rect, 2.0, tab);
    }
    let right_pad = 12.0;
    let gap = 14.0;
    let usable = (width - text_left - right_pad).max(40.0);
    let value_nat = if value.is_empty() {
        0.0
    } else {
        text_width(ui, value, &font)
    };
    let label_max = if value.is_empty() {
        usable
    } else {
        (usable - gap - value_nat.min(usable * 0.46)).max(48.0)
    };
    let label_draw = fit_text(ui, label, &font, label_max);
    let value_max = if value.is_empty() {
        0.0
    } else {
        (usable - gap - text_width(ui, &label_draw, &font)).max(24.0)
    };
    let value_draw = if value.is_empty() {
        String::new()
    } else {
        fit_text(ui, value, &font, value_max)
    };
    // Label on the left, value on the right, both on the middle of the row.
    let text_y = mid_y;
    ui.painter().text(
        egui::pos2(rect.left() + text_left + shift.x, text_y),
        egui::Align2::LEFT_CENTER,
        label_draw,
        font.clone(),
        fg,
    );
    if !value_draw.is_empty() {
        ui.painter().text(
            egui::pos2(rect.right() - right_pad + shift.x, text_y),
            egui::Align2::RIGHT_CENTER,
            &value_draw,
            font,
            fg,
        );
    }
    let resp = if !value.is_empty() && value_draw != value {
        resp.on_hover_text(value.to_string())
    } else {
        resp
    };
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Sunken typing line. Same rim as a menu row, pushed in.
pub fn field(ui: &mut Ui, text: &mut String, width: f32, hint: &str, font: FontId) -> Response {
    let width = width.max(120.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 28.0), Sense::hover());
    paint(ui.painter(), rect, FIELD, true, ROW_ROUND);
    // Same 3px lift as the buttons, so the typed line sits on the middle.
    let text_h = (font.size + 4.0).min(rect.height() - 6.0);
    let inner = Rect::from_center_size(
        rect.center() + Vec2::new(0.0, 1.0 - 3.0),
        Vec2::new((rect.width() - 16.0).max(40.0), text_h),
    );
    let mut response = None;
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(inner), |ui| {
        let mut edit = egui::TextEdit::singleline(text)
            .desired_width(inner.width().max(40.0))
            .font(font)
            .frame(false)
            .text_color(Color32::BLACK);
        if !hint.is_empty() {
            edit = edit.hint_text(hint);
        }
        response = Some(ui.add(edit));
    });
    response.expect("text field")
}

/// Bottom button every form uses. `hot` is the keyboard row sitting on it.
pub fn apply(ui: &mut Ui, hot: bool, font: FontId) -> bool {
    ui.add_space(8.0);
    let w = button_width(ui);
    ui.vertical_centered(|ui| {
        let (rule, _) = ui.allocate_exact_size(Vec2::new(w, 8.0), Sense::hover());
        let y = rule.center().y;
        ui.painter()
            .hline(rule.x_range(), y, Stroke::new(1.0, Color32::BLACK));
        ui.painter()
            .hline(rule.x_range(), y + 1.0, Stroke::new(1.0, Color32::WHITE));
        ui.add_space(6.0);
        push_button(ui, "Apply", w, 30.0, hot, font)
    })
    .inner
}

/// Raised button of a chosen size. Save, Up, and Down in the icon editor.
pub fn button(ui: &mut Ui, label: &str, width: f32, height: f32, font: FontId) -> bool {
    push_button(ui, label, width, height, false, font)
}

/// A choice in a popup that is not a form (Yes / No, replace / append).
pub fn choice(ui: &mut Ui, label: &str, font: FontId) -> bool {
    let w = button_width(ui);
    ui.vertical_centered(|ui| push_button(ui, label, w, 28.0, false, font))
        .inner
}

fn push_button(ui: &mut Ui, label: &str, width: f32, height: f32, hot: bool, font: FontId) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    let down = hot || resp.is_pointer_button_down_on();
    let fill = if hot {
        SELECT
    } else if resp.hovered() {
        FACE_HOVER
    } else {
        FACE
    };
    paint(ui.painter(), rect, fill, down, TOP_ROUND);
    let fg = if hot { Color32::WHITE } else { Color32::BLACK };
    // Centre of the face, so the words sit in the middle of a short button.
    let at = rect.center()
        + if down {
            Vec2::new(1.0, 1.0)
        } else {
            Vec2::ZERO
        }
        - Vec2::new(0.0, 3.0);
    ui.painter()
        .text(at, egui::Align2::CENTER_CENTER, label, font, fg);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

/// White along the top and left, black along the bottom and right.
/// Drawn on the menu window itself, not only on its buttons.
pub fn border(painter: &egui::Painter, rect: Rect) {
    let e = 2.0;
    if rect.width() < e * 2.0 || rect.height() < e * 2.0 {
        return;
    }
    painter.rect_filled(
        Rect::from_min_max(rect.min, egui::pos2(rect.right(), rect.top() + e)),
        0.0,
        Color32::WHITE,
    );
    painter.rect_filled(
        Rect::from_min_max(
            egui::pos2(rect.left(), rect.top() + e),
            egui::pos2(rect.left() + e, rect.bottom() - e),
        ),
        0.0,
        Color32::WHITE,
    );
    painter.rect_filled(
        Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - e), rect.max),
        0.0,
        Color32::BLACK,
    );
    painter.rect_filled(
        Rect::from_min_max(
            egui::pos2(rect.right() - e, rect.top() + e),
            egui::pos2(rect.right(), rect.bottom() - e),
        ),
        0.0,
        Color32::BLACK,
    );
}

fn button_width(ui: &Ui) -> f32 {
    let avail = ui.available_width();
    if avail.is_finite() {
        avail.clamp(200.0, ROW_W)
    } else {
        280.0
    }
}

fn text_width(ui: &Ui, text: &str, font: &FontId) -> f32 {
    ui.fonts(|f| {
        f.layout_no_wrap(text.to_string(), font.clone(), Color32::BLACK)
            .size()
            .x
    })
}

fn fit_text(ui: &Ui, text: &str, font: &FontId, max_w: f32) -> String {
    if text.is_empty() || max_w <= 8.0 {
        return String::new();
    }
    if text_width(ui, text, font) <= max_w {
        return text.to_string();
    }
    let mut kept: String = text.to_string();
    while !kept.is_empty() {
        kept.pop();
        let shown = format!("{kept}...");
        if text_width(ui, &shown, font) <= max_w {
            return shown;
        }
    }
    "...".to_string()
}
