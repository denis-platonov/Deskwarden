//! Read-only diagnostic: what does UI Automation actually expose that could
//! identify a login field, per visible top-level window?
//!
//! This exists to answer one design question with data, not to ship a
//! feature: the proposed "learn a field locator from what the user fills,
//! then match it later" design only works if the fields of the apps the user
//! really runs carry an identifier that survives an app restart. Run it,
//! restart the app, run it again, diff the two `--ids` outputs.
//!
//! ## What it will never do
//!
//! It never types, clicks, focuses, activates or sets anything: it calls only
//! `Current*` property getters and the `ControlViewWalker`. It never reads a
//! field's **value** -- a password may be sitting in one of these boxes, so
//! `ValuePattern` is not touched at all. `Name` is read (it is often the only
//! label there is), but some frameworks put the *current text* in `Name`, so
//! every `Name` goes through [`redact`] before printing and a password field's
//! `Name` is never printed at all. It reads no file, no vault, no settings.
//!
//! ## Usage
//!
//! ```text
//! cargo run --example field_locator_probe               # full human report
//! cargo run --example field_locator_probe -- --ids      # diffable identity only
//! cargo run --example field_locator_probe -- --filter epic
//! ```
//!
//! `--filter <substr>` keeps only windows whose title or exe name contains
//! `<substr>` (case-insensitive). Ordering is deterministic (exe, then title,
//! then tree order), so two runs diff cleanly.

use windows::core::Result;
use windows::Win32::Foundation::{HWND, RECT, RPC_E_CHANGED_MODE};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTreeWalker,
    UIA_EditControlTypeId,
};

/// One Edit (or password-reporting) control, flattened to the properties that
/// could plausibly serve as a locator.
struct Field {
    /// Index path from the window root through the control view, e.g.
    /// `0/3/1` -- the positional fallback today's injector effectively uses.
    path: String,
    automation_id: String,
    name: String,
    class_name: String,
    control_type: i32,
    localized_control_type: String,
    framework_id: String,
    help_text: String,
    labeled_by: String,
    aria_role: String,
    aria_properties: String,
    is_password: bool,
    is_enabled: bool,
    is_offscreen: bool,
    rect: RECT,
}

impl Field {
    /// Does this field carry anything that is plausibly a *stable* identity,
    /// as opposed to a position or a piece of chrome? `AutomationId` first,
    /// then the labelling properties. A `ClassName` alone is not enough:
    /// `Edit` / `Chrome_RenderWidgetHostHWND` name a widget kind, not a field.
    fn identity_strength(&self) -> &'static str {
        if !self.automation_id.is_empty() {
            "strong (AutomationId)"
        } else if !self.labeled_by.is_empty() || !self.help_text.is_empty() {
            "medium (label/helptext only)"
        } else if !self.aria_role.is_empty() || !self.aria_properties.is_empty() {
            "medium (ARIA only)"
        } else if !self.name.is_empty() {
            "weak (Name only -- may be the field's value)"
        } else {
            "none (position only)"
        }
    }
}

