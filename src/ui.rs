use eframe::egui::{self, Align, Color32, CornerRadius, FontId, Layout, RichText, Stroke, Vec2};

use crate::{
    model::VolumeAction,
    theme::{
        AppTheme, theme_bg, theme_edge, theme_hover, theme_quiet, theme_red, theme_soft,
        theme_surface, theme_surface_2, theme_text,
    },
};

#[derive(Clone, Copy)]
pub(crate) enum Icon {
    Home,
    Mix,
    Library,
    Liked,
    Playlist,
    Settings,
    Shuffle,
    Previous,
    Play,
    Pause,
    Next,
    Repeat,
    RepeatOne,
    Queue,
    NowPlaying,
    Export,
    Volume,
    VolumeMute,
    Close,
    Add,
    SortUp,
    SortDown,
    Remove,
    MoveUp,
    MoveDown,
    Search,
    User,
    Headphones,
    Waveform,
    Star,
    Moon,
    Flame,
}

/// Returns a Unicode character for the given icon (for use in text buttons)
pub(crate) fn icon_char(icon: Icon) -> &'static str {
    match icon {
        Icon::Home => "🏠",
        Icon::Mix => "🎚️",
        Icon::Library => "📚",
        Icon::Liked => "♥",
        Icon::Playlist => "📋",
        Icon::Settings => "⚙️",
        Icon::Shuffle => "🔀",
        Icon::Previous => "⏮",
        Icon::Play => "▶",
        Icon::Pause => "⏸",
        Icon::Next => "⏭",
        Icon::Repeat => "🔁",
        Icon::RepeatOne => "🔂",
        Icon::Queue => "📜",
        Icon::NowPlaying => "🎵",
        Icon::Export => "📤",
        Icon::Volume => "🔊",
        Icon::VolumeMute => "🔇",
        Icon::Close => "✕",
        Icon::Add => "+",
        Icon::SortUp => "↑",
        Icon::SortDown => "↓",
        Icon::Remove => "🗑️",
        Icon::MoveUp => "↑",
        Icon::MoveDown => "↓",
        Icon::Search => "🔍",
        Icon::User => "●",
        Icon::Headphones => "♫",
        Icon::Waveform => "≋",
        Icon::Star => "★",
        Icon::Moon => "●",
        Icon::Flame => "●",
    }
}

pub(crate) fn theme_card(ui: &mut egui::Ui, theme: AppTheme, selected: bool) -> bool {
    let palette = theme.palette();
    let height = 62.0;
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), height),
        egui::Sense::click(),
    );
    if ui.is_rect_visible(rect) {
        let painter = ui.painter().with_clip_rect(rect);
        let fill = if response.hovered() {
            color_lerp(palette.surface, palette.hover, 0.45)
        } else {
            palette.surface
        };
        let border = if selected {
            Stroke::new(1.5, theme_red())
        } else {
            Stroke::new(1.0, palette.edge)
        };
        painter.rect_filled(rect, CornerRadius::same(8), fill);
        painter.rect_stroke(
            rect,
            CornerRadius::same(8),
            border,
            egui::StrokeKind::Inside,
        );

        painter.text(
            rect.left_top() + Vec2::new(14.0, 12.0),
            egui::Align2::LEFT_TOP,
            theme.label(),
            FontId::proportional(15.0),
            palette.text,
        );

        let swatch_y = rect.bottom() - 14.0;
        let swatch_width = 22.0;
        let swatch_gap = 6.0;
        for (index, color) in [palette.bg, palette.surface_2, palette.hover]
            .into_iter()
            .enumerate()
        {
            let swatch = egui::Rect::from_min_size(
                egui::pos2(
                    rect.left() + 14.0 + (index as f32) * (swatch_width + swatch_gap),
                    swatch_y,
                ),
                Vec2::new(swatch_width, 8.0),
            );
            painter.rect_filled(swatch, CornerRadius::same(3), color);
            painter.rect_stroke(
                swatch,
                CornerRadius::same(3),
                Stroke::new(1.0, palette.edge),
                egui::StrokeKind::Inside,
            );
        }

        let accent_preview = egui::Rect::from_min_size(
            rect.right_top() + Vec2::new(-40.0, 11.0),
            Vec2::new(26.0, 16.0),
        );
        painter.rect_stroke(
            accent_preview,
            CornerRadius::same(4),
            Stroke::new(1.5, theme_red()),
            egui::StrokeKind::Inside,
        );
        if selected {
            painter.text(
                rect.right_top() + Vec2::new(-13.0, 33.0),
                egui::Align2::RIGHT_TOP,
                "ACTIVE",
                FontId::proportional(10.0),
                theme_red(),
            );
        }
    }
    response.clicked()
}

