//! Native styling adapted from the user-supplied Keyloom component specimen.
use eframe::egui::{self, Color32, Stroke};

pub const APP: Color32 = Color32::from_gray(0x18);
pub const PANEL: Color32 = Color32::from_gray(0x20);
pub const RAISED: Color32 = Color32::from_gray(0x29);
pub const SUNKEN: Color32 = Color32::from_gray(0x14);
pub const DIVIDER: Color32 = Color32::from_gray(0x30);
pub const TEXT: Color32 = Color32::from_gray(0xe8);
pub const SECONDARY: Color32 = Color32::from_gray(0xb0);
pub const MUTED: Color32 = Color32::from_gray(0x96);
pub const DISABLED: Color32 = Color32::from_gray(0x7a);
pub const ACCENT: Color32 = Color32::from_rgb(0x2d, 0x6b, 0xd0);
pub const WARNING: Color32 = Color32::from_rgb(0xe8, 0xa9, 0x3a);
pub const ERROR: Color32 = Color32::from_rgb(0xf2, 0x66, 0x5e);
pub const SUCCESS: Color32 = Color32::from_rgb(0x4c, 0xc3, 0x8a);
const FOCUS: Color32 = Color32::from_rgb(0x8a, 0xb8, 0xff);

pub fn apply(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for (name, data, family) in [
        (
            "Noto Sans",
            include_bytes!("../assets/fonts/NotoSans-Regular.ttf").as_slice(),
            egui::FontFamily::Proportional,
        ),
        (
            "IBM Plex Mono",
            include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf").as_slice(),
            egui::FontFamily::Monospace,
        ),
    ] {
        fonts
            .font_data
            .insert(name.into(), egui::FontData::from_static(data).into());
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, name.into());
    }
    ctx.set_fonts(fonts);
    ctx.all_styles_mut(|style| {
        style.visuals = egui::Visuals::dark();
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 3.0);
        style.spacing.interact_size = egui::vec2(32.0, 22.0);
        style.spacing.window_margin = egui::Margin::same(12);
        style.spacing.menu_margin = egui::Margin::same(4);
        style.spacing.indent = 16.0;
        style.spacing.icon_width = 14.0;
        style.spacing.icon_width_inner = 8.0;
        style.spacing.icon_spacing = 6.0;
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        style.spacing.scroll.bar_width = 8.0;
        style.spacing.scroll.bar_inner_margin = 2.0;
        style.spacing.scroll.bar_outer_margin = 0.0;
        style.animation_time = 0.1;
        style.visuals.panel_fill = PANEL;
        style.visuals.window_fill = Color32::from_gray(0x2c);
        style.visuals.window_stroke = Stroke::new(1.0, Color32::from_gray(0x45));
        style.visuals.window_corner_radius = 4.into();
        style.visuals.menu_corner_radius = 4.into();
        style.visuals.extreme_bg_color = SUNKEN;
        style.visuals.text_edit_bg_color = Some(SUNKEN);
        style.visuals.code_bg_color = SUNKEN;
        style.visuals.faint_bg_color = Color32::from_gray(0x24);
        style.visuals.weak_text_color = Some(MUTED);
        style.visuals.hyperlink_color = Color32::from_rgb(0x82, 0xb1, 0xff);
        style.visuals.warn_fg_color = WARNING;
        style.visuals.error_fg_color = ERROR;
        style.visuals.selection.bg_fill = Color32::from_rgb(0x1d, 0x35, 0x53);
        style.visuals.selection.stroke = Stroke::new(1.0, TEXT);
        style.visuals.text_cursor.stroke = Stroke::new(1.0, TEXT);
        style.visuals.widgets.noninteractive.bg_fill = PANEL;
        style.visuals.widgets.noninteractive.weak_bg_fill = APP;
        style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, DIVIDER);
        style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        for (widget, fill, border) in [
            (
                &mut style.visuals.widgets.inactive,
                RAISED,
                Color32::from_gray(0x75),
            ),
            (
                &mut style.visuals.widgets.hovered,
                Color32::from_gray(0x38),
                Color32::from_gray(0xa0),
            ),
            (
                &mut style.visuals.widgets.active,
                Color32::from_gray(0x4c),
                FOCUS,
            ),
            (
                &mut style.visuals.widgets.open,
                Color32::from_gray(0x38),
                FOCUS,
            ),
        ] {
            widget.bg_fill = fill;
            widget.weak_bg_fill = fill;
            widget.bg_stroke = Stroke::new(1.0, border);
            widget.fg_stroke = Stroke::new(1.0, TEXT);
            widget.corner_radius = 3.into();
            widget.expansion = 0.0;
        }
        style.text_styles = [
            (egui::TextStyle::Heading, egui::FontId::proportional(15.0)),
            (egui::TextStyle::Body, egui::FontId::proportional(12.0)),
            (egui::TextStyle::Button, egui::FontId::proportional(12.0)),
            (egui::TextStyle::Small, egui::FontId::proportional(11.0)),
            (egui::TextStyle::Monospace, egui::FontId::monospace(11.0)),
        ]
        .into();
    });
}

/// Scoped state colors keep primary actions consistent without changing sibling controls.
pub fn primary_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    let enabled = enabled && ui.is_enabled();
    ui.scope(|ui| {
        let visuals = ui.visuals_mut();
        if !enabled {
            visuals.disabled_alpha = 1.0;
        }
        for (widget, fill) in [
            (&mut visuals.widgets.inactive, ACCENT),
            (
                &mut visuals.widgets.hovered,
                Color32::from_rgb(0x35, 0x72, 0xd8),
            ),
            (
                &mut visuals.widgets.active,
                Color32::from_rgb(0x1d, 0x4e, 0x9e),
            ),
        ] {
            widget.bg_fill = if enabled { fill } else { RAISED };
            widget.weak_bg_fill = widget.bg_fill;
            widget.bg_stroke = Stroke::new(1.0, if enabled { fill } else { DIVIDER });
        }
        let text =
            egui::RichText::new(label).color(if enabled { Color32::WHITE } else { DISABLED });
        let button = egui::Button::new(text).min_size(egui::vec2(72.0, 26.0));
        ui.add_enabled(enabled, if enabled { button } else { button.fill(RAISED) })
    })
    .inner
}
