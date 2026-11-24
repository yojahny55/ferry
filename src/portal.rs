// Asking the desktop for a folder through xdg-desktop-portal, so the picker is the one every
// other app on the desktop uses (on Omarchy, the GTK portal's), not one of Ferry's own. Also the
// other direction: Ferry answering the desktop's "show this file in the file manager" calls.

use std::collections::HashMap;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use zbus::MatchRule;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type;
use zbus::zvariant::{OwnedValue, Value};

use crate::i18n::tr;

/// Shows the desktop's folder picker, starting in `start`, and waits for the answer: the
/// folder, or None when cancelled. Blocks, so call it off the UI thread.
pub fn pick_folder(title: &str, start: &Path) -> Result<Option<PathBuf>, String> {
    let fail =
        |e: zbus::Error| tr("Couldn't open the folder picker: %1").replace("%1", &e.to_string());
    let conn = Connection::session().map_err(fail)?;
    // The portal answers on a request object whose path comes from our bus name and a token we
    // choose, so the answer can be listened for before asking (it may come back very fast).
    static N: AtomicU32 = AtomicU32::new(0);
    let token = format!(
        "ferry{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    );
    let sender = conn
        .unique_name()
        .map(|n| n.trim_start_matches(':').replace('.', "_"))
        .unwrap_or_default();
    let request = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .interface("org.freedesktop.portal.Request")
        .and_then(|r| r.member("Response"))
        .and_then(|r| r.path(request.as_str()))
        .map_err(fail)?
        .build();
    let mut answers = MessageIterator::for_match_rule(rule, &conn, Some(1)).map_err(fail)?;

    let mut folder = start.as_os_str().as_bytes().to_vec();
    folder.push(0);
    let mut options: HashMap<&str, Value> = HashMap::new();
    options.insert("handle_token", Value::from(token.as_str()));
    options.insert("directory", Value::from(true));
    options.insert("modal", Value::from(true));
    options.insert("accept_label", Value::from(tr("Extract Here")));
    options.insert("current_folder", Value::from(folder));
    conn.call_method(
        Some("org.freedesktop.portal.Desktop"),
        "/org/freedesktop/portal/desktop",
        Some("org.freedesktop.portal.FileChooser"),
        "OpenFile",
        &("", title, options),
    )
    .map_err(fail)?;

    let answer = answers
        .next()
        .ok_or_else(|| tr("The folder picker closed without an answer"))?
        .map_err(fail)?;
    let (code, results): (u32, HashMap<String, OwnedValue>) =
        answer.body().deserialize().map_err(fail)?;
    // 1 is Cancel; 2 is "ended some other way", which is what closing the picker's window gives.
    if code != 0 {
        return Ok(None);
    }
    let uris: Vec<String> = results
        .get("uris")
        .and_then(|v| Vec::<String>::try_from(v.try_clone().ok()?).ok())
        .unwrap_or_default();
    Ok(uris.first().and_then(|u| file_path(u)))
}

/// A path from a file:// URI, with %XX escapes decoded.
fn file_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // A host may come first ("file://localhost/home"); the path starts at the next slash.
    let rest = &rest[rest.find('/')?..];
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && let (Some(Some(h)), Some(Some(l))) = (
                bytes.get(i + 1).map(|&b| hex(b)),
                bytes.get(i + 2).map(|&b| hex(b)),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(
        <std::ffi::OsString as std::os::unix::ffi::OsStringExt>::from_vec(out),
    ))
}

