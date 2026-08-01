//! The vault window's right pane in read mode (design 4.8 "Detail pane"):
//! title bar, LOGIN CREDENTIALS card, AUTOFILL TARGETS card, and the
//! metadata strip. Edit mode is `detail_edit.rs` (Task 8) -- kept separate
//! because the two have almost no shared state (read mode is passive
//! display + copy actions; edit mode owns a draft `VaultItem` and validates
//! it), and the read-mode file was already large enough on its own.

use crate::password_strength;
use crate::theme;
use crate::vault_bridge::VaultItem;
use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke};

/// The One-time code row's single source of truth. Replaces a bare
/// `Option<String>` (`Some(code)` / `None`), which could not tell apart
/// three genuinely different situations: no TOTP secret configured on this
/// item, a live code, and "the backend could not be reached to find out
/// which of the other two is true". Collapsing those onto one `Option`
/// is what let three consecutive commits each fix one confusion between
/// them and introduce another (independent review of a7b33cb) -- a stale
/// code kept rendering after its secret was removed elsewhere, and later a
/// backend outage made the row vanish entirely, looking identical to "no
/// TOTP here" and inviting a needless 2FA re-enrolment.
///
/// Computed in exactly one place (`vault_window::mod`'s per-frame TOTP
/// block) and rendered exhaustively below (`draw_detail_read`'s `match` has
/// no catch-all arm), so a future variant is a compile error here rather
/// than a silently-unhandled case.
#[derive(Debug, Clone, PartialEq)]
pub enum TotpState {
    /// This item has no TOTP secret configured -- the row is omitted
    /// entirely, same as before TOTP existed in this pane at all.
    ///
    /// *Derived from the item*, every frame, by
    /// `vault_window::mod::totp_state_for_secret_presence`: it is whatever
    /// the item we currently hold says, so a secret removed on another
    /// device clears the row in the same frame the reload lands. It is
    /// deliberately **not** a conclusion drawn from a poll -- that is
    /// `NoCodeReported`, below.
    NoSecret,
    /// This item *does* have a TOTP secret, but the poll for its current
    /// code is a background thread now (see `totp_poll_in_flight`'s doc in
    /// `vault_window::mod`) and hasn't reported back yet -- typically just
    /// the first frame or two after selecting this item, but as long as
    /// ~10s (`ureq`'s read timeout) if a *different* item's poll is still
    /// outstanding and holding the one-poll-at-a-time gate. Distinct from
    /// `NoSecret` for the same reason `Unavailable` is (review 12's
    /// Important 3): rendering no row at all here is pixel-identical to "no
    /// TOTP configured", when this item plainly has one -- the code just
    /// hasn't arrived yet.
    Fetching,
    /// A live code, fetched from `bw serve` on the last successful poll.
    /// `seconds_left` is derived from the wall clock (the 30s TOTP window),
    /// not from the fetch, and is refreshed every frame regardless of
    /// whether a poll happened this tick.
    Code { code: String, seconds_left: u8 },
    /// This item *does* have a TOTP secret configured, but the last poll
    /// could not reach `bw serve` (or it answered with an error other than
    /// "no TOTP configured") to fetch the current code. Distinct from
    /// `NoSecret` specifically so the row stays visible with an honest
    /// "unavailable" state instead of vanishing and reading as "not set up".
    Unavailable,
    /// A poll *answered*, successfully, that there is no current code for
    /// this item (`get_totp` -> `Ok(None)`; `bw serve` returns `400` for
    /// this, see `VaultBridge::get_totp`). Keeps its own row
    /// (`totp_no_code_row`), distinct from both `NoSecret`'s absent row and
    /// `Unavailable`'s: at the live call site this state can *only* mean a
    /// disagreement -- `get_totp` is reachable only for an item whose own
    /// login data carries a seed, so "this item has no TOTP" is not one of
    /// the things `Ok(None)` can be saying (review 14's Important; 48cff27
    /// rendered it as no row and that justification did not survive contact
    /// with the call site).
    ///
    /// It is also a *separate variant* from `NoSecret` so the per-frame
    /// presence derivation (`totp_state_for_secret_presence`) structurally
    /// cannot see it and cannot promote it, and so the poll gate
    /// (`totp_state_wants_poll`) can stop asking.
    ///
    /// Review 13's Important: this used to share `NoSecret`, and the two
    /// situations behind that one value pulled in opposite directions.
    /// `NoSecret` is *derived from the item* and must be re-derived every
    /// frame (review 9's fix -- a remotely removed secret has to clear
    /// immediately), which meant the unconditional derivation promoted a
    /// just-polled `Ok(None)` straight back to `Fetching`, the poll gate
    /// fired again a second later, and an item whose stored seed the backend
    /// rejects (removed on another device before a sync landed, or
    /// malformed) sat on "One-time code / Fetching..." forever while issuing
    /// one HTTP round-trip per second, indefinitely. Splitting the two
    /// situations into two variants is what dissolves that, the same move
    /// that dissolved `TotpState` itself, `PickerItemsResult`,
    /// `BackendReadiness` and `VaultReadyOutcome`.
    ///
    /// Reset on selection change (`run`'s reset block sets `NoSecret`, which
    /// the derivation then promotes to `Fetching`), so selecting the item
    /// again polls normally.
    NoCodeReported,
}

