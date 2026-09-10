//! Reading a QR code off a **camera**, for design 6a's fourth route.
//!
//! This is the OS-touching half of "use a webcam", and it stands in the same
//! relation to [`crate::qr`] that [`crate::screen_capture`] does: it is handed
//! nothing and it produces pixels; what the pixels are for is somebody else's
//! problem. The two routes differ in exactly one way that matters to the
//! shape of this file. A region capture is **one blit on demand** -- the user
//! releases the mouse and a rectangle comes back. A camera is a **stream that
//! runs until somebody stops it**, and a stream on the UI thread is a frozen
//! window.
//!
//! # The device never touches the UI thread
//!
//! Everything between opening the device and closing it happens on a thread
//! this module spawns. The UI thread's whole relationship with the camera is
//! [`Session::take`], which locks a mutex, moves whatever is in it out, and
//! returns -- it never waits for a frame, never waits for the device, and
//! cannot be made to. That is deliberate and it is the defect class this
//! project fixed the day before this route was written: `region_overlay`'s
//! viewport callback re-ran a capture every frame with no exit and hung the
//! app. A camera that has been unplugged mid-stream, or whose driver has
//! wedged, produces exactly that hang if the UI thread is the thing asking it
//! for pictures.
//!
//! There is **one** call in this module the UI thread does make: [`devices`],
//! which enumerates the video capture devices once, when the user presses the
//! row. It is bounded (it asks Media Foundation for a list and returns), it is
//! one-shot, and it is made from the same place and for the same reason as
//! `IFileOpenDialog::Show` on the image route -- out of the action handler,
//! after the frame's draw closures have returned. That dialog holds the frame
//! for as long as the user browses; this holds it for as long as Windows takes
//! to list the cameras.
//!
//! # Releasing the device
//!
//! A camera light left on after the user has moved on is not a tidiness
//! problem. It is the visible signature of spyware, and a password manager
//! cannot afford to look like one for even a second longer than it has to. So
//! release is structural rather than a matter of remembering:
//!
//! * [`Session`] owns an `Arc<Shared>` carrying a stop flag, and its `Drop`
//!   sets that flag. Every way the surface can end -- the stage left, the
//!   modal closed, the vault locked, the window destroyed, a panic unwinding
//!   through the frame -- drops the `TotpAdd` that owns the session, so every
//!   one of them sets the flag.
//! * The capture loop checks the flag before every read and abandons the
//!   device the first time it is set. It is **not joined**, deliberately:
//!   `IMFSourceReader::ReadSample` blocks until the next frame, so a join
//!   would block the UI thread for up to one frame interval on a healthy
//!   camera and *forever* on a wedged one. The honest statement is that the
//!   device is released within one frame of the stop rather than at the
//!   instant of it, and that is the trade this file takes rather than the
//!   hang.
//! * The device itself is shut down through [`Opened`], a guard whose `Drop`
//!   calls `IMFMediaSource::Shutdown` -- which is the call that actually
//!   frees the hardware, and is the application's to make rather than the
//!   source reader's.
//! * A code that reads successfully stops the session itself, from the
//!   capture thread, in the same call that reports it. The user is on the
//!   confirmation before the next frame would have arrived.
//!
//! # The frames ARE the secret
//!
//! A QR code of an `otpauth://` URI is the seed in visual form, so a frame of
//! a camera pointed at one is exactly as sensitive as the seed. [`Frame`]'s
//! buffer is a [`Zeroizing`], the frame the code was read out of is dropped
//! rather than handed to the preview, and the slot [`Sink::show`] writes into
//! wipes whatever it replaces. **Nothing here writes a file**: no capture is
//! saved, no diagnostic path dumps one, and no `log` call carries pixels.
//! [`Frame`]'s and [`Verdict`]'s `Debug` are hand-written so `debug_leak_guard`
//! has nothing to catch.
//!
//! The one copy this module cannot wipe is the **preview**: the surface hands
//! each frame to `egui` as a texture, and a texture is not a `Zeroizing`. That
//! is stated rather than glossed, and it is bounded on both ends -- the
//! texture is freed with the stage, and what it holds is a picture of a code
//! that is at that moment being held up in front of the machine.
//!
//! # What is testable here, and what is not
//!
//! Testable, and tested below with no camera anywhere:
//!
//! * [`pack_rgba`] -- the stride, row order and channel order arithmetic that
//!   turns Media Foundation's `RGB32` into what [`crate::qr::decode_qr`]
//!   reads. This is where a camera route goes wrong silently: a flipped or
//!   channel-swapped buffer decodes nothing and looks like a bad camera.
//! * [`DecodeCadence`] -- the bound on how often a decode is attempted.
//! * [`Session`], [`Sink`] and [`offer`] -- every state the surface can be in
//!   (no device, device busy, frames arriving, a decode landing, a stream that
//!   ends, a camera that never sends anything) is reachable through a
//!   [`Session::start`]ed producer that is an ordinary closure.
//! * [`WebcamSeams::production`] -- asserted by address, so a seam quietly
//!   re-pointed at a stub fails rather than passing.
//!
//! **Not testable, and no assertion below pretends otherwise:** that Media
//! Foundation enumerates a real camera, that a real driver honours the media
//! type this file asks for, and that the picture that comes back is of what
//! the lens is pointed at. No test in this crate may open a capture device.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// **Why a camera produced nothing.** Every variant names its reason, for
/// [`crate::screen_capture::CaptureRefusal`]'s reason exactly: a refusal the
/// user can act on is a different thing from a blank one, and a route that
/// answers "something went wrong" teaches them to retry the thing that will
/// not work.
///
/// Carries nothing derived from a frame and nothing derived from an OS error
/// code, so no arm of [`Self::detail`] can print either. An `HRESULT` on
/// screen is the shape of message this app refuses everywhere else, and a
/// camera is the surface most likely to produce one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraRefusal {
    /// Media Foundation enumerated no video capture devices at all.
    NoCamera,
    /// The device exists and something else has it open.
    Busy,
    /// Windows refused this process the camera. The Settings privacy switch,
    /// or a managed policy.
    Denied,
    /// The device opened and never sent a picture. A lens cover and a hardware
    /// kill switch both look exactly like this, which is why the advice names
    /// them.
    Silent,
    /// Frames were arriving and then stopped -- unplugged, or taken away.
    Lost,
    /// Media Foundation refused for a reason this module does not model. The
    /// catch-all, and it says what to do rather than what happened, because
    /// there is nothing useful to say about what happened.
    Unavailable,
}

impl CameraRefusal {
    /// The headline, in the register of design 6d's own refusals.
    pub fn title(&self) -> &'static str {
        match self {
            CameraRefusal::NoCamera => "No camera on this PC",
            CameraRefusal::Busy => "That camera is already in use",
            CameraRefusal::Denied => "Windows is not allowing camera access",
            CameraRefusal::Silent => "That camera sent no picture",
            CameraRefusal::Lost => "That camera stopped",
            CameraRefusal::Unavailable => "Windows couldn't open that camera",
        }
    }

    /// The sentence under the headline: **what the user can do about it.**
    ///
    /// Not an apology, in any arm. "Sorry" is not a step, and the user is
    /// standing in front of a picker with three other routes on it -- two of
    /// which need no camera at all, which is why two of these say so.
    pub fn detail(&self) -> &'static str {
        match self {
            CameraRefusal::NoCamera => {
                "Plug one in and choose this again, or scan the code off your screen instead."
            }
            CameraRefusal::Busy => {
                "Close whatever has it open \u{2014} a video call, or a camera app \u{2014} and \
                 choose it again."
            }
            CameraRefusal::Denied => {
                "Turn on camera access for desktop apps in Settings \u{2192} Privacy & security \
                 \u{2192} Camera, then choose it again."
            }
            CameraRefusal::Silent => {
                "Check nothing is covering the lens and that any privacy switch on it is open, \
                 then choose it again."
            }
            CameraRefusal::Lost => {
                "It was unplugged or taken by another app. Plug it back in and choose it again."
            }
            CameraRefusal::Unavailable => {
                "Try once more, or enter the secret the site prints under the code by hand."
            }
        }
    }
}

