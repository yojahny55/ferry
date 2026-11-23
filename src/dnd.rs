// Dragging files out of Ferry into other apps (a browser, a chat, another file manager).
//
// winit, under egui, can't start a Wayland drag, so this module does it on the same Wayland
// connection the window uses: it binds its own seat, pointer and data device on a private
// event queue, remembers the serial of the last button press (the compositor only starts a
// drag for the press that is still held), and serves the dragged files as text/uri-list,
// which browsers and file managers take as dropped files.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_pointer::{self, WlPointer};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, event_created_child};

#[derive(Default)]
struct Shared {
    /// Serial of the last pointer button press on our window.
    serial: Option<u32>,
    /// What a drop target receives: one file:// URI per line.
    uris: String,
}

struct State {
    shared: Arc<Mutex<Shared>>,
}

pub struct Dnd {
    conn: Connection,
    qh: QueueHandle<State>,
    manager: WlDataDeviceManager,
    device: WlDataDevice,
    surface: WlSurface,
    shared: Arc<Mutex<Shared>>,
}

impl Dnd {
    /// Sets up dragging out for the window behind `handles`; `None` outside Wayland.
    pub fn new(handles: &(impl HasDisplayHandle + HasWindowHandle)) -> Option<Self> {
        let RawDisplayHandle::Wayland(display) = handles.display_handle().ok()?.as_raw() else {
            return None;
        };
        let RawWindowHandle::Wayland(window) = handles.window_handle().ok()?.as_raw() else {
            return None;
        };
        // Safety: the pointers come from the live window, which outlives the app.
        let backend = unsafe {
            wayland_backend::client::Backend::from_foreign_display(display.display.as_ptr().cast())
        };
        let conn = Connection::from_backend(backend);
        let surface_id = unsafe {
            wayland_backend::client::ObjectId::from_ptr(
                WlSurface::interface(),
                window.surface.as_ptr().cast(),
            )
        }
        .ok()?;
        let surface = WlSurface::from_id(&conn, surface_id).ok()?;

        let (globals, mut queue) = registry_queue_init::<State>(&conn).ok()?;
        let qh = queue.handle();
        let seat: WlSeat = globals.bind(&qh, 1..=7, ()).ok()?;
        let manager: WlDataDeviceManager = globals.bind(&qh, 3..=3, ()).ok()?;
        let _pointer = seat.get_pointer(&qh, ());
        let device = manager.get_data_device(&seat, &qh, ());

        let shared = Arc::new(Mutex::new(Shared::default()));
        let mut state = State {
            shared: shared.clone(),
        };
        std::thread::Builder::new()
            .name("ferry-dnd".into())
            .spawn(move || while queue.blocking_dispatch(&mut state).is_ok() {})
            .ok()?;
        Some(Dnd {
            conn,
            qh,
            manager,
            device,
            surface,
            shared,
        })
    }

    /// Hands a drag of `paths` to the compositor. Must happen while the button that began the
    /// drag is still held.
    pub fn start(&self, paths: &[PathBuf]) -> Result<(), String> {
        let serial = {
            let mut shared = self.shared.lock().unwrap();
            shared.uris = uri_list(paths);
            shared.serial.ok_or("no pointer press seen yet")?
        };
        let source = self.manager.create_data_source(&self.qh, ());
        source.offer("text/uri-list".into());
        if source.version() >= 3 {
            source.set_actions(DndAction::Copy);
        }
        self.device
            .start_drag(Some(&source), &self.surface, None, serial);
        self.conn.flush().map_err(|e| e.to_string())
    }
}

/// text/uri-list: CRLF-terminated file:// URIs.
pub fn uri_list(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| crate::files::file_uri(p) + "\r\n")
        .collect()
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlDataDeviceManager, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlDataDeviceManager,
        _: <WlDataDeviceManager as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_pointer::Event::Button {
            serial,
            state: WEnum::Value(wl_pointer::ButtonState::Pressed),
            ..
        } = event
        {
            state.shared.lock().unwrap().serial = Some(serial);
        }
    }
}

impl Dispatch<WlDataDevice, ()> for State {
    // Offers made to us (drops onto Ferry, the clipboard) are winit's business, not ours.
    fn event(
        _: &mut Self,
        _: &WlDataDevice,
        _: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }

    event_created_child!(State, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, ()),
    ]);
}

impl Dispatch<WlDataOffer, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlDataOffer,
        _: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlDataSource, ()> for State {
    fn event(
        state: &mut Self,
        source: &WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { fd, .. } => {
                let uris = state.shared.lock().unwrap().uris.clone();
                let _ = std::fs::File::from(fd).write_all(uris.as_bytes());
            }
            wl_data_source::Event::Cancelled | wl_data_source::Event::DndFinished => {
                source.destroy()
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_list_is_crlf_terminated_and_encoded() {
        let list = uri_list(&[
            PathBuf::from("/home/a/My Photo.jpg"),
            PathBuf::from("/tmp/b"),
        ]);
        assert_eq!(list, "file:///home/a/My%20Photo.jpg\r\nfile:///tmp/b\r\n");
    }
}
