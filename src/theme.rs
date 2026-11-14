use std::cell::Cell;

use eframe::egui::{self, Color32, CornerRadius, Margin, Stroke, Vec2};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum AppTheme {
    #[default]
    Obsidian,
    Graphite,
    Plum,
    Ember,
}

impl AppTheme {
    pub(crate) const ALL: [Self; 4] = [Self::Obsidian, Self::Graphite, Self::Plum, Self::Ember];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Obsidian => "Obsidian",
            Self::Graphite => "Graphite",
            Self::Plum => "Plum",
            Self::Ember => "Ember",
        }
    }

    pub(crate) fn palette(self) -> ThemePalette {
        match self {
            Self::Obsidian => ThemePalette {
                bg: Color32::from_rgb(17, 18, 20),
                sidebar: Color32::from_rgb(13, 14, 16),
                surface: Color32::from_rgb(25, 27, 30),
                surface_2: Color32::from_rgb(32, 34, 38),
                hover: Color32::from_rgb(40, 42, 46),
                edge: Color32::from_rgb(49, 51, 55),
                text: Color32::from_rgb(239, 239, 236),
                soft: Color32::from_rgb(174, 175, 173),
                quiet: Color32::from_rgb(121, 123, 123),
            },
            Self::Graphite => ThemePalette {
                bg: Color32::from_rgb(20, 23, 27),
                sidebar: Color32::from_rgb(16, 19, 23),
                surface: Color32::from_rgb(28, 32, 37),
                surface_2: Color32::from_rgb(38, 43, 49),
                hover: Color32::from_rgb(49, 56, 64),
                edge: Color32::from_rgb(62, 71, 81),
                text: Color32::from_rgb(238, 242, 245),
                soft: Color32::from_rgb(169, 179, 188),
                quiet: Color32::from_rgb(112, 124, 136),
            },
            Self::Plum => ThemePalette {
                bg: Color32::from_rgb(24, 18, 25),
                sidebar: Color32::from_rgb(18, 13, 20),
                surface: Color32::from_rgb(32, 24, 34),
                surface_2: Color32::from_rgb(42, 31, 45),
                hover: Color32::from_rgb(54, 40, 57),
                edge: Color32::from_rgb(70, 51, 73),
                text: Color32::from_rgb(243, 235, 242),
                soft: Color32::from_rgb(190, 174, 190),
                quiet: Color32::from_rgb(138, 116, 138),
            },
            Self::Ember => ThemePalette {
                bg: Color32::from_rgb(25, 20, 18),
                sidebar: Color32::from_rgb(19, 15, 14),
                surface: Color32::from_rgb(34, 27, 24),
                surface_2: Color32::from_rgb(44, 34, 30),
                hover: Color32::from_rgb(56, 42, 37),
                edge: Color32::from_rgb(74, 54, 47),
                text: Color32::from_rgb(245, 236, 228),
                soft: Color32::from_rgb(196, 178, 168),
                quiet: Color32::from_rgb(145, 124, 113),
            },
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ThemePalette {
    pub(crate) bg: Color32,
    pub(crate) sidebar: Color32,
    pub(crate) surface: Color32,
    pub(crate) surface_2: Color32,
    pub(crate) hover: Color32,
    pub(crate) edge: Color32,
    pub(crate) text: Color32,
    pub(crate) soft: Color32,
    pub(crate) quiet: Color32,
}

thread_local! {
    static ACTIVE_THEME: Cell<AppTheme> = const { Cell::new(AppTheme::Obsidian) };
}

fn active_theme() -> AppTheme {
    ACTIVE_THEME.with(Cell::get)
}

pub(crate) fn theme_bg() -> Color32 {
    active_theme().palette().bg
}

pub(crate) fn theme_sidebar() -> Color32 {
    active_theme().palette().sidebar
}

pub(crate) fn theme_surface() -> Color32 {
    active_theme().palette().surface
}

pub(crate) fn theme_surface_2() -> Color32 {
    active_theme().palette().surface_2
}

pub(crate) fn theme_hover() -> Color32 {
    active_theme().palette().hover
}

pub(crate) fn theme_edge() -> Color32 {
    active_theme().palette().edge
}

pub(crate) fn theme_text() -> Color32 {
    active_theme().palette().text
}

pub(crate) fn theme_soft() -> Color32 {
    active_theme().palette().soft
}

pub(crate) fn theme_quiet() -> Color32 {
    active_theme().palette().quiet
}

pub(crate) fn theme_red() -> Color32 {
    Color32::from_rgb(243, 83, 85)
}

pub(crate) fn set_theme(ctx: &egui::Context, theme: AppTheme) {
    ACTIVE_THEME.with(|active| active.set(theme));
    let palette = theme.palette();
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = palette.bg;
    visuals.window_fill = palette.surface;
    visuals.faint_bg_color = palette.surface_2;
    visuals.extreme_bg_color = palette.sidebar;
    visuals.override_text_color = Some(palette.text);
    visuals.weak_text_color = Some(palette.soft);
    visuals.hyperlink_color = theme_red();
    visuals.selection.bg_fill = Color32::from_rgb(76, 37, 40);
    visuals.window_corner_radius = CornerRadius::same(10);
    visuals.window_shadow = egui::epaint::Shadow {
        offset: [0, 8],
        blur: 28,
        spread: 0,
        color: Color32::from_black_alpha(130),
    };
    visuals.popup_shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 20,
        spread: 0,
        color: Color32::from_black_alpha(120),
    };
    visuals.widgets.inactive.bg_fill = palette.surface_2;
    visuals.widgets.inactive.weak_bg_fill = palette.surface;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, palette.edge);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(5);
    visuals.widgets.hovered.bg_fill = palette.hover;
    visuals.widgets.hovered.weak_bg_fill = palette.hover;
    visuals.widgets.hovered.corner_radius = CornerRadius::same(5);
    visuals.widgets.active.bg_fill = palette.hover;
    visuals.widgets.active.corner_radius = CornerRadius::same(5);
    visuals.widgets.open.bg_fill = palette.surface_2;
    visuals.widgets.noninteractive.bg_fill = palette.bg;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, palette.soft);
    ctx.set_visuals_of(egui::Theme::Dark, visuals);
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.spacing.item_spacing = Vec2::new(9.0, 8.0);
    style.spacing.button_padding = Vec2::new(10.0, 7.0);
    style.spacing.interact_size = Vec2::new(36.0, 32.0);
    style.spacing.slider_width = 160.0;
    style.spacing.window_margin = Margin::same(16);
    // This completely kills the vertical scrollbar you are seeing
    style.spacing.scroll = egui::style::ScrollStyle {
        bar_width: 0.0,
        ..Default::default()
    };
    ctx.set_style_of(egui::Theme::Dark, style);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_names_are_stable_for_saved_settings() {
        assert_eq!(AppTheme::Obsidian.label(), "Obsidian");
        assert_eq!(AppTheme::Graphite.label(), "Graphite");
        assert_eq!(AppTheme::Plum.label(), "Plum");
        assert_eq!(AppTheme::Ember.label(), "Ember");
    }
}