pub(crate) fn avatar_icon(icon_key: &str) -> Icon {
    match icon_key {
        "headphones" => Icon::Headphones,
        "waveform" => Icon::Waveform,
        "star" => Icon::Star,
        "moon" => Icon::Moon,
        "flame" => Icon::Flame,
        _ => Icon::User,
    }
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn profile_avatar_button(
    ui: &mut egui::Ui,
    icon_key: &str,
    tooltip: &str,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(36.0), egui::Sense::click());
    let hovered = ui
        .ctx()
        .animate_bool_responsive(response.id.with("hover"), response.hovered());
    let fill = color_lerp(theme_surface_2(), theme_hover(), hovered);
    ui.painter().circle_filled(rect.center(), 17.0, fill);
    ui.painter().circle_stroke(
        rect.center(),
        17.0,
        Stroke::new(1.0, color_lerp(theme_edge(), theme_red(), hovered * 0.8)),
    );
    paint_icon(
        ui.painter(),
        egui::Rect::from_center_size(rect.center(), Vec2::splat(20.0)),
        avatar_icon(icon_key),
        color_lerp(theme_soft(), theme_text(), hovered),
    );
    response.on_hover_text(tooltip)
}

pub(crate) fn profile_avatar(
    ui: &mut egui::Ui,
    icon_key: &str,
    size: f32,
    selected: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::click());
    let fill = if selected {
        Color32::from_rgb(72, 43, 46)
    } else {
        theme_surface_2()
    };
    ui.painter().circle_filled(rect.center(), size * 0.5, fill);
    ui.painter().circle_stroke(
        rect.center(),
        size * 0.5 - 1.0,
        Stroke::new(
            if selected { 1.6 } else { 1.0 },
            if selected { theme_red() } else { theme_edge() },
        ),
    );
    paint_icon(
        ui.painter(),
        egui::Rect::from_center_size(rect.center(), Vec2::splat(size * 0.55)),
        avatar_icon(icon_key),
        if selected { theme_red() } else { theme_soft() },
    );
    response
}

pub(crate) fn nav_row(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    selected: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 40.0), egui::Sense::click());
    let ctx = ui.ctx();
    let hovered = ctx.animate_bool_responsive(response.id.with("hover"), response.hovered());
    let selected_t = ctx.animate_bool_responsive(response.id.with("selected"), selected);
    let base = color_lerp(Color32::TRANSPARENT, theme_surface_2(), hovered * 0.85);
    let fill = color_lerp(base, Color32::from_rgb(47, 34, 36), selected_t);
    ui.painter().rect_filled(rect, CornerRadius::same(7), fill);
    let accent = (selected_t * 255.0) as u8;
    ui.painter().rect_filled(
        egui::Rect::from_min_size(
            rect.left_top() + Vec2::new(1.0, 10.0),
            Vec2::new(2.0, rect.height() - 20.0),
        ),
        CornerRadius::same(1),
        Color32::from_rgba_unmultiplied(theme_red().r(), theme_red().g(), theme_red().b(), accent),
    );
    paint_icon(
        ui.painter(),
        egui::Rect::from_center_size(
            egui::pos2(rect.left() + 23.0, rect.center().y),
            Vec2::splat(22.0),
        ),
        icon,
        color_lerp(theme_quiet(), theme_red(), selected_t),
    );
    ui.painter().text(
        egui::pos2(rect.left() + 43.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.5),
        color_lerp(theme_soft(), theme_text(), selected_t),
    );
    response
}

pub(crate) fn icon_button(ui: &mut egui::Ui, icon: Icon, tooltip: &str) -> egui::Response {
    icon_button_with_state(ui, icon, false, tooltip)
}

