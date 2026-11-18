//! The design's icons, drawn from its own SVG path data.
//!
//! The v2 canvas draws every icon as an inline SVG `<path d="...">` on a 24x24
//! viewBox, stroked at 2-2.4 with round caps. Rather than redraw each one by
//! hand, this parses the design's path strings as they are and flattens them
//! into polylines, so a glyph here is the design's glyph by construction.
//! Hand-written on purpose: a few hundred lines are cheaper to own than an SVG
//! dependency, and the crate pins its dependency set.

use eframe::egui::{self, epaint::PathShape, Color32, Pos2, Rect, Stroke, Vec2};

/// One run of a path, in viewBox units.
#[derive(Clone, Debug, PartialEq)]
pub struct Subpath {
    pub points: Vec<Pos2>,
    pub closed: bool,
}

/// Why a path could not be read: the byte offset and what was wrong there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IconError {
    pub at: usize,
    pub reason: &'static str,
}

/// Points per cubic or quadratic segment.
const CURVE_STEPS: usize = 12;
/// One flattened point per this many radians of arc (15 degrees).
const ARC_STEP: f32 = std::f32::consts::PI / 12.0;

struct Cursor<'a> {
    s: &'a [u8],
    i: usize,
}

impl Cursor<'_> {
    fn skip_separators(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b',' | b'\t' | b'\r' | b'\n') {
            self.i += 1;
        }
    }

    fn at_end(&mut self) -> bool {
        self.skip_separators();
        self.i >= self.s.len()
    }

    fn command(&mut self) -> Option<u8> {
        self.skip_separators();
        let c = *self.s.get(self.i)?;
        if c.is_ascii_alphabetic() && c != b'e' && c != b'E' {
            self.i += 1;
            Some(c)
        } else {
            None
        }
    }

    fn error(&self, reason: &'static str) -> IconError {
        IconError { at: self.i, reason }
    }

    /// One number. A second `.` or a sign starts the next number, as SVG
    /// allows ("25.4.9" is 25.4 then .9; "-4.5-11" is -4.5 then -11).
    fn number(&mut self) -> Result<f32, IconError> {
        self.skip_separators();
        let start = self.i;
        let s = self.s;
        let mut i = self.i;
        if i < s.len() && matches!(s[i], b'+' | b'-') {
            i += 1;
        }
        let mut digits = false;
        while i < s.len() && s[i].is_ascii_digit() {
            i += 1;
            digits = true;
        }
        if i < s.len() && s[i] == b'.' {
            i += 1;
            while i < s.len() && s[i].is_ascii_digit() {
                i += 1;
                digits = true;
            }
        }
        if !digits {
            return Err(self.error("expected a number"));
        }
        if i < s.len() && matches!(s[i], b'e' | b'E') {
            let mut j = i + 1;
            if j < s.len() && matches!(s[j], b'+' | b'-') {
                j += 1;
            }
            if j < s.len() && s[j].is_ascii_digit() {
                while j < s.len() && s[j].is_ascii_digit() {
                    j += 1;
                }
                i = j;
            }
        }
        self.i = i;
        std::str::from_utf8(&s[start..i])
            .ok()
            .and_then(|t| t.parse::<f32>().ok())
            .ok_or(IconError { at: start, reason: "unreadable number" })
    }

    /// An arc flag, which may be packed with no separator ("0 01").
    fn flag(&mut self) -> Result<bool, IconError> {
        self.skip_separators();
        match self.s.get(self.i) {
            Some(b'0') => {
                self.i += 1;
                Ok(false)
            }
            Some(b'1') => {
                self.i += 1;
                Ok(true)
            }
            _ => Err(self.error("expected an arc flag")),
        }
    }

    fn point(&mut self) -> Result<Pos2, IconError> {
        Ok(Pos2::new(self.number()?, self.number()?))
    }
}

fn cubic(out: &mut Vec<Pos2>, p0: Pos2, p1: Pos2, p2: Pos2, p3: Pos2) {
    for step in 1..=CURVE_STEPS {
        let t = step as f32 / CURVE_STEPS as f32;
        let u = 1.0 - t;
        let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
        out.push(Pos2::new(
            w[0] * p0.x + w[1] * p1.x + w[2] * p2.x + w[3] * p3.x,
            w[0] * p0.y + w[1] * p1.y + w[2] * p2.y + w[3] * p3.y,
        ));
    }
}