struct WindowReport {
    hwnd: isize,
    pid: u32,
    exe_name: String,
    title: String,
    hosted: bool,
    /// `Err` message if the UIA walk failed for this window.
    error: Option<String>,
    fields: Vec<Field>,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ids_only = args.iter().any(|a| a == "--ids");
    let filter = args
        .iter()
        .position(|a| a == "--filter")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.to_lowercase());

    unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if hr.is_err() && hr != RPC_E_CHANGED_MODE {
            eprintln!("CoInitializeEx failed: {hr:?}");
        }
    }

    let automation: IUIAutomation =
        match unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) } {
            Ok(a) => a,
            Err(e) => {
                eprintln!("could not create the UI Automation client: {e:?}");
                std::process::exit(1);
            }
        };
    let walker: IUIAutomationTreeWalker = match unsafe { automation.ControlViewWalker() } {
        Ok(w) => w,
        Err(e) => {
            eprintln!("could not get the control view walker: {e:?}");
            std::process::exit(1);
        }
    };

    // The probe's own pid is excluded the same way the picker excludes it.
    let mut windows = deskwarden::window_list::list_windows(std::process::id());
    windows.sort_by(|a, b| {
        (a.exe_name.to_lowercase(), a.title.clone(), a.hwnd)
            .cmp(&(b.exe_name.to_lowercase(), b.title.clone(), b.hwnd))
    });

    let mut reports = Vec::new();
    for w in windows {
        if let Some(f) = &filter {
            if !w.title.to_lowercase().contains(f) && !w.exe_name.to_lowercase().contains(f) {
                continue;
            }
        }
        let (fields, error) = match collect_fields(&automation, &walker, w.hwnd) {
            Ok(fields) => (fields, None),
            Err(e) => (Vec::new(), Some(format!("{e:?}"))),
        };
        reports.push(WindowReport {
            hwnd: w.hwnd,
            pid: w.pid,
            exe_name: w.exe_name,
            title: w.title,
            hosted: w.hosted,
            error,
            fields,
        });
    }

    if ids_only {
        print_ids(&reports);
    } else {
        print_full(&reports);
    }
}