// ---------------------------------------------------------------------------
// What a device is, and what a frame is
// ---------------------------------------------------------------------------

/// One video capture device, as the picker shows it.
///
/// [`Self::id`] is the device's **symbolic link** -- Windows' own stable name
/// for it -- and it is what the capture thread re-opens the device by. The
/// `IMFActivate` that [`devices`] enumerated is deliberately *not* carried
/// across the thread boundary: it is a COM object created in whatever
/// apartment the UI thread happens to be in, and handing one to another
/// apartment is how a camera route acquires an intermittent, unreproducible
/// failure. A string is a string.
///
/// Neither field is a secret -- a camera's model name is not vault data -- so
/// this one derives everything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// What Windows calls it, e.g. "Integrated Camera". Painted verbatim.
    pub name: String,
    /// The symbolic link. Never painted; it is a device path, not prose.
    pub id: String,
}

/// One captured frame, as straight RGBA8, rows top to bottom, no padding --
/// exactly what [`crate::qr::decode_qr`] documents and what
/// `egui::ColorImage::from_rgba_unmultiplied` wants.
///
/// **No derived `Debug`**: the buffer is a picture of a seed. The hand-written
/// one below prints the size and never a byte of the contents, which is
/// [`crate::screen_capture::Rgba`]'s decision for the same reason.
///
/// Not `Clone`, also for that type's reason: every copy is another copy of the
/// seed, and a derived `Clone` is how one gets made without anyone deciding
/// to make it.
pub struct Frame {
    width: usize,
    height: usize,
    rgba: Zeroizing<Vec<u8>>,
}

impl Frame {
    /// A frame from pixels already in the layout above.
    ///
    /// `None` when the buffer is shorter than the dimensions claim, rather
    /// than a panic: this is built from a driver's output, and a short buffer
    /// is a thing a driver can produce.
    pub fn new(width: usize, height: usize, rgba: Zeroizing<Vec<u8>>) -> Option<Frame> {
        let needed = width.checked_mul(height)?.checked_mul(4)?;
        if width == 0 || height == 0 || rgba.len() < needed {
            return None;
        }
        Some(Frame { width, height, rgba })
    }

    /// Width in pixels.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> usize {
        self.height
    }

    /// The pixels. Borrowed rather than handed over, so the only owner -- and
    /// therefore the only thing that decides when they are wiped -- stays this
    /// value.
    pub fn pixels(&self) -> &[u8] {
        &self.rgba
    }
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Frame({}x{}, pixels not shown)", self.width, self.height)
    }
}

/// **How a session ended.** Exactly one of these is ever recorded, and
/// recording one is what stops the capture loop.
///
/// Hand-written `Debug` for [`Frame`]'s reason: [`Self::Read`] holds the seed.
pub enum Verdict {
    /// A QR decoded. The payload is **untrusted** -- it is whatever was held
    /// up in front of the camera, chosen by whoever talked the user into
    /// scanning it -- and is handed to `otpauth::parse_otpauth` by the caller,
    /// never treated as a URL, a path or a command by anything here.
    Read(Zeroizing<String>),
    /// The camera refused, and says why.
    Refused(CameraRefusal),
    /// The stream ended on its own without a code and without an error. Only
    /// a producer that runs out of frames reaches this; the real device loop
    /// reports [`CameraRefusal::Lost`] instead, because a camera that stops
    /// mid-stream is a fault rather than an ending.
    Ended,
}

impl std::fmt::Debug for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Verdict::Read(text) => write!(f, "Read({} chars not shown)", text.len()),
            Verdict::Refused(why) => write!(f, "Refused({why:?})"),
            Verdict::Ended => write!(f, "Ended"),
        }
    }
}

// ---------------------------------------------------------------------------
// The session
// ---------------------------------------------------------------------------

/// The newest frame and the verdict, shared between the capture thread and
/// the UI thread.
///
/// **A slot and not a channel, and the difference is the whole point.** A
/// preview wants the NEWEST frame; a channel hands over the OLDEST, and a
/// bounded one whose consumer is a frame behind starts refusing exactly the
/// frames the user is waiting to see. Writing into a slot replaces what was
/// there, which is both the right picture and the right lifetime: the
/// [`Zeroizing`] that is displaced wipes as it is dropped, under the lock.
#[derive(Default)]
struct Slot {
    frame: Option<Frame>,
    verdict: Option<Verdict>,
}

struct Shared {
    /// Set by [`Session`]'s `Drop`, by [`Sink::finish`], and by nothing else.
    /// The capture loop's only exit condition that does not come from the
    /// device.
    stop: AtomicBool,
    slot: Mutex<Slot>,
}

fn locked(slot: &Mutex<Slot>) -> MutexGuard<'_, Slot> {
    slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The producer's end of a [`Session`] -- what a capture loop is handed.
///
/// It is `Send` and holds no borrow, so it can cross onto the spawned thread;
/// it is the only thing that can, which is why the loop cannot reach anything
/// of the UI's.
pub struct Sink {
    shared: Arc<Shared>,
}

impl Sink {
    /// **Whether anybody still wants pictures.** The capture loop's exit
    /// condition, checked before every read rather than after, so a session
    /// stopped while a read was in flight releases the device on the next turn
    /// of the loop instead of one frame later.
    pub fn wanted(&self) -> bool {
        !self.shared.stop.load(Ordering::Acquire)
    }

    /// Offers a frame to the preview, replacing whatever was pending.
    ///
    /// The displaced frame is dropped **while the lock is held**, which is
    /// what wipes it. Nothing accumulates: the slot holds at most one frame
    /// however far behind the UI thread falls.
    pub fn show(&self, frame: Frame) {
        locked(&self.shared.slot).frame = Some(frame);
    }

    /// Records a verdict and stops the session.
    ///
    /// Recording is once-only. A capture loop that reports a refusal and then
    /// falls out of its loop and reports an ending would otherwise overwrite
    /// the reason with "it ended", which is the least useful of the two.
    fn finish(&self, verdict: Verdict) {
        {
            let mut held = locked(&self.shared.slot);
            if held.verdict.is_none() {
                held.verdict = Some(verdict);
            }
            // **The pending preview frame is dropped here, whatever the
            // verdict.** A finished session must not leave a picture of a seed
            // sitting in a slot waiting for a UI thread that has already moved
            // on to the confirmation.
            held.frame = None;
        }
        self.shared.stop.store(true, Ordering::Release);
    }

    /// A code was read. Ends the session with it.
    pub fn read(&self, text: Zeroizing<String>) {
        self.finish(Verdict::Read(text));
    }

    /// The camera refused. Ends the session with the reason.
    pub fn refuse(&self, why: CameraRefusal) {
        self.finish(Verdict::Refused(why));
    }

    /// The stream ran out. Ends the session with nothing to report.
    pub fn ended(&self) {
        self.finish(Verdict::Ended);
    }
}

/// What one [`Session::take`] handed back.
///
/// Both halves can be present at once: the frame that was pending when the
/// verdict landed is still worth painting for the frame it takes the surface
/// to change stage.
#[derive(Debug, Default)]
pub struct Taken {
    pub frame: Option<Frame>,
    pub verdict: Option<Verdict>,
}

/// **A running camera.** Holding one is what keeps a device open; dropping one
/// is what closes it.
///
/// Not `Clone`. A second handle would be a second thing whose drop the
/// device's lifetime depended on, and the release story above rests on there
/// being exactly one.
pub struct Session {
    shared: Arc<Shared>,
    /// When the producer was spawned, for [`Self::stalled`].
    started: Instant,
    /// Whether any frame has ever been taken out of this session. A camera
    /// that opens and never sends anything is the case the grace period below
    /// exists for, and it is distinguishable from a slow one only by this.
    seen_frame: bool,
}