#[derive(Clone, Copy)]
enum ButtonStyle {
    Primary,
    Secondary,
    Danger,
    Ghost,
}

pub(crate) fn primary_button(ui: &mut egui::Ui, label: impl Into<String>) -> egui::Response {
    text_button(ui, label.into(), ButtonStyle::Primary, false)
}

pub(crate) fn secondary_button(ui: &mut egui::Ui, label: impl Into<String>) -> egui::Response {
    text_button(ui, label.into(), ButtonStyle::Secondary, false)
}

pub(crate) fn danger_button(ui: &mut egui::Ui, label: impl Into<String>) -> egui::Response {
    text_button(ui, label.into(), ButtonStyle::Danger, false)
}

pub(crate) fn ghost_button(ui: &mut egui::Ui, label: impl Into<String>) -> egui::Response {
    text_button(ui, label.into(), ButtonStyle::Ghost, false)
}

pub(crate) fn small_secondary_button(
    ui: &mut egui::Ui,
    label: impl Into<String>,
) -> egui::Response {
    text_button(ui, label.into(), ButtonStyle::Secondary, true)
}

pub(crate) fn menu_button<'a, R>(
    ui: &mut egui::Ui,
    atoms: impl egui::IntoAtoms<'a>,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    let button = egui::Button::new(atoms)
        .fill(theme_surface_2())
        .stroke(Stroke::new(1.0, theme_edge()))
        .corner_radius(CornerRadius::same(6));
    let (response, inner) = if egui::containers::menu::is_in_menu(ui) {
        egui::containers::menu::SubMenuButton::from_button(
            button.right_text(egui::containers::menu::SubMenuButton::arrow_atom(None)),
        )
        .ui(ui, add_contents)
    } else {
        egui::containers::menu::MenuButton::from_button(button).ui(ui, add_contents)
    };
    egui::InnerResponse::new(inner.map(|inner| inner.inner), response)
}

fn text_button(
    ui: &mut egui::Ui,
    label: String,
    style: ButtonStyle,
    compact: bool,
) -> egui::Response {
    let font_size = if compact { 11.5 } else { 13.0 };
    let height = if compact { 27.0 } else { 32.0 };
    let padding_x = if compact { 10.0 } else { 13.0 };
    let text_color = match style {
        ButtonStyle::Primary => theme_text(),
        ButtonStyle::Secondary => theme_text(),
        ButtonStyle::Danger => theme_red(),
        ButtonStyle::Ghost => theme_soft(),
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label, FontId::proportional(font_size), text_color);
    let desired_size = Vec2::new(galley.size().x + padding_x * 2.0, height);
    let enabled = ui.is_enabled();
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(desired_size, sense);
    let ctx = ui.ctx();
    let hovered = if enabled {
        ctx.animate_bool_responsive(response.id.with("hover"), response.hovered())
    } else {
        0.0
    };
    let pressed = ctx.animate_bool_responsive(
        response.id.with("pressed"),
        response.is_pointer_button_down_on(),
    );
    let accent = theme_red();
    let (fill, border, label_color) = match style {
        ButtonStyle::Primary => (
            Color32::from_rgba_unmultiplied(
                accent.r(),
                accent.g(),
                accent.b(),
                ((hovered * 0.11 + pressed * 0.07) * 255.0) as u8,
            ),
            Color32::from_rgba_unmultiplied(
                accent.r(),
                accent.g(),
                accent.b(),
                ((0.52 + hovered * 0.48) * 255.0) as u8,
            ),
            color_lerp(theme_text(), accent, hovered * 0.35),
        ),
        ButtonStyle::Secondary => (
            color_lerp(theme_surface_2(), theme_hover(), hovered * 0.8),
            color_lerp(theme_edge(), theme_soft(), hovered * 0.75),
            theme_text(),
        ),
        ButtonStyle::Danger => (
            Color32::from_rgba_unmultiplied(
                accent.r(),
                accent.g(),
                accent.b(),
                ((hovered * 0.13 + pressed * 0.07) * 255.0) as u8,
            ),
            Color32::from_rgba_unmultiplied(
                accent.r(),
                accent.g(),
                accent.b(),
                ((0.48 + hovered * 0.52) * 255.0) as u8,
            ),
            accent,
        ),
        ButtonStyle::Ghost => (
            color_lerp(Color32::TRANSPARENT, theme_hover(), hovered * 0.75),
            Color32::TRANSPARENT,
            color_lerp(theme_soft(), theme_text(), hovered),
        ),
    };
    let (fill, border, label_color) = if enabled {
        (fill, border, label_color)
    } else {
        (
            ui.visuals().disable(fill),
            ui.visuals().disable(border),
            ui.visuals().disable(label_color),
        )
    };
    ui.painter().rect_filled(rect, CornerRadius::same(7), fill);
    if border != Color32::TRANSPARENT {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(7),
            Stroke::new(1.0 + hovered * 0.25, border),
            egui::StrokeKind::Inside,
        );
    }
    let text_position = egui::pos2(
        rect.left() + padding_x,
        rect.center().y - galley.size().y * 0.5 + pressed * 0.45,
    );
    ui.painter().galley(text_position, galley, label_color);
    response
}

