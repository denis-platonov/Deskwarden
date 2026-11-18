//! The design's motion: its easing curves, its entrance keyframes (`dw-up`,
//! `dw-pop`, ...), its loops (`dw-breathe`, `dw-line`, the pulses), and a clock
//! that stops asking for frames once an animation has settled.
//!
//! Keyframes take *eased* progress (0 to 1) and answer a [`Pose`]: how far the
//! thing is from rest. Loops take seconds and repeat on their own period.

use eframe::egui;

/// A CSS `cubic-bezier(x1, y1, x2, y2)` timing function.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CubicBezier {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
}

impl CubicBezier {
    pub const fn new(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        Self { x1, y1, x2, y2 }
    }

    /// The eased value at time `t` (clamped to 0..1). Solves x(u) = t by
    /// bisection, which converges for every curve here because all their x
    /// control points lie in 0..1, so x is monotonic in u.
    pub fn at(&self, t: f32) -> f32 {
        // Exact at the ends, where bisection would leave a residue: an
        // animation that has not started must be exactly at its start.
        if t <= 0.0 {
            return 0.0;
        }
        if t >= 1.0 {
            return 1.0;
        }
        let axis = |a: f32, b: f32, u: f32| {
            let v = 1.0 - u;
            3.0 * v * v * u * a + 3.0 * v * u * u * b + u * u * u
        };
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..24 {
            let mid = (lo + hi) / 2.0;
            if axis(self.x1, self.x2, mid) < t {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        axis(self.y1, self.y2, (lo + hi) / 2.0)
    }
}

/// CSS `ease-out`: every entrance.
pub const EASE_OUT: CubicBezier = CubicBezier::new(0.0, 0.0, 0.58, 1.0);
/// CSS `ease-in-out`: the loops.
pub const EASE_IN_OUT: CubicBezier = CubicBezier::new(0.42, 0.0, 0.58, 1.0);
/// The panes' layout moves: `cubic-bezier(.2,.8,.2,1)`.
pub const PANE: CubicBezier = CubicBezier::new(0.2, 0.8, 0.2, 1.0);
/// The bank-card hero's pop: `cubic-bezier(.2,.9,.3,1.15)`.
pub const POP: CubicBezier = CubicBezier::new(0.2, 0.9, 0.3, 1.15);
/// Toasts: `cubic-bezier(.2,.9,.3,1.2)`.
pub const OVERSHOOT: CubicBezier = CubicBezier::new(0.2, 0.9, 0.3, 1.2);
/// The switch's knob: `cubic-bezier(.3,1.4,.5,1)`.
pub const SWITCH: CubicBezier = CubicBezier::new(0.3, 1.4, 0.5, 1.0);

/// How far something is from rest: multiply its alpha by `opacity`, move it
/// by (`dx`, `dy`) points and scale it about its centre by `scale`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub opacity: f32,
    pub dx: f32,
    pub dy: f32,
    pub scale: f32,
}

impl Pose {
    pub const REST: Pose = Pose { opacity: 1.0, dx: 0.0, dy: 0.0, scale: 1.0 };
}

fn entrance(p: f32, dy: f32, scale: f32) -> Pose {
    let rest = 1.0 - p;
    Pose { opacity: p, dx: 0.0, dy: dy * rest, scale: 1.0 - (1.0 - scale) * rest }
}

/// `dw-up`: rises 10 points and fades in.
pub fn up(p: f32) -> Pose {
    entrance(p, 10.0, 1.0)
}
/// `dw-fade`.
pub fn fade(p: f32) -> Pose {
    entrance(p, 0.0, 1.0)
}
/// `dw-pop`: rises 12 points from 97%.
pub fn pop(p: f32) -> Pose {
    entrance(p, 12.0, 0.97)
}
/// `dw-drop`: menus fall 8 points from 98%.
pub fn drop(p: f32) -> Pose {
    entrance(p, -8.0, 0.98)
}
/// `dw-tip`: help bubbles rise 4 points.
pub fn tip(p: f32) -> Pose {
    entrance(p, 4.0, 1.0)
}
/// `dw-toast`: rises 14 points.
pub fn toast(p: f32) -> Pose {
    entrance(p, 14.0, 1.0)
}
/// `dw-ring`: grows from 60%.
pub fn ring(p: f32) -> Pose {
    entrance(p, 0.0, 0.6)
}

/// `dw-breathe`: the splash shield's scale, 1 to 1.06 and back over 1.6 s.
pub fn breathe(seconds: f64) -> f32 {
    const PERIOD: f64 = 1.6;
    let u = (seconds.rem_euclid(PERIOD) / PERIOD) as f32;
    let swell = if u < 0.5 { EASE_IN_OUT.at(u * 2.0) } else { 1.0 - EASE_IN_OUT.at(u * 2.0 - 1.0) };
    1.0 + 0.06 * swell
}

/// `dw-line`: the splash bar's offset as a fraction of its own width, -110%
/// to 330% over 1.2 s, then again.
pub fn line(seconds: f64) -> f32 {
    const PERIOD: f64 = 1.2;
    let u = (seconds.rem_euclid(PERIOD) / PERIOD) as f32;
    -1.1 + 4.4 * EASE_IN_OUT.at(u)
}

/// `dw-pulse-w` / `dw-pulse-b`: a ring spreading to `max_radius` while it
/// fades, every 0.9 s. Returns (radius, alpha multiplier).
pub fn pulse(seconds: f64, max_radius: f32) -> (f32, f32) {
    const PERIOD: f64 = 0.9;
    let e = EASE_OUT.at((seconds.rem_euclid(PERIOD) / PERIOD) as f32);
    (max_radius * e, 1.0 - e)
}

/// A list's entrance delay for its `index`th row.
pub fn stagger(index: usize, step: f32) -> f32 {
    index as f32 * step
}

/// Eased progress (0 to 1) of an animation that started the first frame this
/// `id` was asked about, after `delay` seconds, lasting `duration` seconds.
///
/// **It asks for another frame only while it is still moving.** A window left
/// open with nothing changing must go back to drawing nothing; a settled
/// animation that kept requesting repaints would redraw it forever.
pub fn progress(ctx: &egui::Context, id: egui::Id, delay: f32, duration: f32, ease: CubicBezier) -> f32 {
    let now = ctx.input(|i| i.time);
    let start = ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(id, || now));
    let elapsed = (now - start) as f32 - delay;
    let linear = if duration <= 0.0 { 1.0 } else { (elapsed / duration).clamp(0.0, 1.0) };
    if linear < 1.0 {
        ctx.request_repaint();
        ease.at(linear)
    } else {
        1.0
    }
}