impl Session {
    /// Spawns `run` on a thread of its own and hands back the session it
    /// fills.
    ///
    /// `run` is an ordinary closure, which is what makes every state of this
    /// surface reachable from a test: production passes the Media Foundation
    /// loop, and a test passes three lines that hand over a buffer it built
    /// arithmetically.
    ///
    /// **The thread is detached.** See the module docs: joining it would put
    /// the UI thread's liveness in a camera driver's hands.
    ///
    /// **Named `start` and not `spawn`, deliberately.**
    /// `job_object`'s child-start guard reads every `::spawn` in this crate
    /// and excuses only `std::thread::spawn` handed a closure, against a
    /// per-file budget. A method of this crate's own called `spawn` is
    /// indistinguishable from `Command::spawn` to a scanner that reads text
    /// -- so every call site would spend budget the file does not have, and
    /// raising the budget to make room would blunt the guard for the one
    /// line here that really does start a thread. The name moves; the rule
    /// does not.
    pub fn start(run: impl FnOnce(Sink) + Send + 'static) -> Session {
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            slot: Mutex::new(Slot::default()),
        });
        let sink = Sink {
            shared: Arc::clone(&shared),
        };
        std::thread::spawn(move || run(sink));
        Session {
            shared,
            started: Instant::now(),
            seen_frame: false,
        }
    }

    /// Starts `seams`' capture loop against one device.
    ///
    /// The decoder travels with it, because the decode happens on the
    /// capture thread rather than on the caller's -- see [`offer`].
    pub fn open(seams: &WebcamSeams, device: Device) -> Session {
        let pump = seams.pump;
        let decode = seams.decode;
        Session::start(move |sink| pump(device, decode, sink))
    }

    /// **Takes whatever the capture thread has put down.** Never blocks on the
    /// device, never blocks on a frame: it locks a mutex that is held only for
    /// the length of a `mem::take` and returns.
    pub fn take(&mut self) -> Taken {
        let taken = std::mem::take(&mut *locked(&self.shared.slot));
        let out = Taken {
            frame: taken.frame,
            verdict: taken.verdict,
        };
        if out.frame.is_some() {
            self.seen_frame = true;
        }
        out
    }

    /// Whether the device has been open for `grace` without ever producing a
    /// picture.
    ///
    /// A separate question from any refusal the device raises, because the
    /// case it catches raises none: a camera whose lens is covered, or whose
    /// driver has accepted the open and then gone quiet, reports success at
    /// every step and simply never sends a frame. Without this the surface
    /// would say "starting the camera" forever.
    ///
    /// `now` is an argument rather than an `Instant::now()` inside, which is
    /// what lets a test drive the boundary exactly instead of sleeping through
    /// it.
    pub fn stalled(&self, now: Instant, grace: Duration) -> bool {
        !self.seen_frame && now.saturating_duration_since(self.started) >= grace
    }

    /// Whether a frame has ever come out of this session.
    pub fn seen_frame(&self) -> bool {
        self.seen_frame
    }

    /// Whether the capture loop has been told to stop. Answers `true` once a
    /// verdict has been recorded, so it is also "this session is over".
    pub fn stopped(&self) -> bool {
        self.shared.stop.load(Ordering::Acquire)
    }
}

impl Drop for Session {
    /// **The release.** Every way this surface can end goes through here,
    /// because every one of them drops the value that owns the session. See
    /// the module docs for why this sets a flag rather than joining.
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
    }
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Session(stopped: {})", self.stopped())
    }
}

// ---------------------------------------------------------------------------
// How often a decode runs
// ---------------------------------------------------------------------------

/// How often a frame is put through the decoder, at most.
///
/// The same 150ms `region_overlay::DECODE_INTERVAL` takes, and for the same
/// arithmetic: six or seven attempts a second is faster than a hand can aim a
/// phone, and a decode of every frame at thirty frames a second would spend
/// most of a core answering a question that has not changed.
///
/// This one bounds work on the **capture thread**, not the UI thread, which is
/// the other half of why a slow decode here cannot be felt as a stutter: the
/// worst it can do is drop a camera frame, and a dropped frame at thirty a
/// second is invisible.
pub const DECODE_INTERVAL: Duration = Duration::from_millis(150);

/// How long a camera has to produce its first frame before
/// [`Session::stalled`] gives up on it.
///
/// Generous on purpose. A USB camera that has been asleep can take two or
/// three seconds to wake, and a route that gave up at one would refuse working
/// hardware; the cost of waiting is a line on screen that says it is starting.
pub const FIRST_FRAME_GRACE: Duration = Duration::from_secs(8);

/// Bounds how often a decode is attempted.
///
/// **Time alone, unlike `region_overlay::DecodeThrottle`'s two gates**, and
/// the difference is a real one rather than a simplification. That one also
/// requires the *rectangle* to have changed, because a pointer held still
/// produces frames that are identical and re-decoding them cannot change the
/// answer. Every camera frame is a different picture -- the sensor's noise
/// alone guarantees it -- so there is no equivalent second gate here, and one
/// written over the pixels would be a full-frame comparison costing more than
/// the decode it saved.
#[derive(Debug, Clone)]
pub struct DecodeCadence {
    interval: Duration,
    last: Option<Instant>,
}

impl DecodeCadence {
    /// A cadence with the given minimum spacing. [`DECODE_INTERVAL`] is what
    /// production passes.
    pub fn new(interval: Duration) -> Self {
        DecodeCadence {
            interval,
            last: None,
        }
    }

    /// Whether to attempt a decode at `now`, recording the attempt if so.
    ///
    /// **The first call always answers `true`**: a camera that has just
    /// produced its first frame should be read immediately, because the common
    /// case is a user who was already holding the code up when the preview
    /// opened.
    pub fn should_attempt(&mut self, now: Instant) -> bool {
        if let Some(last) = self.last {
            if now.saturating_duration_since(last) < self.interval {
                return false;
            }
        }
        self.last = Some(now);
        true
    }

    /// The spacing this cadence was built with. Read by the test that pins
    /// production's value; not used by the loop itself.
    pub fn interval(&self) -> Duration {
        self.interval
    }
}

// ---------------------------------------------------------------------------
// The seam
// ---------------------------------------------------------------------------

/// What a decoder looks like from here. Spelled out so the seam below and the
/// capture loop's parameter cannot drift apart.
pub type DecodeFn = fn(&[u8], usize, usize) -> Option<Zeroizing<String>>;

/// What a capture loop looks like from here: it is handed a device, a decoder
/// and a [`Sink`], and it returns when it is done.
pub type PumpFn = fn(Device, DecodeFn, Sink);

/// The three calls this route makes into something it does not own.
///
/// [`crate::vault_window::totp_add::ImageSeams`]'s shape and its reason: the
/// two that talk to Media Foundation cannot run on a machine with no camera,
/// and no test in this crate may open a capture device. Behind a seam every
/// state of this route is reachable from a test that builds its pixels
/// arithmetically.
#[derive(Clone, Copy)]
pub struct WebcamSeams {
    /// [`devices`] in production.
    pub devices: fn() -> Result<Vec<Device>, CameraRefusal>,
    /// [`pump`] in production.
    pub pump: PumpFn,
    /// [`crate::qr::decode_qr`] in production.
    pub decode: DecodeFn,
}

impl WebcamSeams {
    /// The real ones. A test asserts all three are the real functions **by
    /// address**, so a seam quietly re-pointed at a stub fails rather than
    /// passing.
    pub fn production() -> Self {
        WebcamSeams {
            devices,
            pump,
            decode: crate::qr::decode_qr,
        }
    }
}

/// **One captured frame, offered to the preview and to the decoder.**
///
/// The body every capture loop runs per frame, extracted so that the real one
/// and a test's three-line one cannot come to treat a frame differently.
/// Answers whether the loop should keep going.
///
/// **The decode happens BEFORE the preview, and the frame that carries the
/// code is never shown.** Not an optimisation: it means the one frame this
/// route is certain contains a readable seed is the one frame that never
/// reaches an `egui` texture, which is the copy this module cannot wipe.
pub fn offer(
    sink: &Sink,
    cadence: &mut DecodeCadence,
    decode: DecodeFn,
    frame: Frame,
    now: Instant,
) -> bool {
    if cadence.should_attempt(now) {
        if let Some(text) = decode(frame.pixels(), frame.width(), frame.height()) {
            sink.read(text);
            return false;
        }
    }
    sink.show(frame);
    sink.wanted()
}