pub(crate) fn volume_control(
    ui: &mut egui::Ui,
    volume: &mut f32,
    muted: bool,
) -> Option<VolumeAction> {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(96.0, 34.0), egui::Sense::hover());
    let ctx = ui.ctx();
    let mute_rect =
        egui::Rect::from_min_size(rect.left_top() + Vec2::new(5.0, 5.0), Vec2::splat(24.0));
    let slider_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 34.0, rect.top() + 7.0),
        egui::pos2(rect.right() - 6.0, rect.bottom() - 7.0),
    );
    let mute_response = ui.interact(mute_rect, ui.scope_id().with("mute"), egui::Sense::click());
    let slider_response = ui.interact(
        slider_rect,
        ui.scope_id().with("slider"),
        egui::Sense::click_and_drag(),
    );
    let mute_hover =
        ctx.animate_bool_responsive(mute_response.id.with("hover"), mute_response.hovered());
    let mute_pressed = ctx.animate_bool_responsive(
        mute_response.id.with("pressed"),
        mute_response.is_pointer_button_down_on(),
    );
    let slider_hover =
        ctx.animate_bool_responsive(slider_response.id.with("hover"), slider_response.hovered());
    let slider_pressed = ctx.animate_bool_responsive(
        slider_response.id.with("pressed"),
        slider_response.is_pointer_button_down_on(),
    );
    let control_hover = mute_hover.max(slider_hover);
    let border = color_lerp(theme_edge(), theme_soft(), control_hover * 0.7);
    ui.painter()
        .rect_filled(rect, CornerRadius::same(9), theme_surface());
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(9),
        Stroke::new(1.0, border),
        egui::StrokeKind::Inside,
    );

    let mute_color = if muted {
        theme_red()
    } else {
        color_lerp(theme_quiet(), theme_red(), mute_hover)
    };
    let mute_icon_rect = mute_rect.shrink(3.0 + mute_pressed * 0.8);
    paint_icon(
        ui.painter(),
        mute_icon_rect,
        if muted {
            Icon::VolumeMute
        } else {
            Icon::Volume
        },
        mute_color,
    );

    let level = volume.clamp(0.0, 1.0);
    let animated_level = ctx.animate_value_with_time(slider_response.id.with("level"), level, 0.12);
    let knob_radius = 5.0 + slider_hover * 0.8 - slider_pressed * 0.5;
    let usable_width = (slider_rect.width() - knob_radius * 2.0).max(1.0);
    let rail_rect =
        egui::Rect::from_center_size(slider_rect.center(), Vec2::new(slider_rect.width(), 5.0));
    ui.painter()
        .rect_filled(rail_rect, CornerRadius::same(3), theme_edge());
    let knob_center = egui::pos2(
        rail_rect.left() + knob_radius + usable_width * animated_level,
        rail_rect.center().y,
    );
    let fill_rect = egui::Rect::from_min_max(
        rail_rect.left_top(),
        egui::pos2(knob_center.x, rail_rect.bottom()),
    );
    ui.painter()
        .rect_filled(fill_rect, CornerRadius::same(3), theme_red());
    if slider_hover > 0.01 {
        ui.painter().circle_filled(
            knob_center,
            knob_radius + 3.0 * slider_hover,
            Color32::from_rgba_unmultiplied(
                theme_red().r(),
                theme_red().g(),
                theme_red().b(),
                (42.0 * slider_hover) as u8,
            ),
        );
    }
    ui.painter().circle_filled(
        knob_center,
        knob_radius,
        if level > 0.001 {
            theme_red()
        } else {
            theme_quiet()
        },
    );

    if let Some(pointer) = slider_response.interact_pointer_pos() {
        let value = ((pointer.x - rail_rect.left() - knob_radius) / usable_width).clamp(0.0, 1.0);
        *volume = value;
    }

    mute_response
        .clone()
        .on_hover_text(if muted { "Unmute" } else { "Mute" });
    slider_response
        .clone()
        .on_hover_text(format!("Volume: {}%", (level * 100.0).round() as u8));

    if mute_response.clicked() {
        Some(VolumeAction::ToggleMute)
    } else if slider_response.interact_pointer_pos().is_some() {
        Some(VolumeAction::Set(*volume))
    } else {
        None
    }
}

