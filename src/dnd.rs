// Dragging files out of Ferry into other apps (a browser, a chat, another file manager).
//
// winit, under egui, can't start a Wayland drag, so this module does it on the same Wayland
// connection the window uses: it binds its own seat, pointer and data device on a private
// event queue, remembers the serial of the last button press (the compositor only starts a
// drag for the press that is still held), and serves the dragged files as text/uri-list,
// which browsers and file managers take as dropped files.
//
// The connection belongs to winit, which frees it when the window closes. So the event thread
// is stopped and joined while the window still exists (see `shutdown`, called from the app's
// on_exit), and the objects made here are never destroyed: destroying them after winit has
// freed the connection reads freed memory, which crashed Ferry on quit in 0.1.0.

use std::io::Write;
use std::mem::ManuallyDrop;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

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
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, event_created_child,
};

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

/// The objects this module made on winit's connection; never dropped (see the top of the file).
struct Wayland {
    conn: Connection,
    qh: QueueHandle<State>,
    manager: WlDataDeviceManager,
    device: WlDataDevice,
    surface: WlSurface,
}

pub struct Dnd {
    wl: ManuallyDrop<Wayland>,
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
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
        // Safety: the pointers come from the live window. Everything here stops using them
        // before it goes away: `shutdown` runs while the window still exists.
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

        let (globals, queue) = registry_queue_init::<State>(&conn).ok()?;
        let qh = queue.handle();
        let seat: WlSeat = globals.bind(&qh, 1..=7, ()).ok()?;
        let manager: WlDataDeviceManager = globals.bind(&qh, 3..=3, ()).ok()?;
        let _pointer = seat.get_pointer(&qh, ());
        let device = manager.get_data_device(&seat, &qh, ());

        let shared = Arc::new(Mutex::new(Shared::default()));
        let mut state = State {
            shared: shared.clone(),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("ferry-dnd".into())
                .spawn(move || pump(queue, &mut state, &stop))
                .ok()?
        };
        Some(Dnd {
            wl: ManuallyDrop::new(Wayland {
                conn,
                qh,
                manager,
                device,
                surface,
            }),
            shared,
            stop,
            thread: Some(thread),
        })
    }

    /// Stops the event thread and waits for it. Call while the window still exists.
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    /// Hands a drag of `paths` to the compositor. Must happen while the button that began the
    /// drag is still held.
    pub fn start(&self, paths: &[PathBuf]) -> Result<(), String> {
        let serial = {
            let mut shared = self.shared.lock().unwrap();
            shared.uris = uri_list(paths);
            shared.serial.ok_or("no pointer press seen yet")?
        };
        let wl = &self.wl;
        let source = wl.manager.create_data_source(&wl.qh, ());
        source.offer("text/uri-list".into());
        if source.version() >= 3 {
            source.set_actions(DndAction::Copy);
        }
        wl.device
            .start_drag(Some(&source), &wl.surface, None, serial);
        wl.conn.flush().map_err(|e| e.to_string())
    }
}

impl Drop for Dnd {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Dispatches this module's events until told to stop or the connection fails. It waits on
/// the socket at most 100 ms at a time, so a stop request is noticed promptly; a plain
/// `blocking_dispatch` would sit in the socket until winit had already freed the connection.
fn pump(mut queue: EventQueue<State>, state: &mut State, stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        if queue.dispatch_pending(state).is_err() {
            break;
        }
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let mut poll = libc::pollfd {
            fd: guard.connection_fd().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // Safety: one valid pollfd for the duration of the call.
        let ready = unsafe { libc::poll(&mut poll, 1, 100) } > 0;
        // Dropping the guard without reading cancels this read, as libwayland requires.
        if ready && guard.read().is_err() {
            break;
        }
    }
    // Never drop the queue: its last reference would destroy objects on winit's connection.
    std::mem::forget(queue);
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