// ---------------------------------------------------------------------------
// Pixels
// ---------------------------------------------------------------------------

/// The largest frame this route asks a camera for, on the longer edge.
///
/// A cap and not a preference. Cameras offer resolutions up to 4K, a QR code
/// held at arm's length is legible at a fraction of that, and the cost of the
/// difference is paid twice -- once converting every frame and once decoding
/// it. 1280 is comfortably above what any phone screen held in front of a
/// webcam needs.
pub const MAX_FRAME_SIDE: u32 = 1280;

/// **Media Foundation's `RGB32` to the layout [`crate::qr::decode_qr`] reads.**
///
/// Two things are being undone here and both of them fail silently if they are
/// got wrong, which is why this is a pure function with tests rather than four
/// lines inside an `unsafe` block:
///
/// * **Channel order.** `MFVideoFormat_RGB32` is B, G, R, X in memory. A
///   decoder handed it unswapped still sees a perfectly good greyscale image
///   -- `qr::luma` weights the channels differently, so a swapped buffer is
///   merely a slightly differently-contrasted picture, and it *often decodes*.
///   The bug that would leave is a route that works in good light and refuses
///   in poor, which is indistinguishable from a bad camera.
/// * **Row order.** A `stride` may be negative, which is Windows saying the
///   image is stored bottom-up: the first bytes of the buffer are the LAST row
///   of the picture. A QR code is not symmetric under a vertical flip, so this
///   one fails loudly -- nothing decodes, ever -- which is the better of the
///   two failures and still worth a test.
///
/// The alpha channel is forced opaque rather than carried: the `X` in `RGB32`
/// is undefined, some drivers leave it zero, and `egui`'s preview would paint
/// a fully transparent picture. `decode_qr` ignores alpha for its own reasons
/// and would not have noticed.
///
/// `None` when the buffer is shorter than `stride` and `height` claim.
pub fn pack_rgba(
    src: &[u8],
    stride: i32,
    width: usize,
    height: usize,
) -> Option<Zeroizing<Vec<u8>>> {
    if width == 0 || height == 0 {
        return None;
    }
    let row_bytes = usize::try_from(stride.unsigned_abs()).ok()?;
    if row_bytes < width.checked_mul(4)? {
        return None;
    }
    if row_bytes.checked_mul(height)? > src.len() {
        return None;
    }
    let mut out = Zeroizing::new(Vec::with_capacity(width * height * 4));
    for y in 0..height {
        // A negative stride means the buffer's first row is the picture's
        // last. The row that belongs at output line `y` is therefore counted
        // from the far end.
        let source_row = if stride < 0 { height - 1 - y } else { y };
        let at = source_row * row_bytes;
        for px in src[at..at + width * 4].chunks_exact(4) {
            out.extend_from_slice(&[px[2], px[1], px[0], 0xff]);
        }
    }
    Some(out)
}

/// Picks the capture resolution to ask a device for, out of what it offers.
///
/// **The largest that fits under [`MAX_FRAME_SIDE`]**, because a QR code
/// occupies a small part of the frame and every pixel of it counts; and the
/// *smallest* offered when nothing fits, because a 4K-only camera asked for
/// its one mode is better than a route that refuses it.
///
/// A pure function over the list so the choice is a thing a test can
/// enumerate, rather than a comparison buried in an `unsafe` loop.
pub fn best_frame_size(offered: &[(u32, u32)]) -> Option<(u32, u32)> {
    let fits = |&(w, h): &(u32, u32)| w <= MAX_FRAME_SIDE && h <= MAX_FRAME_SIDE;
    let area = |&(w, h): &(u32, u32)| u64::from(w) * u64::from(h);
    offered
        .iter()
        .filter(|size| fits(size) && area(size) > 0)
        .max_by_key(|size| area(size))
        .or_else(|| offered.iter().filter(|size| area(size) > 0).min_by_key(|size| area(size)))
        .copied()
}

// ---------------------------------------------------------------------------
// Media Foundation
// ---------------------------------------------------------------------------

/// COM on this thread, initialised if it was not already, and balanced.
///
/// Media Foundation needs an initialised apartment. This process already
/// initialises COM on the UI thread -- the shell file dialog and the tray both
/// need it -- so `CoInitializeEx` here will often answer `S_FALSE` ("already
/// in, count incremented", which still has to be balanced) or
/// `RPC_E_CHANGED_MODE` ("already in, and in the OTHER apartment model, so
/// nothing was incremented"). The third of those is the one that must NOT be
/// balanced, and getting it wrong would decrement the UI thread's own COM
/// reference out from under the file dialog.
struct Com {
    owned: bool,
}

impl Com {
    fn enter() -> Com {
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
        // Multi-threaded, because everything this module does with COM is on a
        // thread of its own that pumps no messages; an apartment-threaded one
        // there would deadlock the first time Media Foundation marshalled a
        // call back.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Com { owned: hr.is_ok() }
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.owned {
            unsafe { windows::Win32::System::Com::CoUninitialize() };
        }
    }
}

/// **An open capture device, shut down when this drops.**
///
/// Releasing the COM reference is not the same as releasing the CAMERA:
/// `IMFMediaSource::Shutdown` is what tells Media Foundation to let the
/// hardware go, and a source created with `MFCreateDeviceSource` is the
/// APPLICATION's to shut down -- the source reader built on top of it will
/// not do it. Without this the light can stay on until the last stray
/// reference somewhere in the platform is dropped, which is exactly the
/// "a camera light left on after the user has moved on" case this module's
/// docs are about.
///
/// A guard rather than a call at the end of [`stream`] because every
/// refusal in that function is a `?`, and a shutdown written after the last
/// one runs on the happy path only.
struct Opened(windows::Win32::Media::MediaFoundation::IMFMediaSource);

impl Drop for Opened {
    fn drop(&mut self) {
        unsafe {
            let _ = self.0.Shutdown();
        }
    }
}

/// Media Foundation started, and shut down again when this drops.
///
/// `MFSTARTUP_LITE` rather than `MFSTARTUP_FULL`, and the difference is worth
/// naming on a privacy page: the full form initialises Media Foundation's
/// **network** sources. This route reads a camera and has no business opening
/// a socket, so it asks for the platform without them.
struct Platform;

impl Platform {
    fn start() -> Result<Platform, CameraRefusal> {
        use windows::Win32::Media::MediaFoundation::{MFStartup, MFSTARTUP_LITE, MF_VERSION};
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE) }.map_err(refusal_for)?;
        Ok(Platform)
    }
}

impl Drop for Platform {
    fn drop(&mut self) {
        unsafe { let _ = windows::Win32::Media::MediaFoundation::MFShutdown(); };
    }
}

/// **An `HRESULT` to one of this module's sentences.**
///
/// The one place an OS error code is looked at, and nothing downstream of it
/// carries the number. Three codes are modelled by name because the user can
/// act on all three differently; everything else is
/// [`CameraRefusal::Unavailable`], which says what to do rather than
/// pretending to a diagnosis.
fn refusal_for(error: windows::core::Error) -> CameraRefusal {
    use windows::Win32::Foundation::{E_ACCESSDENIED, ERROR_BUSY, ERROR_SHARING_VIOLATION};
    use windows::Win32::Media::MediaFoundation::{
        MF_E_HW_MFT_FAILED_START_STREAMING, MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED,
    };

    let code = error.code();
    if code == E_ACCESSDENIED {
        // What the Settings privacy switch and a group policy both produce.
        return CameraRefusal::Denied;
    }
    if code == MF_E_HW_MFT_FAILED_START_STREAMING
        || code == ERROR_BUSY.to_hresult()
        || code == ERROR_SHARING_VIOLATION.to_hresult()
    {
        return CameraRefusal::Busy;
    }
    if code == MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED {
        return CameraRefusal::Lost;
    }
    CameraRefusal::Unavailable
}