pub(crate) fn export_button(ui: &mut egui::Ui, tooltip: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(34.0), egui::Sense::click());
    let ctx = ui.ctx();
    let appeared = ctx.animate_bool_responsive(response.id.with("appeared"), true);
    let hovered = ctx.animate_bool_responsive(response.id.with("hover"), response.hovered());
    let pressed = ctx.animate_bool_responsive(
        response.id.with("pressed"),
        response.is_pointer_button_down_on(),
    );
    let accent = theme_red();
    let fill_alpha = (appeared * (0.035 + hovered * 0.11)).min(1.0);
    let border_alpha = (appeared * (0.38 + hovered * 0.62)).min(1.0);
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(8),
        Color32::from_rgba_unmultiplied(
            accent.r(),
            accent.g(),
            accent.b(),
            (fill_alpha * 255.0) as u8,
        ),
    );
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(8),
        Stroke::new(
            1.0 + hovered * 0.35,
            Color32::from_rgba_unmultiplied(
                accent.r(),
                accent.g(),
                accent.b(),
                (border_alpha * 255.0) as u8,
            ),
        ),
        egui::StrokeKind::Inside,
    );
    let icon_color = color_lerp(
        theme_quiet(),
        accent,
        (appeared * (0.45 + hovered * 0.55)).min(1.0),
    );
    let icon_rect = rect.shrink(8.0 + pressed * 1.4 - hovered * 0.5);
    paint_icon(
        ui.painter(),
        icon_rect.translate(Vec2::new(0.0, -hovered * 0.7)),
        Icon::Export,
        icon_color,
    );
    response.on_hover_text(tooltip)
}

pub(crate) fn toggle_button(
    ui: &mut egui::Ui,
    icon: Icon,
    active: bool,
    tooltip: &str,
) -> egui::Response {
    icon_button_with_state(ui, icon, active, tooltip)
}

pub(crate) fn icon_button_with_state(
    ui: &mut egui::Ui,
    icon: Icon,
    active: bool,
    tooltip: &str,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(34.0), egui::Sense::click());
    let ctx = ui.ctx();
    let hovered = ctx.animate_bool_responsive(response.id.with("hover"), response.hovered());
    let selected = ctx.animate_bool_responsive(response.id.with("active"), active);
    let pressed = ctx.animate_bool_responsive(
        response.id.with("pressed"),
        response.is_pointer_button_down_on(),
    );
    let background = color_lerp(
        Color32::TRANSPARENT,
        theme_hover(),
        (hovered * 0.9 + pressed * 0.1).min(1.0),
    );
    if selected > 0.01 {
        let accent = theme_red();
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(8),
            Color32::from_rgba_unmultiplied(
                accent.r(),
                accent.g(),
                accent.b(),
                (selected * 36.0) as u8,
            ),
        );
    }
    ui.painter()
        .rect_filled(rect, CornerRadius::same(8), background);
    let border_strength = selected.max(hovered * 0.7);
    if border_strength > 0.01 {
        let border = color_lerp(theme_edge(), theme_red(), selected);
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(8),
            Stroke::new(
                1.0,
                Color32::from_rgba_unmultiplied(
                    border.r(),
                    border.g(),
                    border.b(),
                    (border_strength * 150.0) as u8,
                ),
            ),
            egui::StrokeKind::Inside,
        );
    }
    let color = color_lerp(
        color_lerp(theme_quiet(), theme_text(), hovered * 0.85),
        theme_red(),
        selected,
    );
    paint_icon(
        ui.painter(),
        rect.shrink(8.0 + pressed * 1.2 - hovered * 0.4),
        icon,
        color,
    );
    response.on_hover_text(tooltip)
}