/// Forgets when `id` started, so its next [`progress`] begins again.
pub fn restart(ctx: &egui::Context, id: egui::Id) {
    ctx.data_mut(|d| d.remove::<f64>(id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    const ALL_CURVES: [CubicBezier; 6] = [EASE_OUT, EASE_IN_OUT, PANE, POP, OVERSHOOT, SWITCH];

    #[test]
    fn curves_start_at_0_and_end_at_1() {
        for curve in ALL_CURVES {
            assert!(close(curve.at(0.0), 0.0), "{curve:?} at 0");
            assert!(close(curve.at(1.0), 1.0), "{curve:?} at 1");
        }
    }

    #[test]
    fn ease_out_is_ahead_of_linear_halfway() {
        assert!(EASE_OUT.at(0.5) > 0.5);
    }

    #[test]
    fn the_pop_curves_overshoot() {
        for curve in [POP, OVERSHOOT, SWITCH] {
            let peak = (0..=100).map(|i| curve.at(i as f32 / 100.0)).fold(0.0, f32::max);
            assert!(peak > 1.0, "{curve:?} never passes 1: {peak}");
        }
    }

    #[test]
    fn entrances_begin_offset_and_end_at_rest() {
        assert_eq!(up(0.0), Pose { opacity: 0.0, dx: 0.0, dy: 10.0, scale: 1.0 });
        assert_eq!(up(1.0), Pose::REST);
        assert_eq!(fade(0.0).opacity, 0.0);
        assert_eq!(pop(0.0).scale, 0.97);
        assert_eq!(pop(0.0).dy, 12.0);
        assert_eq!(drop(0.0).dy, -8.0);
        assert_eq!(drop(0.0).scale, 0.98);
        assert_eq!(tip(0.0).dy, 4.0);
        assert_eq!(toast(0.0).dy, 14.0);
        assert_eq!(ring(0.0).scale, 0.6);
        for pose in [fade(1.0), pop(1.0), drop(1.0), tip(1.0), toast(1.0), ring(1.0)] {
            assert_eq!(pose, Pose::REST);
        }
    }

    #[test]
    fn breathe_peaks_at_half_period() {
        assert!(close(breathe(0.0), 1.0));
        assert!(close(breathe(0.8), 1.06));
        assert!(close(breathe(1.6), 1.0));
    }

    #[test]
    fn the_splash_bar_sweeps_and_repeats() {
        assert!(close(line(0.0), -1.1));
        assert!((line(1.2 - 1e-4) - 3.3).abs() < 0.01);
        assert!(close(line(1.2), -1.1));
    }

    #[test]
    fn pulse_grows_and_fades() {
        let (radius, alpha) = pulse(0.0, 16.0);
        assert!(close(radius, 0.0) && close(alpha, 1.0));
        let (radius, alpha) = pulse(0.9 - 1e-4, 16.0);
        assert!((radius - 16.0).abs() < 0.01 && alpha.abs() < 0.01);
    }

    #[test]
    fn stagger_is_index_times_step() {
        assert!(close(stagger(3, 0.035), 0.105));
    }

    #[test]
    fn progress_stops_requesting_repaints_once_settled() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("motion-test");
        let frame = |time: f64| {
            let mut value = 0.0;
            let output = ctx.run_ui(
                egui::RawInput { time: Some(time), ..Default::default() },
                |ui| value = progress(ui.ctx(), id, 0.0, 0.4, EASE_OUT),
            );
            let delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
            (value, delay)
        };
        let (first, first_delay) = frame(0.0);
        assert_eq!(first, 0.0);
        assert_eq!(first_delay, std::time::Duration::ZERO);
        let (middle, middle_delay) = frame(0.1);
        assert!(middle > 0.0 && middle < 1.0);
        assert_eq!(middle_delay, std::time::Duration::ZERO);
        let (end, _) = frame(0.5);
        assert_eq!(end, 1.0);
        // egui carries a repaint request one frame past the frame that made
        // it, so the window goes quiet on the frame after the one that
        // settled -- and must stay quiet after that.
        let (_, after) = frame(0.6);
        assert_ne!(after, std::time::Duration::ZERO);
        let (_, later) = frame(5.0);
        assert_ne!(later, std::time::Duration::ZERO);
    }

    #[test]
    fn a_delay_holds_progress_at_zero_until_it_passes() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("delayed");
        let at = |time: f64| {
            let mut value = 0.0;
            let _ = ctx.run_ui(egui::RawInput { time: Some(time), ..Default::default() }, |ui| {
                value = progress(ui.ctx(), id, 0.2, 0.4, EASE_OUT)
            });
            value
        };
        assert_eq!(at(0.0), 0.0);
        assert_eq!(at(0.15), 0.0);
        assert!(at(0.3) > 0.0);
    }
}
