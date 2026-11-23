// Follows the Omarchy theme live: the whole palette from colors.toml, Hyprland's window
// rounding for corners, the desktop's text scaling for size, and Omarchy's window opacity
// setting (glassy, opaque, or default) for transparency. Theme watching follows Omacut (MIT,
// David Heinemeier Hansson).

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
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
    /// Lines, tracks and other decoration: present, but not meant to be read.
    pub muted: Color32,
    pub light: bool,
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

/// WCAG contrast ratio between two colors, from 1 to 21.
pub fn contrast(a: Color32, b: Color32) -> f64 {
    let lum = |c: Color32| {
        let linear = |v: u8| {
            let v = v as f64 / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(c.r()) + 0.7152 * linear(c.g()) + 0.0722 * linear(c.b())
    };
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// `a` moved `t` of the way to `b`.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// `c`, moved toward `toward` just enough to read on `on` (WCAG AA for text, 4.5:1). Themes'
/// dark foregrounds are often too faint for text; this keeps their hue but makes them legible.
fn readable(c: Color32, on: Color32, toward: Color32) -> Color32 {
    (0..=20)
        .map(|i| mix(c, toward, i as f32 / 20.0))
        .find(|&m| contrast(m, on) >= 4.5)
        .unwrap_or(toward)
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
        let light = text.lines().any(|l| {
            l.split_once('=')
                .is_some_and(|(k, v)| k.trim() == "mode" && v.contains("light"))
        });
        let bg = get("background", "#121214");
        let fg = get("foreground", "#d4d4d8");
        let sidebar = get("dark_background", "#0e0e10");
        // Secondary text sits on the sidebar as much as on the background: readable on both.
        let darker = if contrast(bg, fg) < contrast(sidebar, fg) {
            bg
        } else {
            sidebar
        };
        let dim = readable(get("dark_foreground", "#7a7a85"), darker, fg);
        Palette {
            accent,
            on_accent: on(accent),
            bg,
            sidebar,
            surface: get("lighter_background", "#1e1e22"),
            selection: get("selection", "#2c2c32"),
            fg,
            dim,
            bright: get("bright_foreground", "#f4f4f5"),
            // Older themes have no `muted`: halfway from secondary text to the background.
            muted: if text
                .lines()
                .any(|l| l.split_once('=').is_some_and(|(k, _)| k.trim() == "muted"))
            {
                get("muted", "#414868")
            } else {
                mix(dim, bg, 0.5)
            },
            light,
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
    /// GTK's text-scaling-factor, as f32 bits.
    text_scale: Arc<AtomicU32>,
    glassy: Arc<AtomicBool>,
    /// The window background's opacity, which text fields and inset surfaces share.
    opacity: f32,
    applied: Option<(Palette, u8, u32)>,
}

impl Theme {
    pub fn new(ctx: &egui::Context) -> Self {
        let file = crate::files::home().join(".local/state/omarchy/current/theme/colors.toml");
        let mut theme = Theme {
            file,
            raw: None,
            palette: Palette::parse(""),
            radius: Arc::new(AtomicU8::new(0)),
            text_scale: Arc::new(AtomicU32::new(1f32.to_bits())),
            glassy: Arc::new(AtomicBool::new(false)),
            opacity: 1.0,
            applied: None,
        };
        theme.watch_desktop(ctx.clone());
        theme.poll(ctx);
        theme
    }

    pub fn radius(&self) -> u8 {
        self.radius.load(Ordering::Relaxed)
    }

    /// How much bigger the desktop draws text (GNOME's text-scaling-factor, which Omarchy sets
    /// for high-resolution screens and GTK apps follow).
    #[cfg(test)]
    pub fn set_text_scale(&self, scale: f32) {
        self.text_scale.store(scale.to_bits(), Ordering::Relaxed);
    }

    pub fn text_scale(&self) -> f32 {
        f32::from_bits(self.text_scale.load(Ordering::Relaxed))
    }

    /// Whether Omarchy's windows are glassy (Appearance > Windows > Window Opacity).
    pub fn glassy(&self) -> bool {
        self.glassy.load(Ordering::Relaxed)
    }

    /// Watches what the desktop decides about every window: corners follow Hyprland's
    /// effective rounding (asked of hyprctl, since themes and overrides can both set it), size
    /// follows GTK's text scaling, and transparency Omarchy's glassy toggle. A thread does it,
    /// since each is a program to run or a file to look at.
    fn watch_desktop(&self, ctx: egui::Context) {
        // Tests set these themselves, and shouldn't depend on the machine's desktop.
        if cfg!(test) {
            return;
        }
        let (radius, text_scale, glassy) = (
            self.radius.clone(),
            self.text_scale.clone(),
            self.glassy.clone(),
        );
        let hyprland = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some();
        let toggles = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::files::home().join(".local/state"))
            .join("omarchy/toggles/hypr/window-glassy.lua");
        std::thread::spawn(move || {
            loop {
                let mut changed = false;
                let scale = Command::new("gsettings")
                    .args(["get", "org.gnome.desktop.interface", "text-scaling-factor"])
                    .output()
                    .ok()
                    .and_then(|o| {
                        String::from_utf8_lossy(&o.stdout)
                            .trim()
                            .parse::<f32>()
                            .ok()
                    })
                    .filter(|s| (0.5..=4.0).contains(s))
                    .unwrap_or(1.0);
                changed |= text_scale.swap(scale.to_bits(), Ordering::Relaxed) != scale.to_bits();
                let glass = toggles.exists();
                changed |= glassy.swap(glass, Ordering::Relaxed) != glass;
                if changed {
                    ctx.request_repaint();
                }
                if hyprland
                    && let Ok(out) = Command::new("hyprctl")
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
        self.restyle(ctx);
    }

    /// Sets how see-through the window is; cheap to call every frame.
    pub fn set_opacity(&mut self, ctx: &egui::Context, opacity: f32) {
        self.opacity = opacity;
        self.restyle(ctx);
    }

    fn restyle(&mut self, ctx: &egui::Context) {
        let state = (self.palette, self.radius(), self.opacity.to_bits());
        if self.applied != Some(state) {
            apply(ctx, &state.0, state.1, self.opacity);
            self.applied = Some(state);
        }
    }
}

fn apply(ctx: &egui::Context, p: &Palette, r: u8, opacity: f32) {
    // Inset surfaces (text fields, the path bar) let the glass through as much as the window.
    let inset = p.sidebar.gamma_multiply(opacity.max(0.5));
    ctx.set_theme(if p.light {
        egui::Theme::Light
    } else {
        egui::Theme::Dark
    });
    ctx.all_styles_mut(|s| {
        let mut v = if p.light {
            Visuals::light()
        } else {
            Visuals::dark()
        };
        v.panel_fill = p.bg;
        v.window_fill = p.surface;
        v.window_stroke = Stroke::new(1.0, p.selection);
        v.extreme_bg_color = inset;
        v.faint_bg_color = p.surface;
        v.text_edit_bg_color = Some(inset);
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
            color: Color32::from_black_alpha(if p.light { 50 } else { 120 }),
        };
        v.popup_shadow = Shadow {
            offset: [0, 4],
            blur: 12,
            spread: 0,
            color: Color32::from_black_alpha(if p.light { 40 } else { 100 }),
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
        w.noninteractive.bg_stroke = Stroke::new(1.0, p.muted);
        // Sliders and checkboxes draw their tracks and boxes with these.
        w.inactive.bg_fill = p.muted;
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.fg);
        s.visuals = v;
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        s.spacing.button_padding = egui::vec2(8.0, 4.0);
        s.spacing.interact_size.y = 28.0;
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
    fn secondary_text_is_readable() {
        // Tokyo Night's dark_foreground is 2.9:1 on its background: too faint for text.
        let p = Palette::parse(
            "background = \"#1a1b26\"\ndark_background = \"#13141c\"\nforeground = \"#a9b1d6\"\n\
             dark_foreground = \"#565f89\"\nmuted = \"#414868\"\nmode = \"dark\"\n",
        );
        assert!(contrast(hex("#565f89").unwrap(), p.bg) < 4.5);
        assert!(contrast(p.dim, p.bg) >= 4.5 && contrast(p.dim, p.sidebar) >= 4.5);
        assert!(
            contrast(p.dim, p.fg) > 1.5,
            "still a step below the main text"
        );
        assert_eq!(p.muted, hex("#414868").unwrap());
        assert!(!p.light);
        assert!(Palette::parse("mode = \"light\"").light);
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