pub(crate) fn transport_button(ui: &mut egui::Ui, icon: Icon) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(44.0, 40.0), egui::Sense::click());
    let hovered = ui
        .ctx()
        .animate_bool_responsive(response.id.with("hover"), response.hovered());
    let pressed = ui.ctx().animate_bool_responsive(
        response.id.with("pressed"),
        response.is_pointer_button_down_on(),
    );
    let radius = 20.0 + hovered * 0.8 - pressed * 1.2;
    if hovered > 0.01 {
        ui.painter().circle_filled(
            rect.center(),
            radius + 4.0 * hovered,
            Color32::from_rgba_unmultiplied(
                theme_red().r(),
                theme_red().g(),
                theme_red().b(),
                (38.0 * hovered) as u8,
            ),
        );
    }
    let button_color = color_lerp(theme_text(), theme_red(), hovered * 0.75);
    ui.painter()
        .circle_filled(rect.center(), radius, button_color);
    paint_icon(ui.painter(), rect.shrink(12.0), icon, theme_bg());
    response.on_hover_text(if matches!(icon, Icon::Play) {
        "Play"
    } else {
        "Pause"
    })
}

pub(crate) fn inline_icon(ui: &mut egui::Ui, icon: Icon) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(24.0), egui::Sense::hover());
    paint_icon(ui.painter(), rect.shrink(4.0), icon, theme_quiet());
}

