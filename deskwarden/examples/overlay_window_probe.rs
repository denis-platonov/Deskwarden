//! **Where the region overlay's 1.8 s tail went** -- a probe, because no test
//! process can reproduce it.
//!
//! The owner's log had the overlay's viewport registered and then, 1739-1816
//! ms later, its window found, with one root frame in between: the event loop
//! working, not waiting, and the waiting card's bar frozen for all of it. Two
//! candidates were named -- the 29 MB desktop texture, and `eframe` building a
//! full-screen window with its GL surface -- and this probe measured both and
//! found neither. What it found instead, from inside a running frame:
//!
//! ```text
//! Creating a window for viewport "B35B"      (t)
//! Initializing egui_winit for viewport "B35B" (t + 9..19 ms)     <- the window
//! made context current. setting swap interval (t + 2 ms after)  <- the surface
//! own_window_titled took 419 ms; found=None
//! own_window_titled took 406 ms; found=Some(..)                   <- the tail
//! ```
//!
//! `EnumWindows` over the desktop's 404 windows is 0 ms. What cost 430 ms per
//! lookup was reading the caption of two windows this process owns but did
//! not make -- the NVIDIA driver's `NVOpenGLPbuffer` and its thread's IME
//! window -- each `GetWindowTextW` a `WM_GETTEXT` sent to a driver thread that
//! pumps every ~100 ms. `foreground::win32::window_title` carries the per-call
//! numbers and the fix. This file is kept so the measurement can be repeated:
//! a `cargo test` process never holds a GL context, so it never has those
//! windows, and nothing in it can show the cost or its absence.
//!
//! Nothing is drawn on screen: the root is created hidden and so is the probe
//! viewport, exactly as the real overlay's is. Run with
//!
//! ```text
//! RUST_LOG=eframe=debug,overlay_window_probe=info PROBE_VARIANT=exact \
//!   cargo run --release --example overlay_window_probe
//! ```
//!
//! `PROBE_VARIANT` is one of `exact` (the overlay's builder, verbatim),
//! `no_topmost`, `taskbar`, `decorated`, `small` (1280x720) and `tiny`
//! (64x64); the window build measured the same in all six. `winit=debug`
//! inflates the build to ~115 ms with its own logging, so it is not in the
//! line above.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use eframe::egui;

const TITLE: &str = "Deskwarden — overlay window probe";

static T0: OnceLock<Instant> = OnceLock::new();
static CB_FRAMES: AtomicU32 = AtomicU32::new(0);

fn ms() -> u128 {
    T0.get().map_or(0, |t| t.elapsed().as_millis())
}

struct Probe {
    variant: String,
    display: deskwarden::screen_capture::ScreenRect,
    root_frames: u32,
    registered_at: Option<Instant>,
    frames_since_registered: u32,
    found_at: Option<Instant>,
    frames_since_found: u32,
}

impl eframe::App for Probe {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &root.ctx().clone();
        self.root_frames += 1;
        let frame_began = Instant::now();
        if self.registered_at.is_some() {
            self.frames_since_registered += 1;
        }
        log::info!("root frame {} at {} ms", self.root_frames, ms());

        // Once before the viewport exists and once after, so the per-window
        // cost can be read against the set of windows the process had.
        if self.root_frames == 3 || self.root_frames == 7 {
            time_own_windows();
        }
        if self.root_frames == 5 {
            self.registered_at = Some(Instant::now());
            log::info!("registering the viewport at {} ms ({})", ms(), self.variant);
        }

        if let Some(registered) = self.registered_at {
            if self.found_at.is_none() {
                // The same lookup `region_overlay::appear` makes.
                let began = Instant::now();
                let found = deskwarden::foreground::own_window_titled(TITLE);
                log::info!(
                    "own_window_titled took {} ms; found={:?}",
                    began.elapsed().as_millis(),
                    found
                );
                if found.is_some() {
                    self.found_at = Some(Instant::now());
                    log::info!(
                        "RESULT {}: the window existed {} ms after the viewport was registered, \
                         with {} root frame(s) in between and {} callback frame(s) painted",
                        self.variant,
                        registered.elapsed().as_millis(),
                        self.frames_since_registered,
                        CB_FRAMES.load(Ordering::SeqCst)
                    );
                }
            } else {
                self.frames_since_found += 1;
                if self.frames_since_found >= 5 {
                    log::info!(
                        "done at {} ms with {} callback frame(s)",
                        ms(),
                        CB_FRAMES.load(Ordering::SeqCst)
                    );
                    std::process::exit(0);
                }
            }

            let scale = ctx.pixels_per_point();
            let display = self.display;
            let (w, h) = match self.variant.as_str() {
                "small" => (1280.0, 720.0),
                "tiny" => (64.0, 64.0),
                _ => (display.width() as f32, display.height() as f32),
            };
            let mut builder = egui::ViewportBuilder::default()
                .with_title(TITLE)
                .with_position(egui::pos2(
                    display.left as f32 / scale,
                    display.top as f32 / scale,
                ))
                .with_inner_size([w / scale, h / scale])
                .with_visible(false);
            if self.variant != "decorated" {
                builder = builder.with_decorations(false);
            }
            if self.variant != "no_topmost" {
                builder = builder.with_always_on_top();
            }
            if self.variant != "taskbar" {
                builder = builder.with_taskbar(false);
            }
            ctx.show_viewport_deferred(
                egui::ViewportId::from_hash_of(TITLE),
                builder,
                move |root, _class| {
                    let n = CB_FRAMES.fetch_add(1, Ordering::SeqCst) + 1;
                    if n <= 3 {
                        log::info!("callback frame {n} at {} ms", ms());
                    }
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(root, |ui| {
                            let full = ui.max_rect();
                            ui.painter().rect_filled(full, 0.0, egui::Color32::from_gray(40));
                        });
                    root.request_repaint_of(egui::ViewportId::from_hash_of(TITLE));
                },
            );
            ctx.request_repaint_of(egui::ViewportId::from_hash_of(TITLE));
        }
        log::info!(
            "root frame {} took {} ms in update",
            self.root_frames,
            frame_began.elapsed().as_millis()
        );
        ctx.request_repaint();
    }
}

