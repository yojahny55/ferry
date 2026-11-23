// ferry — a keyboard-first file manager that follows the Omarchy theme.

mod app;
mod dnd;
mod editor;
mod files;
mod i18n;
mod theme;
mod thumbs;

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

fn main() -> eframe::Result {
    // `ferry [path]`: a folder opens there, a file opens its folder with it selected (the app
    // works out which on its loader thread). Nothing here touches the disk: the path may be on
    // a network mount that isn't answering, and the window has to open regardless.
    let start = std::env::args_os()
        .nth(1)
        .and_then(|p| std::path::absolute(PathBuf::from(p)).ok())
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .filter(|d| d != &PathBuf::from("/"))
        })
        .unwrap_or_else(files::home);
    let select = None;

    let options = eframe::NativeOptions {
        // The app id matches ferry.desktop, so Hyprland and the launcher pick up its icon.
        viewport: egui::ViewportBuilder::default()
            .with_app_id("ferry")
            .with_title("Ferry")
            .with_inner_size([1100.0, 700.0])
            .with_min_inner_size([640.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native(
        "ferry",
        options,
        Box::new(move |cc| {
            set_fonts(&cc.egui_ctx);
            Ok(Box::new(
                app::App::new(&cc.egui_ctx, start, select)
                    .with_dnd(dnd::Dnd::new(cc))
                    .with_settings(app::settings_file()),
            ))
        }),
    )
}

/// Uses the desktop's monospace font (Omarchy's, set with `omarchy font`), whose
/// Nerd Font glyphs draw the icons. egui's own fonts stay behind it for anything it lacks.
fn set_fonts(ctx: &egui::Context) {
    let Ok(out) = Command::new("fc-match")
        .args(["-f", "%{file}", "monospace"])
        .output()
    else {
        return;
    };
    let Ok(data) = std::fs::read(String::from_utf8_lossy(&out.stdout).as_ref()) else {
        return;
    };
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("omarchy".into(), Arc::new(FontData::from_owned(data)));
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, "omarchy".into());
    }
    ctx.set_fonts(fonts);
}