/// What the LOGIN CREDENTIALS card's One-time code row actually shows --
/// the *render* layer's own vocabulary, derived from [`TotpState`] by
/// [`totp_row_for`] and nothing else.
///
/// This exists because the render layer had its own instance of the bug that
/// [`TotpState`]'s own variants were introduced to kill: several distinct
/// situations sharing one representation. `NoSecret` and `NoCodeReported`
/// were two different facts about an item that both drew *nothing*, and
/// "nothing" is not a neutral rendering -- it is the pixel-for-pixel
/// rendering of "this item has no 2FA at all", which is what made a
/// previous reviewer point out a user would re-enrol.
///
/// `Option<TotpRow>` is the single source of truth for whether the row
/// appears: `draw_detail_read` cannot omit a row without `totp_row_for`
/// having said `None`, so the decision is unit-testable directly rather than
/// living inside an `egui` closure no test can call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TotpRow<'a> {
    /// [`TotpState::Fetching`].
    Fetching,
    /// [`TotpState::Code`].
    Code { code: &'a str, seconds_left: u8 },
    /// [`TotpState::Unavailable`].
    Unavailable,
    /// [`TotpState::NoCodeReported`].
    NoCode,
}

/// The One-time code row for a given [`TotpState`], or `None` for the one
/// state that genuinely means "this item has no one-time codes".
///
/// Exhaustive with no catch-all, so a new `TotpState` variant is a compile
/// error here rather than silently inheriting some other variant's pixels.
///
/// **Only `NoSecret` may return `None`.** Review 14's Important: at the live
/// call site (`vault_window::mod`'s per-frame TOTP block), `get_totp` is
/// only ever called for an item whose *own login data carries a seed*, so
/// the one situation in which `Ok(None)` could have meant "this item has no
/// TOTP" is unreachable -- every `NoCodeReported` that can actually occur is
/// a *disagreement* between the cached item and `bw serve`. Drawing that as
/// an absent row is the same "reads as: this item has no TOTP" conflation
/// reviews 8 and 12 forced out of `Unavailable` and `Fetching`; it had
/// simply reappeared one layer down.
pub fn totp_row_for(totp: &TotpState) -> Option<TotpRow<'_>> {
    match totp {
        TotpState::NoSecret => None,
        TotpState::Fetching => Some(TotpRow::Fetching),
        TotpState::Code { code, seconds_left } => Some(TotpRow::Code {
            code: code.as_str(),
            seconds_left: *seconds_left,
        }),
        TotpState::Unavailable => Some(TotpRow::Unavailable),
        TotpState::NoCodeReported => Some(TotpRow::NoCode),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailAction {
    None,
    Edit,
    Fill,
    CopyUsername,
    CopyPassword,
    CopyTotp,
    OpenWebsite(String),
    /// The header's Delete button was clicked. `vault_window::mod`'s
    /// two-click `confirm_click` gates whether this click is armed or
    /// confirming -- `draw_detail_read` itself only reports the click, via
    /// `delete_pending` (see that param's doc comment) for which label/state
    /// to show.
    Delete,
}

/// The metadata strip's text: "Updated N days ago · Filled N times ·
/// Strength: X". `updated_days_ago` is `None` when the item carries no
/// parseable `revisionDate` (shows "Updated recently" rather than
/// fabricating a number).
pub fn metadata_line(updated_days_ago: Option<i64>, fill_count: u32, password: &str) -> String {
    let updated = match updated_days_ago {
        Some(0) => "Updated today".to_string(),
        Some(1) => "Updated 1 day ago".to_string(),
        Some(n) => format!("Updated {n} days ago"),
        None => "Updated recently".to_string(),
    };
    let filled = if fill_count == 1 {
        "Filled 1 time".to_string()
    } else {
        format!("Filled {fill_count} times")
    };
    let strength = password_strength::rate(password).label();
    format!("{updated} \u{b7} {filled} \u{b7} Strength: {strength}")
}

pub fn draw_detail_read(
    ui: &mut egui::Ui,
    item: &VaultItem,
    fill_count: u32,
    totp: &TotpState,
    // Whether *this* item currently has a delete armed (its first click
    // already happened and the confirm window hasn't expired) -- purely for
    // what the Delete button shows; `vault_window::mod`'s `confirm_click` is
    // what actually decides whether a click here is arming or confirming.
    delete_pending: bool,
    reveal_password: &mut bool,
    // This item's favicon texture, if `vault_window::mod`'s icon cache has
    // already loaded one -- mirrors `item_list.rs`'s `item_row`, which uses
    // the exact same `Some(tex)`/`None` pattern for its row avatar. `None`
    // falls back to the colored-initials monogram, same as every other
    // avatar in this app when no favicon is available.
    icon: Option<&egui::TextureHandle>,
) -> DetailAction {
    let mut action = DetailAction::None;
    let login = item.login.as_ref();
    let username = login.and_then(|l| l.username.as_deref()).unwrap_or("");
    let password = login
        .and_then(|l| l.password.as_deref())
        .map(|p| p.as_str())
        .unwrap_or("");

    ui.horizontal(|ui| {
        match icon {
            Some(tex) => {
                // Rounded to match `theme::avatar`'s initials-tile treatment
                // (same `size * 0.25` formula) -- see `item_list.rs`'s
                // matching fix for why an unrounded favicon in an identical
                // box reads as visually heavier than the monogram fallback.
                const SIZE: f32 = 44.0;
                ui.add(
                    egui::Image::new((tex.id(), tex.size_vec2()))
                        .fit_to_exact_size(egui::Vec2::splat(SIZE))
                        .corner_radius(CornerRadius::same((SIZE * 0.25) as u8)),
                );
            }
            None => theme::avatar(ui, &theme::initials(&item.name), 44.0, true),
        }
        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.label(theme::bold(&item.name, 22.0).color(theme::INK));
            ui.label(RichText::new("Login").size(12.0).color(theme::TEXT_FAINT));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::secondary_button(ui, "Edit").clicked() {
                action = DetailAction::Edit;
            }
            if theme::primary_button(ui, "Fill in app", Some("CTRL+SHIFT+F")).clicked() {
                action = DetailAction::Fill;
            }
            let (delete_label, delete_hover, delete_color) = if delete_pending {
                (
                    "Delete? Click to confirm",
                    "Click again to delete this item. It may still be recoverable from \
                     bitwarden.com or another Bitwarden client afterward.",
                    theme::ERROR,
                )
            } else {
                ("Delete", "Delete this item", theme::INK)
            };
            let delete_button = egui::Button::new(theme::semibold(delete_label, 13.0).color(delete_color))
                .fill(theme::CARD)
                .stroke(Stroke::new(1.0, if delete_pending { theme::ERROR } else { theme::BORDER_STRONG }))
                .corner_radius(CornerRadius::same(7))
                .min_size(egui::Vec2::new(0.0, 32.0));
            if ui.add(delete_button).on_hover_text(delete_hover).clicked() {
                action = DetailAction::Delete;
            }
        });
    });
    ui.add_space(14.0);

    card(ui, "LOGIN CREDENTIALS", |ui| {
        credential_row(ui, "Username", username, "Copy", &mut action, DetailAction::CopyUsername);
        theme::hairline(ui);
        password_row(ui, password, reveal_password, &mut action);
        // Whether there is a row at all is decided by `totp_row_for` and
        // nowhere else (see its doc), so "this item looks like it has no
        // 2FA" is a decision a unit test can call directly instead of one
        // buried in an `egui` closure. Exhaustive on purpose -- no catch-all
        // arm -- so a new `TotpRow` variant fails to compile here instead of
        // silently inheriting whatever the last arm happened to draw.
        if let Some(row) = totp_row_for(totp) {
            theme::hairline(ui);
            match row {
                TotpRow::Fetching => totp_fetching_row(ui),
                TotpRow::Code { code, seconds_left } => totp_code_row(ui, code, seconds_left, &mut action),
                TotpRow::Unavailable => totp_unavailable_row(ui),
                TotpRow::NoCode => totp_no_code_row(ui),
            }
        }
    });
    ui.add_space(10.0);

    let website = login
        .and_then(|l| l.uris.first())
        .and_then(|u| u.uri.as_deref())
        .unwrap_or("");
    if !website.is_empty() {
        card(ui, "AUTOFILL TARGETS", |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(website).size(13.0).color(theme::TEXT_SECONDARY));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::secondary_button(ui, "Open").clicked() {
                        action = DetailAction::OpenWebsite(website.to_string());
                    }
                });
            });
        });
        ui.add_space(10.0);
    }

    let updated_days_ago = item
        .other
        .get("revisionDate")
        .and_then(|v| v.as_str())
        .and_then(days_since);
    ui.label(
        RichText::new(metadata_line(updated_days_ago, fill_count, password))
            .size(11.0)
            .color(theme::TEXT_GHOST),
    );

    action
}

