//! Keeps the screen from locking, blanking or starting the screensaver while a recording is live.
//!
//! A long class with no keyboard or mouse input must not be cut by the idle timers. On Wayland
//! the way to ask for that is the `idle-inhibit` protocol, and it is the one that works on
//! Omarchy/Hyprland: Hyprland does not read logind's `idle` inhibitors (checked with an
//! `ext-idle-notify` listener: `systemd-inhibit --what=idle` did not stop it from firing, a
//! protocol inhibitor did), and the Omarchy shell takes its idle events from that protocol.
//!
//! The inhibitor lives as long as the Wayland connection that created it. Nothing has to be
//! cleaned up when the process dies (`kill -9`, a crash, a panic): the compositor drops the
//! inhibitor when the socket closes. `exec()` (the updater's restart) closes it too, because
//! the socket is close-on-exec, and `relaunch` also releases it first.
//!
//! Only idle is inhibited. Closing the lid (`handle-lid-switch`) and the power key are other
//! mechanisms and still suspend the laptop on purpose.

use std::any::Any;

/// Holds the idle inhibitor while it is wanted; dropping it, or `set(false)`, releases it.
pub struct Awake {
    open: fn() -> Result<Box<dyn Any>, String>,
    hold: Option<Box<dyn Any>>,
    /// Said once, so a machine without the protocol does not repeat it on every recording.
    said: bool,
}

impl Awake {
    /// The real thing; inside this crate's unit tests it does nothing, so they never touch the
    /// user's session.
    pub fn system() -> Awake {
        if cfg!(test) { Awake::with(|| Ok(Box::new(()))) } else { Awake::with(system_hold) }
    }

    pub fn with(open: fn() -> Result<Box<dyn Any>, String>) -> Awake {
        Awake { open, hold: None, said: false }
    }

    /// True while the inhibitor is held.
    #[cfg(test)]
    pub fn held(&self) -> bool {
        self.hold.is_some()
    }

    /// Takes the inhibitor when `on` and it is not held yet, and releases it when not `on`.
    /// Failing to take it is not an error for the caller: the recording goes on as it was.
    pub fn set(&mut self, on: bool) {
        match (on, self.hold.is_some()) {
            (true, false) => match (self.open)() {
                Ok(h) => self.hold = Some(h),
                Err(e) if !self.said => {
                    self.said = true;
                    eprintln!("hushnote: the screen may lock while recording: {e}");
                }
                Err(_) => {}
            },
            (false, true) => self.hold = None,
            _ => {}
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn system_hold() -> Result<Box<dyn Any>, String> {
    Err("idle inhibit is only implemented for Wayland".into())
}

#[cfg(target_os = "linux")]
fn system_hold() -> Result<Box<dyn Any>, String> {
    wl::Inhibitor::new().map(|i| Box::new(i) as Box<dyn Any>)
}

#[cfg(target_os = "linux")]
mod wl {
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::{wl_compositor::WlCompositor, wl_registry::WlRegistry, wl_surface::WlSurface};
    use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
    use wayland_protocols::wp::idle_inhibit::zv1::client::zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1;
    use wayland_protocols::wp::idle_inhibit::zv1::client::zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1;

    struct State;

    /// Our own short connection with one surface that has no role and no buffer: Hyprland
    /// still honours the inhibitor, and it holds on every workspace.
    pub struct Inhibitor {
        conn: Connection,
        surface: WlSurface,
        inhibitor: ZwpIdleInhibitorV1,
    }

    impl Inhibitor {
        pub fn new() -> Result<Inhibitor, String> {
            let conn = Connection::connect_to_env().map_err(|e| format!("no Wayland session ({e})"))?;
            let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(|e| format!("Wayland: {e}"))?;
            let qh = queue.handle();
            let compositor: WlCompositor =
                globals.bind(&qh, 1..=1, ()).map_err(|e| format!("no wl_compositor ({e})"))?;
            let manager: ZwpIdleInhibitManagerV1 =
                globals.bind(&qh, 1..=1, ()).map_err(|e| format!("the compositor has no idle-inhibit ({e})"))?;
            let surface = compositor.create_surface(&qh, ());
            let inhibitor = manager.create_inhibitor(&surface, &qh, ());
            manager.destroy();
            queue.roundtrip(&mut State).map_err(|e| format!("Wayland: {e}"))?;
            Ok(Inhibitor { conn, surface, inhibitor })
        }
    }

    impl Drop for Inhibitor {
        fn drop(&mut self) {
            self.inhibitor.destroy();
            self.surface.destroy();
            let _ = self.conn.flush();
            // Dropping `conn` closes the socket; the compositor frees whatever is left.
        }
    }

    macro_rules! ignore_events {
        ($($t:ty),*) => {$(
            impl Dispatch<$t, ()> for State {
                fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
            }
        )*};
    }
    ignore_events!(WlCompositor, WlSurface, ZwpIdleInhibitManagerV1, ZwpIdleInhibitorV1);

    impl Dispatch<WlRegistry, GlobalListContents> for State {
        fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static OPENED: AtomicUsize = AtomicUsize::new(0);
    static LIVE: AtomicUsize = AtomicUsize::new(0);

    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            LIVE.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn takes_once_and_releases() {
        let ok: fn() -> Result<Box<dyn Any>, String> = || {
            OPENED.fetch_add(1, Ordering::SeqCst);
            LIVE.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(Guard))
        };
        let mut a = Awake::with(ok);
        a.set(false);
        assert!(!a.held());
        assert_eq!(OPENED.load(Ordering::SeqCst), 0, "nothing is taken while not wanted");
        a.set(true);
        a.set(true);
        assert!(a.held());
        assert_eq!((OPENED.load(Ordering::SeqCst), LIVE.load(Ordering::SeqCst)), (1, 1), "no double acquire");
        a.set(false);
        a.set(false);
        assert!(!a.held());
        assert_eq!(LIVE.load(Ordering::SeqCst), 0);
        a.set(true);
        assert_eq!((OPENED.load(Ordering::SeqCst), LIVE.load(Ordering::SeqCst)), (2, 1), "it can be taken again");
        drop(a);
        assert_eq!(LIVE.load(Ordering::SeqCst), 0, "dropping releases");
    }

    #[test]
    fn failing_to_take_it_is_harmless() {
        let mut a = Awake::with(|| Err("no compositor".into()));
        a.set(true);
        a.set(true);
        assert!(!a.held());
        a.set(false);
        assert!(!a.held());
    }

    /// Holds the real inhibitor for `HUSHNOTE_HOLD_SECS` seconds, then releases it. Driven by a
    /// shell script that checks, from outside, whether the compositor still sees it.
    #[test]
    #[ignore = "needs a Wayland session; run by hand"]
    #[cfg(target_os = "linux")]
    fn hold_real_inhibitor() {
        let secs: u64 = std::env::var("HUSHNOTE_HOLD_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(10);
        let mut a = Awake::with(system_hold);
        a.set(true);
        assert!(a.held(), "could not take the inhibitor");
        println!("HELD pid={}", std::process::id());
        if std::env::var("HUSHNOTE_HOLD_EXEC").is_ok() {
            use std::os::unix::process::CommandExt;
            let e = std::process::Command::new("sleep").arg(secs.to_string()).exec();
            panic!("exec failed: {e}");
        }
        std::thread::sleep(std::time::Duration::from_secs(secs));
        a.set(false);
        println!("RELEASED");
        std::thread::sleep(std::time::Duration::from_secs(secs));
    }
}