/// **Every video capture device Windows knows about.**
///
/// The one call in this module the UI thread makes; see the module docs for
/// why that is acceptable here and not for anything downstream of it.
///
/// An empty list is [`CameraRefusal::NoCamera`] rather than an empty `Ok`, so
/// the caller has one thing to render and not two.
pub fn devices() -> Result<Vec<Device>, CameraRefusal> {
    use windows::Win32::Media::MediaFoundation::{
        IMFActivate, MFCreateAttributes, MFEnumDeviceSources,
        MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
        MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
        MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
    };
    use windows::Win32::System::Com::CoTaskMemFree;

    let _com = Com::enter();
    let _mf = Platform::start()?;

    unsafe {
        let mut attributes = None;
        MFCreateAttributes(&mut attributes, 1).map_err(refusal_for)?;
        let attributes = attributes.ok_or(CameraRefusal::Unavailable)?;
        attributes
            .SetGUID(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
            )
            .map_err(refusal_for)?;

        let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        MFEnumDeviceSources(&attributes, &mut list, &mut count).map_err(refusal_for)?;
        if list.is_null() {
            return Err(CameraRefusal::NoCamera);
        }

        let mut found = Vec::new();
        {
            // Every `IMFActivate` in the array is an owned reference. Taking
            // each out of its `Option` here is what releases it at the end of
            // this block; the ARRAY itself is a separate allocation and is
            // freed below, and leaking either would hold the device's
            // enumeration entry open.
            let entries = std::slice::from_raw_parts_mut(list, count as usize);
            for entry in entries.iter_mut() {
                let Some(activate) = entry.take() else {
                    continue;
                };
                let Some(id) = allocated_string(
                    &activate,
                    &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
                ) else {
                    // A device with no symbolic link cannot be re-opened on
                    // the capture thread, so offering it would be offering a
                    // row that fails when pressed.
                    continue;
                };
                let name = allocated_string(&activate, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME)
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| UNNAMED_CAMERA.to_string());
                found.push(Device { name, id });
            }
        }
        CoTaskMemFree(Some(list as *const core::ffi::c_void));

        if found.is_empty() {
            return Err(CameraRefusal::NoCamera);
        }
        Ok(found)
    }
}

/// What a camera that does not tell Windows its name is called in the picker.
///
/// It has to be called something: a row with an empty face is a row the user
/// cannot tell from a rendering bug.
pub const UNNAMED_CAMERA: &str = "Camera";

/// One `IMFAttributes` string, copied out and the OS allocation freed.
///
/// # Safety
///
/// `key` must be a valid attribute GUID. The `PWSTR` Media Foundation hands
/// back is a `CoTaskMemAlloc` allocation and is freed here whether or not the
/// conversion to a `String` succeeded -- an early return past the free is how
/// this leaks once per enumeration.
unsafe fn allocated_string(
    attributes: &windows::Win32::Media::MediaFoundation::IMFActivate,
    key: &windows::core::GUID,
) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::System::Com::CoTaskMemFree;

    let mut text = PWSTR::null();
    let mut length = 0u32;
    let got = attributes.GetAllocatedString(key, &mut text, &mut length);
    if got.is_err() || text.is_null() {
        return None;
    }
    let out = text.to_string().ok();
    CoTaskMemFree(Some(text.0 as *const core::ffi::c_void));
    out
}

/// **The capture loop.** Opens `device`, converts every frame, and runs until
/// a code is read, the device fails, or the session is stopped.
///
/// Runs on the thread [`Session::start`] made for it and touches nothing of
/// the UI's: the only thing it can reach is the [`Sink`] it was handed.
///
/// It reports through `sink` and returns nothing, so there is no answer a
/// caller could be tempted to wait for.
pub fn pump(device: Device, decode: DecodeFn, sink: Sink) {
    let _com = Com::enter();
    let platform = match Platform::start() {
        Ok(platform) => platform,
        Err(why) => {
            sink.refuse(why);
            return;
        }
    };
    match stream(&device, decode, &sink) {
        Ok(()) => sink.ended(),
        Err(why) => sink.refuse(why),
    }
    // Named rather than `_`, so the shutdown is visibly ordered after the
    // stream rather than at some point the compiler chose.
    drop(platform);
}