fn card(ui: &mut egui::Ui, title: &str, contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(theme::CARD)
        .corner_radius(CornerRadius::same(10))
        .stroke(Stroke::new(1.0, theme::HAIRLINE))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(theme::letterspaced(title, 10.0, theme::SEMIBOLD, 1.2, theme::TEXT_GHOST));
            ui.add_space(8.0);
            contents(ui);
        });
}

fn credential_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &str,
    copy_label: &str,
    action: &mut DetailAction,
    on_copy: DetailAction,
) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(label).size(11.0).color(theme::TEXT_FAINT));
            ui.label(RichText::new(value).size(13.0).color(theme::INK));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::secondary_button(ui, copy_label).clicked() {
                *action = on_copy;
            }
        });
    });
    ui.add_space(6.0);
}

fn password_row(ui: &mut egui::Ui, password: &str, revealed: &mut bool, action: &mut DetailAction) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new("Password").size(11.0).color(theme::TEXT_FAINT));
            let shown = if *revealed { password.to_string() } else { "•".repeat(password.chars().count().max(8)) };
            ui.label(RichText::new(shown).size(13.0).color(theme::INK).family(egui::FontFamily::Monospace));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::secondary_button(ui, "Copy").clicked() {
                *action = DetailAction::CopyPassword;
            }
            if theme::secondary_button(ui, if *revealed { "Hide" } else { "Reveal" }).clicked() {
                *revealed = !*revealed;
            }
        });
    });
    ui.add_space(6.0);
}