/// Serves `org.freedesktop.FileManager1` on the session bus: it's how a browser's "Show in
/// folder" (and other apps) asks the desktop's file manager to open a folder with a file
/// selected. D-Bus starts this on demand as `ferry --file-manager1`; each request opens a
/// window. Runs until the bus connection ends.
pub fn serve_file_manager() -> Result<(), String> {
    let conn = Connection::session().map_err(|e| e.to_string())?;
    conn.request_name("org.freedesktop.FileManager1")
        .map_err(|e| e.to_string())?;
    // argv[0] (D-Bus runs /usr/bin/ferry) rather than current_exe: after an upgrade new windows
    // run the new binary, not the deleted one this long-lived process still has mapped.
    let exe = std::env::args_os().next().unwrap_or_else(|| "ferry".into());
    for msg in MessageIterator::from(&conn) {
        let Ok(msg) = msg else { continue };
        let header = msg.header();
        if header.message_type() != Type::MethodCall
            || header.interface().map(|i| i.as_str()) != Some("org.freedesktop.FileManager1")
        {
            continue;
        }
        let member = header.member().map(|m| m.to_string()).unwrap_or_default();
        let uris = msg
            .body()
            .deserialize::<(Vec<String>, String)>()
            .map(|(uris, _startup_id)| uris)
            .unwrap_or_default();
        let Some(path) = show_target(&member, &uris) else {
            let _ = conn.reply_error(
                &header,
                "org.freedesktop.DBus.Error.InvalidArgs",
                &format!("Nothing to show for {member}"),
            );
            continue;
        };
        // Reaped on a thread so closed windows don't linger as zombies under the service.
        if let Ok(mut child) = Command::new(&exe).arg(path).spawn() {
            std::thread::spawn(move || child.wait());
        }
        let _ = conn.reply(&header, &());
    }
    Ok(())
}

/// What a FileManager1 call should open: a folder (ShowFolders) or a file, which Ferry opens in
/// its folder with the file selected (ShowItems, ShowItemProperties). Plain paths are taken too:
/// not every caller sends a URI. None for an unknown method or nothing to show.
/// shortcut: only the first item is shown, since Ferry selects one file at start; open a window
/// per folder if a caller ever sends several.
fn show_target(member: &str, uris: &[String]) -> Option<PathBuf> {
    if !["ShowFolders", "ShowItems", "ShowItemProperties"].contains(&member) {
        return None;
    }
    let uri = uris.first()?;
    file_path(uri).or_else(|| uri.starts_with('/').then(|| PathBuf::from(uri)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_file_uris() {
        assert_eq!(
            file_path("file:///home/me/Lisbon%202026"),
            Some(PathBuf::from("/home/me/Lisbon 2026"))
        );
        assert_eq!(
            file_path("file://localhost/tmp/a%C3%B1o"),
            Some(PathBuf::from("/tmp/año"))
        );
        assert_eq!(file_path("https://example.com/x"), None);
        assert_eq!(file_path("file:///odd%2"), Some(PathBuf::from("/odd%2")));
    }

    #[test]
    fn file_manager_calls_open_the_first_item() {
        let uris = |u: &[&str]| u.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            show_target(
                "ShowItems",
                &uris(&["file:///home/me/Downloads/a%20b.pdf", "file:///x"])
            ),
            Some(PathBuf::from("/home/me/Downloads/a b.pdf"))
        );
        assert_eq!(
            show_target("ShowFolders", &uris(&["/home/me/Downloads"])),
            Some(PathBuf::from("/home/me/Downloads"))
        );
        assert_eq!(show_target("ShowItems", &[]), None);
        assert_eq!(
            show_target("ShowItems", &uris(&["https://example.com/a"])),
            None
        );
        assert_eq!(show_target("Quit", &uris(&["/tmp"])), None);
    }

    /// Opens the real picker: `FERRY_PORTAL=<dir> cargo test real_picker -- --ignored`, then
    /// accept the folder it opens in.
    #[test]
    #[ignore]
    fn real_picker() {
        let Some(dir) = std::env::var_os("FERRY_PORTAL") else {
            return;
        };
        let dir = PathBuf::from(dir);
        let picked = pick_folder("Ferry test", &dir).unwrap();
        println!("picked: {picked:?}");
        assert_eq!(picked, Some(dir));
    }
}