/// [`pump`]'s body, with the refusals as a `Result` so every failure path is
/// one `?` rather than a nest.
fn stream(device: &Device, decode: DecodeFn, sink: &Sink) -> Result<(), CameraRefusal> {
    use windows::core::HSTRING;
    use windows::Win32::Media::MediaFoundation::{
        MFCreateAttributes, MFCreateDeviceSource, MFCreateMediaType,
        MFCreateSourceReaderFromMediaSource, MFMediaType_Video, MFVideoFormat_RGB32,
        MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
        MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK, MF_MT_DEFAULT_STRIDE,
        MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
        MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
        MF_SOURCE_READERF_ENDOFSTREAM,
    };

    let stream_index = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

    unsafe {
        // **Re-opened from the symbolic link, not from an `IMFActivate` handed
        // across the thread boundary.** See [`Device::id`].
        let mut source_attributes = None;
        MFCreateAttributes(&mut source_attributes, 2).map_err(refusal_for)?;
        let source_attributes = source_attributes.ok_or(CameraRefusal::Unavailable)?;
        source_attributes
            .SetGUID(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
            )
            .map_err(refusal_for)?;
        source_attributes
            .SetString(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
                &HSTRING::from(device.id.as_str()),
            )
            .map_err(refusal_for)?;
        // Wrapped as it is created, so there is no window in which the
        // device is open and nothing is on the hook for closing it.
        let source: Opened =
            Opened(MFCreateDeviceSource(&source_attributes).map_err(refusal_for)?);

        // **The reader is asked to do the colour conversion**, which is what
        // makes this file's pixel handling one function rather than a decoder
        // per camera. Without this flag a device offering only NV12 or MJPG --
        // which is most of them -- would refuse the RGB32 output type below,
        // and the alternative is writing a YUV converter and a JPEG decoder
        // for a route whose whole job is to read a black-and-white square.
        let mut reader_attributes = None;
        MFCreateAttributes(&mut reader_attributes, 1).map_err(refusal_for)?;
        let reader_attributes = reader_attributes.ok_or(CameraRefusal::Unavailable)?;
        reader_attributes
            .SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)
            .map_err(refusal_for)?;

        // Declared AFTER the source, so it drops BEFORE it: the reader must
        // be gone before the source it was built on is shut down.
        let reader = MFCreateSourceReaderFromMediaSource(&source.0, &reader_attributes)
            .map_err(refusal_for)?;

        // What the device offers, so [`best_frame_size`] can choose. The loop
        // ends when `GetNativeMediaType` refuses an index, which is Media
        // Foundation's own way of saying "that was the last one".
        let mut offered = Vec::new();
        for index in 0.. {
            let Ok(native) = reader.GetNativeMediaType(stream_index, index) else {
                break;
            };
            if let Ok(packed) = native.GetUINT64(&MF_MT_FRAME_SIZE) {
                offered.push(((packed >> 32) as u32, packed as u32));
            }
        }
        if let Some(wanted) = best_frame_size(&offered) {
            // Set the NATIVE type first: this is what tells the device which
            // of its modes to run in. Asking only for an output size would
            // leave the camera streaming 4K and make the reader scale every
            // frame down, which is the same picture for several times the
            // work. A device that refuses its own advertised mode is not fatal
            // -- the output type below still applies -- so this is attempted
            // rather than required.
            for index in 0.. {
                let Ok(native) = reader.GetNativeMediaType(stream_index, index) else {
                    break;
                };
                let matches = native
                    .GetUINT64(&MF_MT_FRAME_SIZE)
                    .map(|packed| ((packed >> 32) as u32, packed as u32) == wanted)
                    .unwrap_or(false);
                if matches && reader.SetCurrentMediaType(stream_index, None, &native).is_ok() {
                    break;
                }
            }
        }

        let output = MFCreateMediaType().map_err(refusal_for)?;
        output
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(refusal_for)?;
        output
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
            .map_err(refusal_for)?;
        reader
            .SetCurrentMediaType(stream_index, None, &output)
            .map_err(refusal_for)?;

        // Read BACK what the reader settled on rather than trusting what was
        // asked for: a device may deliver a different size than the mode that
        // was selected, and a buffer walked at the wrong width is the silent
        // failure `pack_rgba`'s doc describes.
        let settled = reader
            .GetCurrentMediaType(stream_index)
            .map_err(refusal_for)?;
        let packed = settled.GetUINT64(&MF_MT_FRAME_SIZE).map_err(refusal_for)?;
        let (width, height) = ((packed >> 32) as usize, packed as usize);
        if width == 0 || height == 0 {
            return Err(CameraRefusal::Unavailable);
        }
        // The stride is stored as a `UINT32` and read as a signed number,
        // which is Media Foundation's own convention: the sign is the row
        // order. Absent, it is a packed top-down frame.
        let stride = settled
            .GetUINT32(&MF_MT_DEFAULT_STRIDE)
            .map(|raw| raw as i32)
            .unwrap_or((width * 4) as i32);

        let mut cadence = DecodeCadence::new(DECODE_INTERVAL);
        // **Checked before every read**, so a stop that lands while a read is
        // in flight costs one frame rather than two.
        while sink.wanted() {
            let mut flags = 0u32;
            let mut sample = None;
            reader
                .ReadSample(
                    stream_index,
                    0,
                    None,
                    Some(&mut flags),
                    None,
                    Some(&mut sample),
                )
                .map_err(refusal_for)?;
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                // The device said it is done. Not an ending this route has a
                // use for: a camera that stops mid-scan has been unplugged or
                // taken, and the user needs to be told which.
                return Err(CameraRefusal::Lost);
            }
            let Some(sample) = sample else {
                // A gap. `ReadSample` answers with no sample when the device
                // has nothing yet, and that is ordinary rather than an error.
                continue;
            };
            let buffer = sample.ConvertToContiguousBuffer().map_err(refusal_for)?;
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut current = 0u32;
            buffer
                .Lock(&mut data, None, Some(&mut current))
                .map_err(refusal_for)?;
            // The conversion happens with the buffer locked and the result is
            // this module's own allocation, so the device's memory is unlocked
            // before anything slow (the decode) touches the pixels.
            let packed = (!data.is_null())
                .then(|| {
                    pack_rgba(
                        std::slice::from_raw_parts(data, current as usize),
                        stride,
                        width,
                        height,
                    )
                })
                .flatten();
            let _ = buffer.Unlock();

            let Some(rgba) = packed else {
                // A frame that does not match what the media type promised.
                // Skipped rather than fatal: one short buffer from a driver
                // waking up is not a reason to close the camera.
                continue;
            };
            let Some(frame) = Frame::new(width, height, rgba) else {
                continue;
            };
            if !offer(sink, &mut cadence, decode, frame, Instant::now()) {
                return Ok(());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // Pixels
    // -----------------------------------------------------------------

    /// A `width x height` BGRX buffer whose every pixel is distinguishable,
    /// laid out top-down at `row_bytes` per row.
    fn bgrx(width: usize, height: usize, row_bytes: usize) -> Vec<u8> {
        let mut out = vec![0u8; row_bytes * height];
        for y in 0..height {
            for x in 0..width {
                let at = y * row_bytes + x * 4;
                out[at] = (10 + y) as u8; // blue
                out[at + 1] = (50 + x) as u8; // green
                out[at + 2] = (90 + x + y) as u8; // red
                out[at + 3] = 0x00; // the undefined X
            }
        }
        out
    }

    /// **BGRX becomes RGBA, and the alpha is forced opaque.**
    ///
    /// The swap is the failure that would not show up as a failure -- see
    /// [`pack_rgba`]'s own doc -- so it is asserted channel by channel rather
    /// than by whether something decoded.
    #[test]
    fn a_frame_arrives_as_rgba_with_the_channels_the_right_way_round() {
        let src = bgrx(3, 2, 12);
        let out = pack_rgba(&src, 12, 3, 2).expect("a well-formed buffer");
        assert_eq!(out.len(), 3 * 2 * 4);
        for y in 0..2usize {
            for x in 0..3usize {
                let at = (y * 3 + x) * 4;
                assert_eq!(out[at], (90 + x + y) as u8, "red at {x},{y}");
                assert_eq!(out[at + 1], (50 + x) as u8, "green at {x},{y}");
                assert_eq!(out[at + 2], (10 + y) as u8, "blue at {x},{y}");
                assert_eq!(out[at + 3], 0xff, "alpha at {x},{y} is not opaque");
            }
        }
    }

    /// **A negative stride is a bottom-up picture and is flipped back.**
    ///
    /// The control is the same bytes at a positive stride: without it this
    /// test would pass against a function that flipped both.
    #[test]
    fn a_bottom_up_frame_is_turned_the_right_way_up() {
        let src = bgrx(2, 3, 8);
        let up = pack_rgba(&src, 8, 2, 3).expect("top-down");
        let down = pack_rgba(&src, -8, 2, 3).expect("bottom-up");
        assert_ne!(&*up, &*down, "the sign of the stride changed nothing");
        // Row 0 of the bottom-up read is row 2 of the top-down one, and so on.
        let row = |buf: &[u8], y: usize| buf[y * 2 * 4..(y + 1) * 2 * 4].to_vec();
        assert_eq!(row(&down, 0), row(&up, 2));
        assert_eq!(row(&down, 1), row(&up, 1));
        assert_eq!(row(&down, 2), row(&up, 0));
    }

    /// **Padding between rows is skipped rather than walked into the picture.**
    ///
    /// A camera's stride is very often wider than its width; a reader that
    /// ignored that would shear every frame by a few pixels per row, and a
    /// sheared QR code decodes as nothing at all.
    #[test]
    fn a_row_wider_than_the_picture_is_read_at_its_stride() {
        // 3 pixels of picture in rows of 5 pixels' worth of bytes.
        let src = bgrx(3, 2, 20);
        let padded = pack_rgba(&src, 20, 3, 2).expect("a padded buffer");
        let tight = pack_rgba(&bgrx(3, 2, 12), 12, 3, 2).expect("a tight buffer");
        assert_eq!(&*padded, &*tight, "the padding leaked into the picture");
    }

    /// A short, empty or absurd buffer is `None` rather than a panic. This is
    /// handed a driver's output.
    #[test]
    fn a_malformed_frame_buffer_is_refused_rather_than_panicking() {
        assert!(pack_rgba(&[], 0, 0, 0).is_none());
        assert!(pack_rgba(&[], 8, 2, 3).is_none());
        // One byte short of `stride * height`.
        let short = vec![0u8; 8 * 3 - 1];
        assert!(pack_rgba(&short, 8, 2, 3).is_none());
        // A stride narrower than the picture claims to be.
        let src = bgrx(3, 2, 12);
        assert!(pack_rgba(&src, 8, 3, 2).is_none());
        // Control: exactly `stride * height` bytes is accepted, so the length
        // check above is off by nothing.
        let exact = bgrx(2, 3, 8);
        assert_eq!(exact.len(), 8 * 3);
        assert!(pack_rgba(&exact, 8, 2, 3).is_some());
    }

    /// **A frame shorter than its dimensions is refused by [`Frame::new`]**,
    /// so no `Frame` in this crate can be walked past its buffer.
    #[test]
    fn a_frame_cannot_be_built_smaller_than_it_claims() {
        assert!(Frame::new(2, 2, Zeroizing::new(vec![0u8; 2 * 2 * 4 - 1])).is_none());
        assert!(Frame::new(0, 2, Zeroizing::new(vec![0u8; 64])).is_none());
        assert!(Frame::new(2, 0, Zeroizing::new(vec![0u8; 64])).is_none());
        assert!(Frame::new(usize::MAX, usize::MAX, Zeroizing::new(vec![0u8; 64])).is_none());
        let ok = Frame::new(2, 2, Zeroizing::new(vec![7u8; 2 * 2 * 4])).expect("the control");
        assert_eq!((ok.width(), ok.height()), (2, 2));
        assert_eq!(ok.pixels().len(), 16);
    }

    /// **Neither `Debug` prints what it holds.** `debug_leak_guard` requires
    /// the impls to be hand-written; this says the hand-written ones are also
    /// right.
    #[test]
    fn the_debug_output_of_a_frame_and_a_verdict_carries_no_secret() {
        let frame = Frame::new(2, 2, Zeroizing::new(vec![0xabu8; 16])).expect("a frame");
        let printed = format!("{frame:?}");
        assert!(printed.contains("2x2"), "the size is useful and should be there: {printed}");
        assert!(!printed.contains("171") && !printed.contains("ab"), "{printed}");

        let verdict = Verdict::Read(Zeroizing::new(
            "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP".to_string(),
        ));
        let printed = format!("{verdict:?}");
        assert!(!printed.contains("JBSWY3DPEHPK3PXP"), "the seed is printed: {printed}");
        assert!(printed.contains("40"), "the length is what it prints instead: {printed}");
    }

    // -----------------------------------------------------------------
    // Choosing a resolution
    // -----------------------------------------------------------------

    #[test]
    fn the_largest_size_under_the_cap_is_chosen() {
        let offered = [(640, 480), (1280, 720), (1920, 1080), (320, 240)];
        assert_eq!(best_frame_size(&offered), Some((1280, 720)));
    }

    #[test]
    fn a_camera_with_nothing_under_the_cap_gets_its_smallest_mode() {
        // A 4K-only device. Refusing it would be refusing working hardware.
        let offered = [(3840, 2160), (2560, 1440)];
        assert_eq!(best_frame_size(&offered), Some((2560, 1440)));
    }

    #[test]
    fn a_camera_that_offers_nothing_usable_gets_no_choice() {
        assert_eq!(best_frame_size(&[]), None);
        assert_eq!(best_frame_size(&[(0, 0), (640, 0), (0, 480)]), None);
    }

    // -----------------------------------------------------------------
    // The cadence
    // -----------------------------------------------------------------

    /// **The first frame is read immediately, and the next is not.**
    #[test]
    fn a_decode_is_attempted_at_once_and_then_no_faster_than_the_interval() {
        let interval = Duration::from_millis(150);
        let mut cadence = DecodeCadence::new(interval);
        let start = Instant::now();
        assert!(cadence.should_attempt(start), "the first frame was not read");
        assert!(!cadence.should_attempt(start), "the very next frame was read too");
        assert!(
            !cadence.should_attempt(start + interval - Duration::from_millis(1)),
            "a frame one millisecond early was read"
        );
        assert!(
            cadence.should_attempt(start + interval),
            "a frame exactly one interval later was not read"
        );
        // And the clock restarts from the attempt that was made, not from the
        // start: two intervals after the first must not admit two attempts.
        assert!(!cadence.should_attempt(start + interval + Duration::from_millis(1)));
    }

    #[test]
    fn production_reads_a_camera_six_or_seven_times_a_second() {
        assert_eq!(DECODE_INTERVAL, Duration::from_millis(150));
        assert_eq!(DecodeCadence::new(DECODE_INTERVAL).interval(), DECODE_INTERVAL);
    }

    // -----------------------------------------------------------------
    // Sessions, without a camera
    // -----------------------------------------------------------------

    /// A frame whose pixels say which one it is, so a test can tell the newest
    /// from the one before it.
    fn marked(mark: u8) -> Frame {
        Frame::new(2, 2, Zeroizing::new(vec![mark; 2 * 2 * 4])).expect("a frame")
    }

    /// Spins until `check` answers, or fails after a bounded wait.
    ///
    /// **A bounded spin and not a sleep-then-assert**: the producer is a real
    /// thread, so the alternative is either a race or a fixed delay long
    /// enough to be slow on every machine that is not the slowest one.
    fn until(what: &str, mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if check() {
                return;
            }
            std::thread::yield_now();
        }
        panic!("timed out waiting for {what}");
    }

    /// **Frames arrive newest-first, and the older ones do not queue up.**
    ///
    /// The producer signals with a flag and then idles rather than calling
    /// [`Sink::ended`], because ending a session deliberately drops the
    /// pending preview frame -- which is the very thing under test here.
    /// That behaviour has a test of its own below.
    #[test]
    fn the_ui_side_of_a_session_sees_the_newest_frame_and_never_a_backlog() {
        let sent = Arc::new(AtomicBool::new(false));
        let sent_in = Arc::clone(&sent);
        let mut session = Session::start(move |sink| {
            for mark in 1..=3u8 {
                sink.show(marked(mark));
            }
            sent_in.store(true, Ordering::Release);
            while sink.wanted() {
                std::thread::yield_now();
            }
        });
        until("three frames to be offered", || sent.load(Ordering::Acquire));

        let taken = session.take();
        assert_eq!(
            taken.frame.as_ref().map(|f| f.pixels()[0]),
            Some(3),
            "the preview was handed a stale frame"
        );
        assert!(taken.verdict.is_none(), "a running camera reported a verdict");
        // The slot held one frame at a time, so the next take is empty
        // rather than handing over the two that were displaced.
        assert!(session.take().frame.is_none(), "the displaced frames were queued");
    }

    /// **A finished session leaves no picture behind it.**
    ///
    /// The frame that was pending when the verdict landed is dropped rather
    /// than left in the slot: a session that has answered must not leave a
    /// picture of a seed waiting for a UI thread that has moved on.
    #[test]
    fn ending_a_session_drops_the_picture_that_was_waiting() {
        let mut session = Session::start(|sink| {
            sink.show(marked(7));
            sink.refuse(CameraRefusal::Lost);
        });
        let mut taken = Taken::default();
        until("the verdict", || {
            taken = session.take();
            taken.verdict.is_some()
        });
        assert!(taken.frame.is_none(), "a finished session left a picture in the slot");
    }

    /// **A decode landing stops the session and carries the payload out.**
    ///
    /// The decoder is a stub here on purpose: what is under test is the
    /// wiring, and the real decoder is exercised against a real QR code in
    /// `qr.rs`. The production seam is pinned to that decoder by address
    /// below, which is what stops this from being a test of a stub only.
    #[test]
    fn a_code_read_off_a_frame_ends_the_session_with_the_payload() {
        fn always_finds(_: &[u8], _: usize, _: usize) -> Option<Zeroizing<String>> {
            Some(Zeroizing::new("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP".to_string()))
        }
        let mut session = Session::start(move |sink| {
            let mut cadence = DecodeCadence::new(DECODE_INTERVAL);
            let mut offered = 0;
            while sink.wanted() && offered < 100 {
                offered += 1;
                if !offer(&sink, &mut cadence, always_finds, marked(offered as u8), Instant::now())
                {
                    return;
                }
            }
            sink.ended();
        });

        let mut verdict = None;
        until("a code to be read", || {
            verdict = session.take().verdict;
            verdict.is_some()
        });
        match verdict {
            Some(Verdict::Read(text)) => {
                assert_eq!(&*text, "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP");
            }
            other => panic!("the session did not report a code: {other:?}"),
        }
        assert!(session.stopped(), "a session that read a code is still running");
    }

    /// **The frame the code came off is never handed to the preview.**
    ///
    /// The copy this module cannot wipe is the `egui` texture, and this is the
    /// assertion that says the one frame certain to hold a readable seed never
    /// reaches it.
    #[test]
    fn the_frame_a_code_is_read_from_is_not_shown() {
        fn finds_the_second(rgba: &[u8], _: usize, _: usize) -> Option<Zeroizing<String>> {
            (rgba[0] == 2).then(|| Zeroizing::new("otpauth://totp/x?secret=AA".to_string()))
        }
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            slot: Mutex::new(Slot::default()),
        });
        let sink = Sink {
            shared: Arc::clone(&shared),
        };
        let mut cadence = DecodeCadence::new(Duration::ZERO);
        let now = Instant::now();
        assert!(offer(&sink, &mut cadence, finds_the_second, marked(1), now));
        assert_eq!(
            locked(&shared.slot).frame.as_ref().map(|f| f.pixels()[0]),
            Some(1),
            "the frame with no code in it was not shown"
        );
        assert!(!offer(&sink, &mut cadence, finds_the_second, marked(2), now));
        assert!(
            locked(&shared.slot).frame.is_none(),
            "the frame the code was read off was left in the preview slot"
        );
    }

    /// **A refusal from the producer is what the surface sees**, and the first
    /// one wins.
    #[test]
    fn a_refusal_is_reported_once_and_is_not_overwritten_by_the_ending() {
        let mut session = Session::start(|sink| {
            sink.refuse(CameraRefusal::Busy);
            // A loop that then falls out and reports an ending, which is what
            // the real one does. The reason must survive it.
            sink.ended();
        });
        let mut verdict = None;
        until("the refusal", || {
            verdict = session.take().verdict;
            verdict.is_some()
        });
        assert!(matches!(verdict, Some(Verdict::Refused(CameraRefusal::Busy))), "{verdict:?}");
    }

    /// **Dropping the session stops the producer**, which is the release.
    #[test]
    fn dropping_a_session_tells_the_capture_loop_to_let_the_device_go() {
        let ran = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new(AtomicBool::new(false));
        let (ran_in, stopped_in) = (Arc::clone(&ran), Arc::clone(&stopped));
        let session = Session::start(move |sink| {
            ran_in.store(true, Ordering::Release);
            // The real loop's shape: check the flag, do a unit of work, repeat.
            while sink.wanted() {
                std::thread::yield_now();
            }
            stopped_in.store(true, Ordering::Release);
        });
        until("the producer to start", || ran.load(Ordering::Acquire));
        assert!(!stopped.load(Ordering::Acquire), "it stopped before it was asked to");
        drop(session);
        until("the producer to stop", || stopped.load(Ordering::Acquire));
    }

    /// **A camera that opens and says nothing is caught by the grace period.**
    ///
    /// Paired with the control below it, because "stalled" is the answer a
    /// broken check gives to everything.
    #[test]
    fn a_camera_that_never_sends_a_frame_is_noticed_and_a_working_one_is_not() {
        let grace = Duration::from_secs(8);
        let quiet = Session::start(|sink| {
            while sink.wanted() {
                std::thread::yield_now();
            }
        });
        let start = Instant::now();
        assert!(!quiet.stalled(start, grace), "it gave up before the grace period");
        assert!(
            !quiet.stalled(start + grace - Duration::from_millis(1), grace),
            "it gave up one millisecond early"
        );
        assert!(quiet.stalled(start + grace, grace), "it never gave up");

        // The control: a session that HAS produced a frame is never stalled,
        // however long it has been open.
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let mut busy = Session::start(move |sink| {
            if rx.recv().is_ok() {
                sink.show(marked(9));
            }
            while sink.wanted() {
                std::thread::yield_now();
            }
        });
        tx.send(()).unwrap();
        until("a frame", || busy.take().frame.is_some());
        assert!(busy.seen_frame());
        assert!(!busy.stalled(start + grace * 100, grace), "a working camera was given up on");
    }

    /// A producer's own `take` marks the session as having seen a frame, and
    /// nothing else does.
    #[test]
    fn a_session_has_seen_no_frame_until_one_is_taken_out_of_it() {
        let mut session = Session::start(|sink| {
            sink.show(marked(4));
            while sink.wanted() {
                std::thread::yield_now();
            }
        });
        assert!(!session.seen_frame(), "it counted a frame nobody took");
        until("a frame", || session.take().frame.is_some());
        assert!(session.seen_frame());
    }

    // -----------------------------------------------------------------
    // The refusals, as sentences
    // -----------------------------------------------------------------

    /// **Every refusal names its reason and says what to do**, and none of
    /// them apologises or prints a code.
    #[test]
    fn every_camera_refusal_is_a_sentence_the_user_can_act_on() {
        for why in [
            CameraRefusal::NoCamera,
            CameraRefusal::Busy,
            CameraRefusal::Denied,
            CameraRefusal::Silent,
            CameraRefusal::Lost,
            CameraRefusal::Unavailable,
        ] {
            let title = why.title();
            let detail = why.detail();
            assert!(!title.is_empty() && !detail.is_empty(), "{why:?} has no words");
            assert!(detail.ends_with('.'), "{why:?}'s advice is not a sentence: {detail}");
            for banned in ["sorry", "unfortunately", "oops", "error code", "0x", "HRESULT"] {
                assert!(
                    !title.to_lowercase().contains(banned)
                        && !detail.to_lowercase().contains(banned),
                    "{why:?} says {banned:?}, which is not something a user can act on"
                );
            }
        }
        // The six are distinguishable from one another, so no two states of
        // the camera are reported with the same words.
        let titles: std::collections::BTreeSet<&str> = [
            CameraRefusal::NoCamera,
            CameraRefusal::Busy,
            CameraRefusal::Denied,
            CameraRefusal::Silent,
            CameraRefusal::Lost,
            CameraRefusal::Unavailable,
        ]
        .iter()
        .map(CameraRefusal::title)
        .collect();
        assert_eq!(titles.len(), 6, "two refusals share a headline");
    }

    /// The named refusals point at the routes that need no camera, because the
    /// user is one press away from both.
    #[test]
    fn the_no_camera_refusal_points_at_a_route_that_needs_no_camera() {
        assert!(CameraRefusal::NoCamera.detail().contains("scan the code off your screen"));
        assert!(CameraRefusal::Unavailable.detail().contains("by hand"));
    }

    // -----------------------------------------------------------------
    // The seam
    // -----------------------------------------------------------------

    /// **The production seams are the real functions, by address.**
    ///
    /// Without this every test above could be passing against a stub while the
    /// shipping path called something else entirely.
    #[test]
    fn the_webcam_route_runs_the_real_enumeration_decoder_and_capture_loop() {
        let seams = WebcamSeams::production();
        let real_decode: DecodeFn = crate::qr::decode_qr;
        assert!(
            std::ptr::fn_addr_eq(seams.decode, real_decode),
            "the webcam route's production decoder is not `qr::decode_qr`"
        );
        let real_devices: fn() -> Result<Vec<Device>, CameraRefusal> = devices;
        assert!(
            std::ptr::fn_addr_eq(seams.devices, real_devices),
            "the webcam route's production enumeration is not `webcam::devices`"
        );
        let real_pump: PumpFn = pump;
        assert!(
            std::ptr::fn_addr_eq(seams.pump, real_pump),
            "the webcam route's production capture loop is not `webcam::pump`"
        );
    }

    /// **Nothing in this module writes a file, opens a socket or logs a
    /// picture.**
    ///
    /// A source pin rather than a behavioural test, for the reason
    /// `screen_capture` pins the same thing: the claim on the privacy page is
    /// about what this code does not contain, and the only way to check that
    /// is to read it.
    #[test]
    fn this_module_writes_nothing_anywhere() {
        let source = include_str!("webcam.rs");
        let production = source
            .split_once(concat!("#[cfg(", "test)]"))
            .expect("this file has a test module")
            .0;
        for forbidden in [
            "File::create",
            "fs::write",
            "fs::File",
            "TcpStream",
            "ureq::",
            "log::info",
            "log::debug",
            "println!",
        ] {
            assert!(
                !production.contains(forbidden),
                "the capture path contains {forbidden:?}, which the privacy page says it does not"
            );
        }
        // Media Foundation is started without its network sources, which is
        // the other half of the same claim.
        assert!(
            production.contains("MFSTARTUP_LITE"),
            "Media Foundation is no longer started in its network-free mode"
        );
    }
}