fn totp_code_row(ui: &mut egui::Ui, code: &str, seconds_left: u8, action: &mut DetailAction) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new("One-time code").size(11.0).color(theme::TEXT_FAINT));
            ui.label(
                RichText::new(code)
                    .size(17.0)
                    .family(egui::FontFamily::Monospace)
                    .color(theme::INK),
            );
            let (rect, _) = ui.allocate_exact_size(egui::vec2(96.0, 4.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, CornerRadius::same(2), theme::HAIRLINE);
            let fraction = (seconds_left as f32 / 30.0).clamp(0.0, 1.0);
            let filled = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * fraction, rect.height()));
            ui.painter().rect_filled(filled, CornerRadius::same(2), theme::BLUE);
            ui.label(RichText::new(format!("{seconds_left}s left")).size(10.0).color(theme::TEXT_GHOST));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::secondary_button(ui, "Copy").clicked() {
                *action = DetailAction::CopyTotp;
            }
        });
    });
}

/// The One-time code row for `TotpState::Fetching`: this item has a TOTP
/// secret and a poll for its current code is already on its way, just not
/// back yet. Keeps the row's label in place, the same shape `Unavailable`'s
/// row does, but reads as an ordinary in-progress state rather than a
/// problem -- this is the everyday, usually sub-second case right after
/// selecting an item, not a backend issue.
fn totp_fetching_row(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new("One-time code").size(11.0).color(theme::TEXT_FAINT));
            ui.label(
                RichText::new("Fetching\u{2026}")
                    .size(13.0)
                    .color(theme::TEXT_SECONDARY),
            );
        });
    });
}

/// The One-time code row for `TotpState::Unavailable`: this item has a TOTP
/// secret, but the last attempt to fetch its current code couldn't reach
/// `bw serve`. Keeps the row's label in place (so the item still visibly
/// *has* one-time codes) without a code, a countdown, or a Copy button --
/// there is nothing valid to show or copy right now, and a countdown here
/// would falsely suggest a code is still live. Wording is a plain status,
/// not an alarm: this is very likely `bw serve` still starting up or a
/// transient hiccup, not something the user needs to act on.
fn totp_unavailable_row(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new("One-time code").size(11.0).color(theme::TEXT_FAINT));
            ui.label(
                RichText::new("Unavailable right now")
                    .size(13.0)
                    .color(theme::TEXT_SECONDARY),
            );
            ui.label(
                RichText::new("Couldn't reach the vault to get the current code.")
                    .size(10.0)
                    .color(theme::TEXT_GHOST),
            );
        });
    });
}