pub(crate) fn paint_icon(painter: &egui::Painter, rect: egui::Rect, icon: Icon, color: Color32) {
    use egui::{Shape, pos2};

    let scale = (rect.width().min(rect.height()) / 20.0).clamp(0.7, 1.15);
    let c = rect.center();
    let point = |x: f32, y: f32| pos2(c.x + x * scale, c.y + y * scale);
    let stroke = Stroke::new(1.65 * scale, color);
    let line = |a: (f32, f32), b: (f32, f32)| {
        painter.line_segment([point(a.0, a.1), point(b.0, b.1)], stroke);
    };
    let path = |points: &[(f32, f32)]| {
        painter.add(Shape::line(
            points.iter().map(|&(x, y)| point(x, y)).collect(),
            stroke,
        ));
    };
    let fill = |points: &[(f32, f32)]| {
        painter.add(Shape::convex_polygon(
            points.iter().map(|&(x, y)| point(x, y)).collect(),
            color,
            Stroke::NONE,
        ));
    };
    let dot = |x: f32, y: f32, radius: f32| {
        painter.circle_filled(point(x, y), radius * scale, color);
    };

    match icon {
        Icon::Home => {
            path(&[(-8.0, -1.0), (0.0, -8.0), (8.0, -1.0)]);
            line((-6.0, -2.5), (-6.0, 7.0));
            line((6.0, -2.5), (6.0, 7.0));
            line((-6.0, 7.0), (6.0, 7.0));
            line((-1.5, 7.0), (-1.5, 1.0));
            line((2.5, 7.0), (2.5, 3.0));
        }
        Icon::Mix | Icon::NowPlaying => {
            for (x, h) in [
                (-7.0, 7.0),
                (-3.0, 13.0),
                (1.0, 9.0),
                (5.0, 15.0),
                (9.0, 6.0),
            ] {
                line((x, h * 0.5), (x, -h * 0.5));
            }
        }
        Icon::Library => {
            line((-7.0, -8.0), (-7.0, 8.0));
            for y in [-6.0, -2.0, 2.0, 6.0] {
                line((-3.0, y), (8.0, y));
            }
        }
        Icon::Liked => {
            painter.add(Shape::closed_line(
                [
                    (-9.0, -2.0),
                    (-9.0, -4.0),
                    (-7.0, -7.0),
                    (-4.0, -7.0),
                    (0.0, -3.0),
                    (4.0, -7.0),
                    (7.0, -7.0),
                    (9.0, -4.0),
                    (9.0, -1.0),
                    (6.0, 3.0),
                    (0.0, 8.0),
                    (-6.0, 3.0),
                    (-9.0, -1.0),
                ]
                .iter()
                .map(|&(x, y)| point(x, y))
                .collect(),
                stroke,
            ));
        }
        Icon::Playlist | Icon::Queue => {
            for y in [-6.0, 0.0, 6.0] {
                dot(-7.0, y, 1.0);
                line((-3.0, y), (8.0, y));
            }
        }
        Icon::Settings => {
            for (y, x) in [(-6.0, 3.0), (0.0, -3.0), (6.0, 1.0)] {
                line((-8.0, y), (8.0, y));
                painter.circle_filled(point(x, y), 2.1 * scale, theme_bg());
                painter.circle_stroke(point(x, y), 2.1 * scale, stroke);
            }
        }
        Icon::Shuffle => {
            path(&[(-8.0, -6.0), (-5.0, -6.0), (5.0, 6.0), (8.0, 6.0)]);
            path(&[(-8.0, 6.0), (-5.0, 6.0), (5.0, -6.0), (8.0, -6.0)]);
            path(&[(4.0, -9.0), (8.0, -6.0), (4.0, -3.0)]);
            path(&[(4.0, 3.0), (8.0, 6.0), (4.0, 9.0)]);
        }
        Icon::Previous | Icon::Next => {
            let direction = if matches!(icon, Icon::Previous) {
                -1.0
            } else {
                1.0
            };
            line((direction * 7.0, -7.0), (direction * 7.0, 7.0));
            fill(&[
                (direction * 3.0, -7.0),
                (direction * 3.0, 7.0),
                (direction * -6.0, 0.0),
            ]);
        }
        Icon::Play => fill(&[(-5.5, -8.0), (-5.5, 8.0), (7.0, 0.0)]),
        Icon::Pause => {
            painter.rect_filled(
                egui::Rect::from_center_size(point(-3.5, 0.0), Vec2::new(3.3, 13.0) * scale),
                2.0 * scale,
                color,
            );
            painter.rect_filled(
                egui::Rect::from_center_size(point(3.5, 0.0), Vec2::new(3.3, 13.0) * scale),
                2.0 * scale,
                color,
            );
        }
        Icon::Repeat | Icon::RepeatOne => {
            path(&[(-7.0, -3.0), (-4.0, -6.0), (6.0, -6.0), (8.0, -4.0)]);
            path(&[(5.0, -7.0), (8.0, -4.0), (5.0, -1.0)]);
            path(&[(7.0, 3.0), (4.0, 6.0), (-6.0, 6.0), (-8.0, 4.0)]);
            path(&[(-5.0, 7.0), (-8.0, 4.0), (-5.0, 1.0)]);
            if matches!(icon, Icon::RepeatOne) {
                painter.text(
                    point(0.0, 0.0),
                    egui::Align2::CENTER_CENTER,
                    "1",
                    FontId::proportional(8.0 * scale),
                    color,
                );
            }
        }
        Icon::Close | Icon::Remove => {
            line((-5.0, -5.0), (5.0, 5.0));
            line((-5.0, 5.0), (5.0, -5.0));
        }
        Icon::Add => {
            line((-6.0, 0.0), (6.0, 0.0));
            line((0.0, -6.0), (0.0, 6.0));
        }
        Icon::SortUp | Icon::MoveUp | Icon::SortDown | Icon::MoveDown => {
            let up = matches!(icon, Icon::SortUp | Icon::MoveUp);
            let d = if up { -1.0 } else { 1.0 };
            line((0.0, -7.0 * d), (0.0, 7.0 * d));
            path(&[(-4.0, -3.0 * d), (0.0, -7.0 * d), (4.0, -3.0 * d)]);
        }
        Icon::Search => {
            painter.circle_stroke(point(-2.0, -2.0), 5.5 * scale, stroke);
            line((2.0, 2.0), (7.0, 7.0));
        }
        Icon::User => {
            painter.circle_stroke(point(0.0, -4.0), 3.2 * scale, stroke);
            path(&[(-7.0, 8.0), (-5.0, 3.0), (5.0, 3.0), (7.0, 8.0)]);
        }
        Icon::Headphones => {
            path(&[
                (-8.0, 1.0),
                (-8.0, -2.0),
                (-5.0, -7.0),
                (0.0, -9.0),
                (5.0, -7.0),
                (8.0, -2.0),
                (8.0, 1.0),
            ]);
            painter.rect_filled(
                egui::Rect::from_center_size(point(-6.0, 3.0), Vec2::new(3.0, 6.0) * scale),
                1.5 * scale,
                color,
            );
            painter.rect_filled(
                egui::Rect::from_center_size(point(6.0, 3.0), Vec2::new(3.0, 6.0) * scale),
                1.5 * scale,
                color,
            );
        }
        Icon::Waveform => {
            for (x, height) in [
                (-7.0, 4.0),
                (-3.5, 8.0),
                (0.0, 12.0),
                (3.5, 7.0),
                (7.0, 3.0),
            ] {
                line((x, -height), (x, height));
            }
        }
        Icon::Star => {
            path(&[
                (0.0, -9.0),
                (2.5, -3.0),
                (9.0, -2.0),
                (4.0, 2.0),
                (6.0, 8.0),
                (0.0, 5.0),
                (-6.0, 8.0),
                (-4.0, 2.0),
                (-9.0, -2.0),
                (-2.5, -3.0),
            ]);
        }
        Icon::Moon => {
            painter.circle_stroke(point(1.0, 0.0), 7.0 * scale, stroke);
            fill(&[(6.0, -7.0), (1.0, -3.0), (6.0, 1.0), (8.0, -3.0)]);
        }
        Icon::Flame => {
            path(&[
                (0.0, -9.0),
                (5.0, -4.0),
                (7.0, 1.0),
                (4.0, 8.0),
                (-4.0, 8.0),
                (-7.0, 1.0),
                (-3.0, -3.0),
                (-1.0, 1.0),
            ]);
        }
        Icon::Export => {
            line((0.0, -8.0), (0.0, 3.0));
            path(&[(-4.0, -1.0), (0.0, 3.0), (4.0, -1.0)]);
            path(&[(-7.0, 3.0), (-5.0, 7.0), (5.0, 7.0), (7.0, 3.0)]);
        }
        Icon::Volume | Icon::VolumeMute => {
            fill(&[
                (-8.0, -3.0),
                (-4.0, -3.0),
                (0.0, -7.0),
                (0.0, 7.0),
                (-4.0, 3.0),
                (-8.0, 3.0),
            ]);
            path(&[(1.0, -4.0), (4.0, 0.0), (1.0, 4.0)]);
            path(&[(4.0, -6.0), (7.0, 0.0), (4.0, 6.0)]);
            if matches!(icon, Icon::VolumeMute) {
                line((4.0, -4.0), (8.0, 4.0));
                line((8.0, -4.0), (4.0, 4.0));
            }
        }
    }
}

pub(crate) fn color_lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
        channel(a.a(), b.a()),
    )
}

pub(crate) fn brand_mark(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(28.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(7), Color32::from_rgb(54, 34, 37));
    for (i, h) in [8.0, 13.0, 18.0, 10.0].into_iter().enumerate() {
        let x = rect.left() + 7.0 + i as f32 * 4.0;
        p.line_segment(
            [
                egui::pos2(x, rect.center().y - h * 0.5),
                egui::pos2(x, rect.center().y + h * 0.5),
            ],
            Stroke::new(2.0, theme_red()),
        );
    }
}

pub(crate) fn empty_library_mark(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(54.0), egui::Sense::hover());
    let p = ui.painter();
    p.circle_stroke(rect.center(), 24.0, Stroke::new(1.0, theme_edge()));
    paint_icon(p, rect.shrink(16.0), Icon::Library, theme_red());
}

pub(crate) fn detail_row(ui: &mut egui::Ui, title: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(11.0).color(theme_quiet()));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(value).size(11.0).color(theme_soft()));
        });
    });
}
