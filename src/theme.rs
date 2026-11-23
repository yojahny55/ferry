// Follows the Omarchy theme live: the whole palette from colors.toml, and Hyprland's
// window rounding for corners. Theme watching follows Omacut (MIT, David Heinemeier Hansson).

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use eframe::egui::{self, Color32, CornerRadius, Shadow, Stroke, Visuals};

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Palette {
    pub accent: Color32,
    pub on_accent: Color32,
    pub bg: Color32,
    pub sidebar: Color32,
    pub surface: Color32,
    pub selection: Color32,
    pub fg: Color32,
    pub dim: Color32,
    pub bright: Color32,
    pub red: Color32,
    pub yellow: Color32,
    pub green: Color32,
    pub cyan: Color32,
    pub magenta: Color32,
}

pub fn hex(s: &str) -> Option<Color32> {
    let s = s.strip_prefix('#')?;
    let v = (s.len() == 6)
        .then(|| u32::from_str_radix(s, 16).ok())
        .flatten()?;
    Some(Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// Black or white, whichever reads on `c` (WCAG relative luminance).
pub fn on(c: Color32) -> Color32 {
    let linear = |v: u8| {
        let v = v as f64 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let l = 0.2126 * linear(c.r()) + 0.7152 * linear(c.g()) + 0.0722 * linear(c.b());
    if l > 0.179 {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

impl Palette {
    /// Reads Omarchy's colors.toml; any key it lacks keeps a neutral dark default and Omarchy's yellow accent.
    pub fn parse(text: &str) -> Self {
        let get = |key: &str, fallback: &str| {
            text.lines()
                .find_map(|l| {
                    let (k, v) = l.split_once('=')?;
                    (k.trim() == key).then(|| hex(v.trim().trim_matches(['"', '\''])))?
                })
                .unwrap_or_else(|| hex(fallback).unwrap())
        };
        let accent = get("accent", "#FFD60A");
        Palette {
            accent,
            on_accent: on(accent),
            bg: get("background", "#121214"),
            sidebar: get("dark_background", "#0e0e10"),
            surface: get("lighter_background", "#1e1e22"),
            selection: get("selection", "#2c2c32"),
            fg: get("foreground", "#d4d4d8"),
            dim: get("dark_foreground", "#7a7a85"),
            bright: get("bright_foreground", "#f4f4f5"),
            red: get("red", "#f7768e"),
            yellow: get("yellow", "#e0af68"),
            green: get("green", "#9ece6a"),
            cyan: get("cyan", "#7dcfff"),
            magenta: get("magenta", "#bb9af7"),
        }
    }
}

pub struct Theme {
    file: PathBuf,
    raw: Option<String>,
    pub palette: Palette,
    radius: Arc<AtomicU8>,
    applied: Option<(Palette, u8)>,
}

impl Theme {
    pub fn new(ctx: &egui::Context) -> Self {
        let file = crate::files::home().join(".local/state/omarchy/current/theme/colors.toml");
        let mut theme = Theme {
            file,
            raw: None,
            palette: Palette::parse(""),
            radius: Arc::new(AtomicU8::new(0)),
            applied: None,
        };
        theme.watch_rounding(ctx.clone());
        theme.poll(ctx);
        theme
    }

    pub fn radius(&self) -> u8 {
        self.radius.load(Ordering::Relaxed)
    }

    /// Corners follow Hyprland's effective rounding, which themes and personal overrides can
    /// both set, so ask hyprctl rather than reading its config. Without Hyprland they stay sharp.
    fn watch_rounding(&self, ctx: egui::Context) {
        if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
            return;
        }
        let radius = self.radius.clone();
        std::thread::spawn(move || {
            loop {
                if let Ok(out) = Command::new("hyprctl")
                    .args(["-j", "getoption", "decoration:rounding"])
                    .output()
                {
                    let value = serde_json::from_slice::<serde_json::Value>(&out.stdout)
                        .ok()
                        .and_then(|v| v["int"].as_u64());
                    if let Some(r) = value.filter(|_| out.status.success())
                        && radius.swap(r.min(24) as u8, Ordering::Relaxed) != r.min(24) as u8
                    {
                        ctx.request_repaint();
                    }
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        });
    }

    /// Re-reads colors.toml. Reading the contents (not watching mtimes) also catches a theme
    /// switch, which swaps symlinks to files that may be older than the last ones.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let raw = std::fs::read_to_string(&self.file).ok();
        if raw != self.raw {
            self.palette = Palette::parse(raw.as_deref().unwrap_or(""));
            self.raw = raw;
        }
        let state = (self.palette, self.radius());
        if self.applied != Some(state) {
            apply(ctx, &state.0, state.1);
            self.applied = Some(state);
        }
    }
}

fn apply(ctx: &egui::Context, p: &Palette, r: u8) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|s| {
        let mut v = Visuals::dark();
        v.panel_fill = p.bg;
        v.window_fill = p.surface;
        v.window_stroke = Stroke::new(1.0, p.selection);
        v.extreme_bg_color = p.sidebar;
        v.faint_bg_color = p.surface;
        v.text_edit_bg_color = Some(p.sidebar);
        v.hyperlink_color = p.accent;
        v.error_fg_color = p.red;
        v.warn_fg_color = p.yellow;
        v.selection.bg_fill = p.accent.gamma_multiply(0.45);
        v.selection.stroke = Stroke::new(1.0, p.accent);
        v.window_corner_radius = CornerRadius::same(r);
        v.menu_corner_radius = CornerRadius::same(r);
        v.window_shadow = Shadow {
            offset: [0, 6],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(120),
        };
        v.popup_shadow = Shadow {
            offset: [0, 4],
            blur: 12,
            spread: 0,
            color: Color32::from_black_alpha(100),
        };
        let small = CornerRadius::same(r.min(4));
        let w = &mut v.widgets;
        for (state, bg, fg) in [
            (&mut w.noninteractive, p.bg, p.fg),
            (&mut w.inactive, p.surface, p.fg),
            (&mut w.hovered, p.selection, p.bright),
            (&mut w.active, p.accent, p.on_accent),
            (&mut w.open, p.selection, p.bright),
        ] {
            state.bg_fill = bg;
            state.weak_bg_fill = bg;
            state.fg_stroke = Stroke::new(1.0, fg);
            state.bg_stroke = Stroke::NONE;
            state.corner_radius = small;
            state.expansion = 0.0;
        }
        w.noninteractive.bg_stroke = Stroke::new(1.0, p.selection);
        s.visuals = v;
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        s.spacing.button_padding = egui::vec2(8.0, 4.0);
        s.spacing.interact_size.y = 26.0;
        s.spacing.menu_margin = egui::Margin::same(6);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_without_omarchy() {
        let p = Palette::parse("");
        assert_eq!(p.accent, Color32::from_rgb(0xFF, 0xD6, 0x0A));
        assert_eq!(p.on_accent, Color32::BLACK);
    }

    #[test]
    fn reads_the_palette() {
        let p = Palette::parse(
            "mode = \"dark\"\naccent = \"#112233\"\nbackground='#000000'\nbright_background = \"#ffffff\"\n",
        );
        assert_eq!(p.accent, Color32::from_rgb(0x11, 0x22, 0x33));
        assert_eq!(p.on_accent, Color32::WHITE);
        assert_eq!(p.bg, Color32::BLACK);
    }
}