/// Before the event loop exists: how much of `own_window_titled` is
/// `EnumWindows` itself, how much the per-window pid read, and what a
/// `FindWindowW` by title costs instead. All of it is 0 ms here, which is the
/// control: the cost below appears only once this process has a GL context.
fn measure_lookups() {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, FindWindowW, GetWindowThreadProcessId,
    };

    unsafe extern "system" fn count_only(_: HWND, lparam: LPARAM) -> BOOL {
        *(lparam.0 as *mut u32) += 1;
        BOOL(1)
    }
    unsafe extern "system" fn count_with_pid(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        *(lparam.0 as *mut u32) += 1;
        BOOL(1)
    }
    for round in 0..3 {
        let mut n = 0u32;
        let began = Instant::now();
        unsafe {
            let _ = EnumWindows(Some(count_only), LPARAM(&mut n as *mut u32 as isize));
        }
        let count_only_ms = began.elapsed().as_millis();
        let mut m = 0u32;
        let began = Instant::now();
        unsafe {
            let _ = EnumWindows(Some(count_with_pid), LPARAM(&mut m as *mut u32 as isize));
        }
        let with_pid_ms = began.elapsed().as_millis();
        let began = Instant::now();
        let own = deskwarden::foreground::own_window_titled("no such window");
        let own_ms = began.elapsed().as_millis();
        let title: Vec<u16> = "no such window\0".encode_utf16().collect();
        let began = Instant::now();
        let found = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())) };
        let find_us = began.elapsed().as_micros();
        log::info!(
            "LOOKUP round {round}: EnumWindows count-only {n} windows in {count_only_ms} ms; \
             with GetWindowThreadProcessId {m} windows in {with_pid_ms} ms; own_window_titled \
             {own_ms} ms ({own:?}); FindWindowW {find_us} us ({found:?})"
        );
    }
}

/// Every window of this process, with what each call `foreground`'s
/// enumeration used to make costs on it, and which thread owns it. This is
/// the table that named the cause: `GetWindowTextLengthW` and
/// `GetWindowTextW` at ~108 ms each on the two driver windows, microseconds
/// on every window this thread made.
fn time_own_windows() {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindowLongW, GetWindowTextLengthW, GetWindowTextW,
        GetWindowThreadProcessId, InternalGetWindowText, IsIconic, IsWindowVisible, GWL_EXSTYLE,
    };

    unsafe extern "system" fn per_window(hwnd: HWND, _: LPARAM) -> BOOL {
        let mut pid = 0u32;
        let tid = GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid != GetCurrentProcessId() {
            return BOOL(1);
        }
        let mut class = [0u16; 128];
        let n = GetClassNameW(hwnd, &mut class);
        let class = String::from_utf16_lossy(&class[..n.max(0) as usize]);
        let began = Instant::now();
        let len = GetWindowTextLengthW(hwnd);
        let len_us = began.elapsed().as_micros();
        let began = Instant::now();
        let mut buffer = vec![0u16; len.max(0) as usize + 1];
        let copied = GetWindowTextW(hwnd, &mut buffer);
        let text_us = began.elapsed().as_micros();
        let title = String::from_utf16_lossy(&buffer[..copied.max(0) as usize]);
        let began = Instant::now();
        let mut internal = [0u16; 512];
        let copied = InternalGetWindowText(hwnd, &mut internal);
        let internal_us = began.elapsed().as_micros();
        let same = String::from_utf16_lossy(&internal[..copied.max(0) as usize]) == title;
        let began = Instant::now();
        let visible = IsWindowVisible(hwnd).as_bool();
        let iconic = IsIconic(hwnd).as_bool();
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE);
        let rest_us = began.elapsed().as_micros();
        log::info!(
            "OWN {hwnd:?} class {class:?} title {title:?} thread {tid} (this thread {}): \
             GetWindowTextLengthW {len_us} us, GetWindowTextW {text_us} us, \
             InternalGetWindowText {internal_us} us (same title: {same}), \
             visible/iconic/exstyle {rest_us} us (visible={visible} iconic={iconic} ex={ex:#x})",
            GetCurrentThreadId()
        );
        BOOL(1)
    }
    let began = Instant::now();
    unsafe {
        let _ = EnumWindows(Some(per_window), LPARAM(0));
    }
    log::info!("OWN: the timed enumeration took {} ms", began.elapsed().as_millis());
}

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();
    T0.set(Instant::now()).ok();
    let variant = std::env::var("PROBE_VARIANT").unwrap_or_else(|_| "exact".to_string());
    measure_lookups();
    let monitors = deskwarden::screen_capture::monitor_bounds();
    let display = monitors.first().copied().expect("a monitor");
    log::info!("probe: variant {variant}, display {display:?}");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("overlay probe root")
            .with_inner_size([320.0, 200.0])
            .with_visible(false),
        ..Default::default()
    };
    eframe::run_native(
        "overlay_window_probe",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(Probe {
                variant,
                display,
                root_frames: 0,
                registered_at: None,
                frames_since_registered: 0,
                found_at: None,
                frames_since_found: 0,
            }))
        }),
    )
}