/// Walks the control view under `hwnd` and returns every Edit control, plus
/// any control at all that reports `IsPassword` (a password box that is not
/// typed `Edit` is exactly the case worth seeing).
///
/// Depth and breadth are capped: a browser's document tree can run to tens of
/// thousands of nodes, and this is a diagnostic, not a crawler.
fn collect_fields(
    automation: &IUIAutomation,
    walker: &IUIAutomationTreeWalker,
    hwnd: isize,
) -> Result<Vec<Field>> {
    const MAX_DEPTH: usize = 24;
    const MAX_NODES: usize = 6000;

    let root = unsafe { automation.ElementFromHandle(HWND(hwnd as *mut core::ffi::c_void))? };
    let mut out = Vec::new();
    let mut visited = 0usize;
    let mut path = Vec::new();
    walk(walker, &root, &mut path, 0, MAX_DEPTH, &mut visited, MAX_NODES, &mut out);
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn walk(
    walker: &IUIAutomationTreeWalker,
    element: &IUIAutomationElement,
    path: &mut Vec<usize>,
    depth: usize,
    max_depth: usize,
    visited: &mut usize,
    max_nodes: usize,
    out: &mut Vec<Field>,
) {
    if depth > max_depth || *visited >= max_nodes {
        return;
    }
    *visited += 1;

    let control_type = unsafe { element.CurrentControlType() }.map(|t| t.0).unwrap_or(0);
    // **Fails CLOSED, unlike every other property here.** This one is the
    // redactor's switch: `false` sends the element's `Name` (and `HelpText`)
    // through the heuristic alone, which catches `hunter2!9x` and does not
    // catch `swordfish`. A UIA call can error for reasons that have nothing
    // to do with the answer -- a slow provider, an element torn down
    // mid-walk -- so an errored read must not be reported as "not a
    // password". One over-redacted label is the correct price.
    let is_password = unsafe { element.CurrentIsPassword() }
        .map(|b| b.as_bool())
        .unwrap_or(true);

    if control_type == UIA_EditControlTypeId.0 || is_password {
        out.push(describe(element, path, control_type, is_password));
    }

    let mut child = match unsafe { walker.GetFirstChildElement(element) } {
        Ok(c) => Some(c),
        Err(_) => None,
    };
    let mut index = 0usize;
    while let Some(c) = child {
        if *visited >= max_nodes {
            return;
        }
        path.push(index);
        walk(walker, &c, path, depth + 1, max_depth, visited, max_nodes, out);
        path.pop();
        index += 1;
        child = unsafe { walker.GetNextSiblingElement(&c) }.ok();
    }
}

fn describe(
    element: &IUIAutomationElement,
    path: &[usize],
    control_type: i32,
    is_password: bool,
) -> Field {
    // Every getter is a `Current*` read. Nothing here can change the target.
    let s = |r: windows::core::Result<windows::core::BSTR>| {
        r.map(|b| b.to_string()).unwrap_or_default()
    };
    let labeled_by = unsafe { element.CurrentLabeledBy() }
        .ok()
        .and_then(|el| unsafe { el.CurrentName() }.ok())
        .map(|b| b.to_string())
        .unwrap_or_default();

    Field {
        path: path.iter().map(|i| i.to_string()).collect::<Vec<_>>().join("/"),
        automation_id: s(unsafe { element.CurrentAutomationId() }),
        name: s(unsafe { element.CurrentName() }),
        class_name: s(unsafe { element.CurrentClassName() }),
        control_type,
        localized_control_type: s(unsafe { element.CurrentLocalizedControlType() }),
        framework_id: s(unsafe { element.CurrentFrameworkId() }),
        help_text: s(unsafe { element.CurrentHelpText() }),
        labeled_by,
        aria_role: s(unsafe { element.CurrentAriaRole() }),
        aria_properties: s(unsafe { element.CurrentAriaProperties() }),
        is_password,
        is_enabled: unsafe { element.CurrentIsEnabled() }
            .map(|b| b.as_bool())
            .unwrap_or(false),
        is_offscreen: unsafe { element.CurrentIsOffscreen() }
            .map(|b| b.as_bool())
            .unwrap_or(false),
        rect: unsafe { element.CurrentBoundingRectangle() }.unwrap_or_default(),
    }
}

/// Free text that might be a *label* ("Email", "Search") or might be the
/// field's current *contents* -- some frameworks put the value in `Name`.
/// The probe cannot tell which, so anything with the shape of content is
/// replaced by its shape. A password field's text is never printed at all.
///
/// Kept deliberately blunt: it is better to redact a real label and lose a
/// little diagnostic detail than to print one character of a secret.
fn redact(text: &str, is_password: bool) -> String {
    if text.is_empty() {
        return "<empty>".into();
    }
    if is_password {
        return format!("<redacted: password field, len {}>", text.chars().count());
    }
    let chars: Vec<char> = text.chars().collect();
    let looks_like_content = chars.len() > 40
        || text.contains('@')
        || (chars.len() >= 8
            && !text.contains(' ')
            && chars.iter().any(|c| c.is_ascii_digit())
            && chars.iter().any(|c| c.is_alphabetic()));
    if looks_like_content {
        format!(
            "<redacted: looks like content, len {}, {}>",
            chars.len(),
            shape(&chars)
        )
    } else {
        format!("{text:?}")
    }
}

/// A character-class summary -- enough to tell "Search Google or type a URL"
/// from "hunter2" without revealing either.
fn shape(chars: &[char]) -> String {
    let mut alpha = 0;
    let mut digit = 0;
    let mut space = 0;
    let mut other = 0;
    for c in chars {
        if c.is_alphabetic() {
            alpha += 1;
        } else if c.is_ascii_digit() {
            digit += 1;
        } else if c.is_whitespace() {
            space += 1;
        } else {
            other += 1;
        }
    }
    format!("{alpha} alpha / {digit} digit / {space} space / {other} other")
}

fn print_header() {
    println!("deskwarden field locator probe -- READ ONLY.");
    println!("No input is sent, nothing is focused, no field VALUE is read.");
    println!("`Name` is printed only after redaction: any Name that has the shape of");
    println!("content (long, contains '@', or an unbroken alphanumeric run) is replaced");
    println!("by its length and character-class shape, and a password field's Name is");
    println!("always redacted -- some frameworks put the current value in Name.");
    println!();
}

fn print_full(reports: &[WindowReport]) {
    print_header();
    for r in reports {
        println!("================================================================");
        println!("window : {:?}", r.title);
        println!("exe    : {}  pid {}  hwnd 0x{:x}", r.exe_name, r.pid, r.hwnd);
        println!("hosted : {} (ApplicationFrameHost frame with an app resolved inside)", r.hosted);
        if let Some(e) = &r.error {
            println!("UIA    : FAILED -- {e}");
            println!();
            continue;
        }
        println!("fields : {}", r.fields.len());
        if r.fields.is_empty() {
            println!("  (no Edit control and nothing reporting IsPassword)");
        }
        for f in &r.fields {
            println!("  ----");
            println!("  path            : {}", if f.path.is_empty() { "<root>" } else { &f.path });
            println!("  AutomationId    : {}", quote_or_empty(&f.automation_id));
            println!("  Name            : {}", redact(&f.name, f.is_password));
            println!("  ClassName       : {}", quote_or_empty(&f.class_name));
            println!("  ControlType     : {} ({})", f.control_type, quote_or_empty(&f.localized_control_type));
            println!("  FrameworkId     : {}", quote_or_empty(&f.framework_id));
            println!("  HelpText        : {}", redact(&f.help_text, f.is_password));
            println!("  LabeledBy.Name  : {}", redact(&f.labeled_by, false));
            println!("  AriaRole        : {}", quote_or_empty(&f.aria_role));
            // Free text, and Chromium puts `valuetext=` in it -- so it goes
            // through the redactor like every other free-text field, not
            // through `quote_or_empty`.
            println!("  AriaProperties  : {}", redact(&f.aria_properties, f.is_password));
            println!("  IsPassword      : {}", f.is_password);
            println!("  IsEnabled       : {}   IsOffscreen: {}", f.is_enabled, f.is_offscreen);
            println!(
                "  Rect            : ({}, {}) {}x{}",
                f.rect.left,
                f.rect.top,
                f.rect.right - f.rect.left,
                f.rect.bottom - f.rect.top
            );
            println!("  locator verdict : {}", f.identity_strength());
        }
        println!();
    }
    print_summary(reports);
}

/// The diffable mode: only the properties a locator could be built from, no
/// geometry, no counts, no free text that could churn between runs for
/// reasons that have nothing to do with identity.
fn print_ids(reports: &[WindowReport]) {
    println!("# deskwarden field locator probe -- identity only (diff two runs of this)");
    for r in reports {
        println!("[{}] {:?}", r.exe_name, r.title);
        if let Some(e) = &r.error {
            println!("  !uia-failed {e}");
            continue;
        }
        for f in &r.fields {
            println!(
                "  {} fw={} class={} type={} pw={} aid={} aria={} label={}",
                if f.path.is_empty() { "<root>" } else { &f.path },
                dash_if_empty(&f.framework_id),
                dash_if_empty(&f.class_name),
                f.control_type,
                f.is_password as u8,
                dash_if_empty(&f.automation_id),
                dash_if_empty(&f.aria_role),
                dash_if_empty(&f.labeled_by),
            );
        }
    }
}

fn print_summary(reports: &[WindowReport]) {
    println!("================================================================");
    println!("SUMMARY (one line per window with at least one field)");
    println!(
        "{:<28} {:<18} {:>6} {:>4} {:>10}  {}",
        "exe", "frameworks", "fields", "pw", "with-aid", "title"
    );
    for r in reports {
        if r.fields.is_empty() {
            continue;
        }
        let mut frameworks: Vec<&str> =
            r.fields.iter().map(|f| f.framework_id.as_str()).collect();
        frameworks.sort_unstable();
        frameworks.dedup();
        let pw = r.fields.iter().filter(|f| f.is_password).count();
        let aid = r.fields.iter().filter(|f| !f.automation_id.is_empty()).count();
        println!(
            "{:<28} {:<18} {:>6} {:>4} {:>10}  {:?}",
            truncate(&r.exe_name, 28),
            truncate(&frameworks.join(","), 18),
            r.fields.len(),
            pw,
            aid,
            truncate(&r.title, 40)
        );
    }
}

fn quote_or_empty(s: &str) -> String {
    if s.is_empty() {
        "<empty>".into()
    } else {
        format!("{s:?}")
    }
}

fn dash_if_empty(s: &str) -> &str {
    if s.is_empty() {
        "-"
    } else {
        s
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "\u{2026}"
    }
}