/// An SVG elliptical arc from `from` to `to`, flattened (endpoint to centre
/// parameterisation, SVG 1.1 appendix F.6.5).
#[allow(clippy::too_many_arguments)]
fn arc(out: &mut Vec<Pos2>, from: Pos2, rx: f32, ry: f32, rotation_deg: f32, large: bool, sweep: bool, to: Pos2) {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx < 1e-6 || ry < 1e-6 || from == to {
        out.push(to);
        return;
    }
    let (sin, cos) = rotation_deg.to_radians().sin_cos();
    let dx = (from.x - to.x) / 2.0;
    let dy = (from.y - to.y) / 2.0;
    let x1 = cos * dx + sin * dy;
    let y1 = -sin * dx + cos * dy;
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        let k = lambda.sqrt();
        rx *= k;
        ry *= k;
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut coef = (num / den).max(0.0).sqrt();
    if large == sweep {
        coef = -coef;
    }
    let cx1 = coef * rx * y1 / ry;
    let cy1 = -coef * ry * x1 / rx;
    let cx = cos * cx1 - sin * cy1 + (from.x + to.x) / 2.0;
    let cy = sin * cx1 + cos * cy1 + (from.y + to.y) / 2.0;
    let angle = |ux: f32, uy: f32, vx: f32, vy: f32| (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
    let ux = (x1 - cx1) / rx;
    let uy = (y1 - cy1) / ry;
    let vx = (-x1 - cx1) / rx;
    let vy = (-y1 - cy1) / ry;
    let theta = angle(1.0, 0.0, ux, uy);
    let mut delta = angle(ux, uy, vx, vy);
    if !sweep && delta > 0.0 {
        delta -= std::f32::consts::TAU;
    } else if sweep && delta < 0.0 {
        delta += std::f32::consts::TAU;
    }
    let steps = ((delta.abs() / ARC_STEP).ceil() as usize).max(1);
    for step in 1..=steps {
        let t = theta + delta * step as f32 / steps as f32;
        let (st, ct) = t.sin_cos();
        out.push(Pos2::new(cos * rx * ct - sin * ry * st + cx, sin * rx * ct + cos * ry * st + cy));
    }
    // Land exactly on the endpoint the path names.
    if let Some(last) = out.last_mut() {
        *last = to;
    }
}

/// Reads SVG path data (`M m L l H h V v C c S s Q q A a Z z`) into flattened
/// subpaths in viewBox units.
pub fn parse(d: &str) -> Result<Vec<Subpath>, IconError> {
    let mut c = Cursor { s: d.as_bytes(), i: 0 };
    let mut out: Vec<Subpath> = Vec::new();
    let mut points: Vec<Pos2> = Vec::new();
    let mut pos = Pos2::ZERO;
    let mut start = Pos2::ZERO;
    let mut last: Option<u8> = None;
    // The previous segment's second control point, for S and T reflection.
    let mut last_ctrl: Option<Pos2> = None;

    let flush = |points: &mut Vec<Pos2>, out: &mut Vec<Subpath>, closed: bool| {
        if points.len() > 1 || (closed && !points.is_empty()) {
            out.push(Subpath { points: std::mem::take(points), closed });
        } else {
            points.clear();
        }
    };

    while !c.at_end() {
        let cmd = match c.command() {
            Some(cmd) => cmd,
            None => match last {
                Some(b'M') => b'L',
                Some(b'm') => b'l',
                Some(b'Z' | b'z') | None => return Err(c.error("expected a command")),
                Some(prev) => prev,
            },
        };
        if last.is_none() && !matches!(cmd, b'M' | b'm') {
            return Err(c.error("a path must start with a move"));
        }
        let relative = cmd.is_ascii_lowercase();
        let base = if relative { pos.to_vec2() } else { Vec2::ZERO };
        if !matches!(cmd, b'M' | b'm' | b'Z' | b'z') && points.is_empty() {
            points.push(pos);
        }
        let mut ctrl = None;
        match cmd.to_ascii_uppercase() {
            b'M' => {
                flush(&mut points, &mut out, false);
                pos = c.point()? + base;
                start = pos;
                points.push(pos);
            }
            b'L' => {
                pos = c.point()? + base;
                points.push(pos);
            }
            b'H' => {
                let x = c.number()?;
                pos.x = if relative { pos.x + x } else { x };
                points.push(pos);
            }
            b'V' => {
                let y = c.number()?;
                pos.y = if relative { pos.y + y } else { y };
                points.push(pos);
            }
            b'C' => {
                let (p1, p2, p3) = (c.point()? + base, c.point()? + base, c.point()? + base);
                cubic(&mut points, pos, p1, p2, p3);
                pos = p3;
                ctrl = Some(p2);
            }
            b'S' => {
                let p1 = match (last.map(|l| l.to_ascii_uppercase()), last_ctrl) {
                    (Some(b'C' | b'S'), Some(prev)) => pos + (pos - prev),
                    _ => pos,
                };
                let (p2, p3) = (c.point()? + base, c.point()? + base);
                cubic(&mut points, pos, p1, p2, p3);
                pos = p3;
                ctrl = Some(p2);
            }
            b'Q' => {
                let (q, p3) = (c.point()? + base, c.point()? + base);
                let p1 = pos + (q - pos) * (2.0 / 3.0);
                let p2 = p3 + (q - p3) * (2.0 / 3.0);
                cubic(&mut points, pos, p1, p2, p3);
                pos = p3;
                ctrl = Some(q);
            }
            b'A' => {
                let (rx, ry, rot) = (c.number()?, c.number()?, c.number()?);
                let (large, sweep) = (c.flag()?, c.flag()?);
                let to = c.point()? + base;
                arc(&mut points, pos, rx, ry, rot, large, sweep, to);
                pos = to;
            }
            b'Z' => {
                pos = start;
                flush(&mut points, &mut out, true);
            }
            _ => return Err(IconError { at: c.i.saturating_sub(1), reason: "unsupported command" }),
        }
        last_ctrl = ctrl;
        last = Some(cmd);
    }
    flush(&mut points, &mut out, false);
    Ok(out)
}

/// Maps viewBox units into `rect`: uniform scale, centred.
fn fit(rect: Rect, view: Vec2) -> impl Fn(Pos2) -> Pos2 {
    let scale = (rect.width() / view.x).min(rect.height() / view.y);
    let origin = rect.center() - view * scale / 2.0;
    move |p| origin + p.to_vec2() * scale
}

/// Strokes the 24x24 glyph `d` into `rect`. `width` is in viewBox units, as
/// the design's `stroke-width` is.
pub fn stroke(painter: &egui::Painter, rect: Rect, d: &str, width: f32, color: Color32) {
    let Ok(paths) = parse(d) else {
        debug_assert!(false, "unreadable icon path data: {d}");
        return;
    };
    let view = Vec2::splat(24.0);
    let map = fit(rect, view);
    let scale = (rect.width() / view.x).min(rect.height() / view.y);
    let pen = Stroke::new(width * scale, color);
    for path in paths {
        let points: Vec<Pos2> = path.points.iter().map(|p| map(*p)).collect();
        if path.closed {
            painter.add(PathShape::closed_line(points, pen));
        } else {
            // The design's `stroke-linecap: round`: egui cuts line ends
            // square, so each open end gets a disc of the stroke's width.
            let ends = [points[0], points[points.len() - 1]];
            painter.add(PathShape::line(points, pen));
            for end in ends {
                painter.circle_filled(end, pen.width / 2.0, color);
            }
        }
    }
}

/// Fills each subpath of `d` (on a `view`-sized viewBox) into `rect`. Every
/// subpath must be convex.
pub fn fill(painter: &egui::Painter, rect: Rect, d: &str, view: Vec2, color: Color32) {
    let Ok(paths) = parse(d) else {
        debug_assert!(false, "unreadable icon path data: {d}");
        return;
    };
    let map = fit(rect, view);
    for path in paths {
        let points: Vec<Pos2> = path.points.iter().map(|p| map(*p)).collect();
        painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
    }
}

/// The design's glyphs, as its path data (24x24 viewBox).
pub mod glyph {
    pub const ALL: &str = "M3 3h7v7H3zM14 3h7v7h-7zM14 14h7v7h-7zM3 14h7v7H3z";
    pub const FAV: &str = "M12 2.8l2.8 5.8 6.4.9-4.6 4.5 1.1 6.3L12 17.3l-5.7 3 1.1-6.3-4.6-4.5 6.4-.9z";
    pub const LOGIN: &str = "M15 7a3 3 0 1 1-6 0 3 3 0 0 1 6 0zM12 10v11M12 15h4M12 19h3";
    pub const CARD: &str = "M2 6a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2zM2 10h20";
    pub const NOTE: &str = "M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8zM14 2v6h6M16 13H8M16 17H8";
    pub const TRASH: &str = "M3 6h18M8 6V4h8v2M6 6l1 14h10l1-14";
    pub const SHARED: &str = "M22 2 11 13M22 2l-7 20-4-9-9-4z";
    pub const GEN: &str = "M21 12a9 9 0 1 1-2.64-6.36M21 3v6h-6";
    pub const SETTINGS: &str = "M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6";
    pub const FOLDER: &str = "M4 4h5l2 3h9a1 1 0 0 1 1 1v10a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2z";
    pub const PLUS: &str = "M12 5v14M5 12h14";
    pub const LOCK: &str = "M5 11h14v10H5zM8 11V7a4 4 0 0 1 8 0v4";
    pub const BACK: &str = "M19 12H5M12 19l-7-7 7-7";
    pub const CHECK: &str = "M5 12l5 5 9-10";

    /// Every glyph, named, for tests and the kit sheet.
    pub const ALL_GLYPHS: [(&str, &str); 14] = [
        ("all", ALL),
        ("fav", FAV),
        ("login", LOGIN),
        ("card", CARD),
        ("note", NOTE),
        ("trash", TRASH),
        ("shared", SHARED),
        ("gen", GEN),
        ("settings", SETTINGS),
        ("folder", FOLDER),
        ("plus", PLUS),
        ("lock", LOCK),
        ("back", BACK),
        ("check", CHECK),
    ];
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Pos2;

    fn near(a: Pos2, b: Pos2) -> bool {
        (a.x - b.x).abs() < 0.01 && (a.y - b.y).abs() < 0.01
    }

    #[test]
    fn a_relative_horizontal_line_parses_to_two_points() {
        let paths = parse("M3 6h18").unwrap();
        assert_eq!(paths.len(), 1);
        assert!(!paths[0].closed);
        assert_eq!(paths[0].points, vec![Pos2::new(3.0, 6.0), Pos2::new(21.0, 6.0)]);
    }

    #[test]
    fn close_marks_the_subpath_closed() {
        let paths = parse("M5 11h14v10H5z").unwrap();
        assert_eq!(paths.len(), 1);
        assert!(paths[0].closed);
        assert_eq!(paths[0].points.len(), 4);
    }

    #[test]
    fn packed_numbers_split_where_svg_splits_them() {
        let paths = parse("M.5 14H12v13.2C5 25.4.9 21 .5 14Z").unwrap();
        let points = &paths[0].points;
        assert!(near(points[0], Pos2::new(0.5, 14.0)));
        assert!(points.iter().any(|p| near(*p, Pos2::new(12.0, 27.2))));
        assert!(near(*points.last().unwrap(), Pos2::new(0.5, 14.0)));
        assert!(paths[0].closed);
    }

    #[test]
    fn an_arc_ends_where_svg_says() {
        let paths = parse("M8 11V7a4 4 0 0 1 8 0v4").unwrap();
        let points = &paths[0].points;
        assert!(near(*points.last().unwrap(), Pos2::new(16.0, 11.0)));
        // The arc runs from (8,7) to (16,7) over the top of a circle of radius
        // 4 centred at (12,7).
        let arc: Vec<_> = points.iter().filter(|p| p.y < 7.0 - 1e-3).collect();
        assert!(!arc.is_empty(), "the arc should bulge upwards");
        for p in arc {
            assert!(((p.x - 12.0).hypot(p.y - 7.0) - 4.0).abs() < 0.01, "{p:?} is off the circle");
        }
    }

    #[test]
    fn every_glyph_parses_inside_the_viewbox() {
        for (name, d) in glyph::ALL_GLYPHS {
            let paths = parse(d).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert!(!paths.is_empty(), "{name} is empty");
            for p in paths.iter().flat_map(|s| &s.points) {
                assert!((0.0..=24.0).contains(&p.x) && (0.0..=24.0).contains(&p.y), "{name}: {p:?}");
            }
        }
    }

    #[test]
    fn malformed_path_data_is_an_error_not_a_panic() {
        assert!(parse("M3").is_err());
        assert!(parse("X1 2").is_err());
        assert!(parse("M1 2 L").is_err());
        assert!(parse("L1 2").is_err(), "a path must start with a move");
        assert_eq!(parse("").unwrap().len(), 0);
    }
}