/// The One-time code row for `TotpState::NoCodeReported`: this item's own
/// login data carries a TOTP seed, but `bw serve` answered -- successfully --
/// that it has no current code for it. That is a *disagreement* between the
/// item we hold and the backend, not "this item has no 2FA", and it is the
/// only thing it can be at the live call site: `get_totp` is never called for
/// an item without a seed (see `vault_window::mod`'s per-frame TOTP block).
///
/// Same shape as `totp_unavailable_row`, deliberately, and same reason: the
/// row has to stay put so the pane is not pixel-identical to an item that
/// never had 2FA -- the reading that had a previous reviewer point out a user
/// would conclude TOTP was not set up and needlessly re-enrol. Different
/// wording, though, because it is a different fact: `Unavailable` means the
/// vault could not be reached and the app is still trying; this one means the
/// vault was reached and had nothing to give. The hint names the usual cause
/// (the seed changed elsewhere and this copy of the item predates it) and the
/// one action that resolves it, since -- unlike `Unavailable` -- this state
/// deliberately stops polling.
fn totp_no_code_row(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new("One-time code").size(11.0).color(theme::TEXT_FAINT));
            ui.label(
                RichText::new("No code available for this item")
                    .size(13.0)
                    .color(theme::TEXT_SECONDARY),
            );
            ui.label(
                RichText::new(
                    "The vault has no current code for it. If its authenticator key was \
                     changed on another device, Sync to pick that up.",
                )
                .size(10.0)
                .color(theme::TEXT_GHOST),
            );
        });
    });
}

/// Days between an RFC3339 `revisionDate` (as `bw serve` sends it) and now.
/// `None` on anything unparseable -- the caller shows "Updated recently"
/// rather than a wrong number.
fn days_since(revision_date: &str) -> Option<i64> {
    // A minimal RFC3339 date parse: only the `YYYY-MM-DD` prefix is needed
    // for a day-granularity "N days ago", so this avoids pulling in a full
    // datetime crate for one field. `std::time::SystemTime` supplies "now".
    let date_part = revision_date.get(0..10)?;
    let mut parts = date_part.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    let revision_days = days_from_civil(year, month, day);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    let today_days = (now.as_secs() / 86400) as i64;

    Some((today_days - revision_days).max(0))
}

/// Howard Hinnant's civil-from-days algorithm, days-from-civil direction:
/// converts a (year, month, day) into a day count since the Unix epoch,
/// without pulling in a datetime crate for one field.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review 14's Important, at the render layer. `NoSecret` is the *only*
    /// state that may draw nothing, because drawing nothing is the exact
    /// pixels of "this item has no 2FA" -- and every other state belongs to
    /// an item that demonstrably does have a seed.
    #[test]
    fn no_secret_is_the_only_state_that_omits_the_one_time_code_row() {
        assert_eq!(totp_row_for(&TotpState::NoSecret), None);

        for state in [
            TotpState::Fetching,
            TotpState::Code { code: "123456".to_string(), seconds_left: 12 },
            TotpState::Unavailable,
            TotpState::NoCodeReported,
        ] {
            assert!(
                totp_row_for(&state).is_some(),
                "{state:?} belongs to an item that HAS a TOTP seed, so omitting the row \
                 renders it pixel-identically to an item with no 2FA at all"
            );
        }
    }

    /// `NoCodeReported` and `Unavailable` are two different messages, not one
    /// row wearing two labels: the first is "the backend answered and has no
    /// code for this", the second is "the backend could not be reached".
    /// Keeping them visually distinct is the property review 8 established
    /// for `Unavailable` and must survive `NoCodeReported` gaining a row.
    #[test]
    fn no_code_reported_and_unavailable_render_as_different_rows() {
        assert_ne!(
            totp_row_for(&TotpState::NoCodeReported),
            totp_row_for(&TotpState::Unavailable)
        );
    }

    #[test]
    fn metadata_line_pluralizes_fill_count() {
        assert_eq!(
            metadata_line(Some(3), 41, "Tr0ub4dor&3xtraLong!"),
            "Updated 3 days ago \u{b7} Filled 41 times \u{b7} Strength: Strong"
        );
        assert_eq!(
            metadata_line(Some(1), 1, "weak"),
            "Updated 1 day ago \u{b7} Filled 1 time \u{b7} Strength: Weak"
        );
    }

    #[test]
    fn metadata_line_handles_missing_update_date() {
        assert_eq!(
            metadata_line(None, 0, ""),
            "Updated recently \u{b7} Filled 0 times \u{b7} Strength: Weak"
        );
    }

    #[test]
    fn metadata_line_handles_today() {
        assert_eq!(
            metadata_line(Some(0), 5, "abc"),
            "Updated today \u{b7} Filled 5 times \u{b7} Strength: Weak"
        );
    }
}
