# The app account picker

**Status:** designed, not started. Supersedes the "overlay light states in Win32"
line item by giving it a first concrete surface.

## The problem

Press `CTRL+ALT+B` on an app you have an account for but have never bound, and
Deskwarden says it has nothing. That is true only in the narrowest sense:
`MatchEngine::lookup` answers `Option<(&str, &AppMatch)>` -- one item or
nothing -- and the `AppMatch` is a field written onto the vault item when the
user configures the app. An account that exists in the vault but carries no
such field is invisible to the engine, so the user is offered *New login* for a
login they already have, or *Search vault*, which opens the ~102 MB window to
find something the daemon could have offered in place.

## The shape

`CTRL+ALT+B` keeps its meaning and its chord. When the configured lookup misses,
the card that appears is no longer a dead end but a two-step picker:

1. **Candidates for this app** -- a list of vault items that plausibly belong to
   the foreground window, found by a looser match than the engine's.
2. **What to send** -- having chosen an item, the user chooses which field is
   typed: username, password, TOTP, a custom field, all, or the item's own
   sequence when it has one.

*Edit record* remains available and hands off to the egui vault window.

## The renderer, and why this moves a surface

**This card is bare Win32, in the daemon.** Measured against the numbers in
`2026-08-21-daemon-and-ui-split-design.md`: the unlock prompt's process is
**1.79 MB** with its window on screen, against ~102 MB for any eframe window on
the `glow` renderer.

The `wgpu` alternative was probed on 2026-08-22 and is **blocked, not rejected**:
`wgpu-hal 29.0.4` fails to compile against the `windows` crate it requires, at
both 0.62.2 and 0.62.0, in a `windows-core` unification failure that has nothing
to do with this crate's own `windows 0.58`. The comment at `Cargo.toml:102`
attributing the `glow` choice to a clash with *this app's* pin is **wrong** and
should be corrected when someone next touches it. A measured comparison exists:
a wgpu/D3D11 egui app on the same machine sits at 39.8 MB working set, loading
`dxgi.dll` and `d3d11.dll` but not `nvoglv64.dll`. If wgpu becomes buildable,
that changes the arithmetic for the *vault window*; it does not change this
card, which is small enough to hand-draw and frequent enough to be worth it.

**Design 3a moves.** The no-match card exists in egui today. The split's rule is
that every surface lives in exactly one renderer -- a card drawn twice is the
"two things that must agree" defect on the surface that types passwords. So this
work *relocates* 3a to Win32 and deletes the egui one, rather than adding a
second. The egui overlay retains only the states that stay rich: the save-login
form (3c) and the generator (3d).

**Fidelity is already demonstrated.** The unlock prompt paints with `RoundRect`
into a double-buffered DC and reads `theme::CARD`, `theme::INK`,
`theme::TEXT_FAINT`, `theme::FIELD_HEIGHT` and `theme::BUTTON_HEIGHT` from the
same module egui reads, and registers `theme::ARCHIVO_FACES` privately with
`AddFontMemResourceEx` -- the same bytes, not a second copy. A theme change moves
both renderers at once.

**Direct2D was measured and rejected.** GDI cannot antialias a rounded corner
or match egui's text rasterization, and Direct2D/DirectWrite can do both, so it
was spiked on 2026-08-22: one window, one rounded rectangle, one line of system
font. It cost **53.85 MB private / 44.45 MB working set** and loaded `d2d1.dll`,
`dwrite.dll`, `dxgi.dll`, `d3d11.dll` and `nvgpucomp64.dll`. Direct2D is not a
CPU rasterizer -- it sits on Direct3D, so it pulls in the GPU stack, and it
lands within a few MB of the wgpu figure above because it is the same thing
underneath. That is 30x the GDI prompt for a rectangle and a word. **Do not
re-try this**; the DirectWrite custom-font work the quality comparison needed
was never written, because the cheap half of the spike disqualified it.

The corollary is worth keeping: two independent measurements on this machine now
agree that a D3D-backed renderer costs ~40-55 MB against ~102 MB for OpenGL. The
daemon cannot afford either. The vault window can, and roughly halves if the
wgpu build problem is solved.

**What is not yet on-design:** any control painted by Windows rather than by us.
The prompt's *Cancel* is a stock button -- grey gradient, system font, square
corners -- beside a correctly drawn *Unlock*. The fix is owner-draw
(`WM_DRAWITEM`), and this feature needs it for the list rows regardless, so the
button comes along with it. Accepted as-is for now by the owner's decision;
tracked, not forgotten.

## Candidate matching

A new pure function over the vault snapshot, separate from `MatchEngine`, which
keeps its exact single-answer semantics for configured apps.

Input: the `ForegroundEvent` (exe name, window title) and the items. Output: a
ranked `Vec` of items, strongest first.

Signals, in descending confidence:

- the exe's stem appears in an item URI's host (`slack.exe` -> `slack.com`)
- the exe's stem appears in the item name, case-insensitively
- a significant word of the window title appears in the item name

Ranking exists so the list is worth reading top-down; it is not a confidence
score and nothing is auto-filled from it. **Nothing is ever typed without an
explicit choice**, which is what keeps a loose matcher safe: a wrong candidate
costs a glance, never a keystroke into the wrong field.

Being pure and window-free, this is testable directly -- fixtures in, ranked
list out -- which is where the coverage for this feature should concentrate.

## Step one: the list

An owner-drawn listbox (`LBS_OWNERDRAWFIXED`). Each row: the favicon from the
existing cache, the item name, and the username, with hover and selection
painted in theme colours.

**Bounded height.** The overlay is frameless, always-on-top and has no
`ScrollArea` anywhere, because a control past the bottom edge is unreachable.
The list therefore shows a fixed maximum number of rows, and when there are more
candidates than that, the last row is a *Search vault* handoff rather than a
silently truncated list. **A cap that hides candidates without saying so is the
defect this project keeps finding**; the overflow row is what makes the cap
honest.

Empty candidate list: the card falls back to today's 3a content -- *New login*
and *Search vault* -- unchanged.

**Usernames are shown in full.** They are already visible on the matched-item
card (design 2a shows the username so the user can recognise what is about to be
filled), the window is excluded from capture, and a list of masked usernames
cannot be chosen between, which defeats the feature. Passwords are never shown
at any point in this flow.

## Step two: what to send

`key_sequence::field_palette(item)` already computes exactly this list --
username, password and TOTP when non-empty, then every custom field, skipping
Deskwarden's own `deskwarden:`-prefixed ones. It is built and tested for the
sequence editor and is reused verbatim.

Presented as buttons, plus:

- **All** -- defined as `{USERNAME}{TAB}{PASSWORD}`, **with no trailing Enter.**
  A trailing Enter submits, and if the target's field order differs from the
  assumption the submission happens anyway with the wrong content in the wrong
  box. Typing without submitting fails visibly and harmlessly; submitting fails
  invisibly. This is a default, not a law -- if it proves annoying the place to
  revisit it is here, not in the injector.
- **Sequence** -- shown only when the item carries one, and run through the
  existing resolver rather than a second interpretation of the same string.

Dispatch goes through the existing injector, keeping its foreground check: if
the target window is no longer in front, the send is refused rather than typed
blind. That check is why the picker may take as long as the user likes.

## Secrets

The password is fetched at the moment of dispatch by the component that already
holds the plaintext, and is not carried on the selection. This is
`DetailAction`'s existing discipline -- name the item, let the holder fetch --
and the reason is that a copy carried on a selection becomes a second,
non-zeroizing home for the secret that lives as long as the card does.

The candidate list holds item ids and display strings. It does not hold
passwords.

## Testing

The house defect is "a test that passes because it never reached the thing it
names", so:

- **Candidate matching** is pure and gets real coverage: ranking order, the
  no-candidates case, and a case where the configured engine would have matched
  (which must never reach this path).
- **The two-step decision** follows the `PromptCalls` idiom the unlock prompt
  established -- Win32 as `fn` pointers, the decision in a `run_with` that a
  test can drive without a desktop. What is typed for each choice is asserted
  there, including that *All* emits no Enter.
- **The overflow row** is asserted to appear exactly when candidates exceed the
  cap, because a silent truncation is the failure this design is guarding
  against.
- **No test touches** the network, the real vault, the clipboard, the screen,
  `%APPDATA%\Deskwarden`, a real dialog, or spawns `bw`.
- **A preview** is added following `examples/unlock_prompt_preview.rs`, so the
  card can be looked at, and screenshotted, without a locked vault. Win32
  surfaces are absent from `examples/ui_preview.rs`, which walks egui surfaces
  through one `run_native`; a state nobody renders is a state nobody looks at.

## Explicitly out of scope

- Writing an `AppMatch` from the picker ("always use this account for this app").
  It is the obvious next request and the design leaves room for it, but binding
  an app is a configuration act and belongs with the rest of configuration.
- Any change to how configured matches behave.
- The `wgpu` renderer investigation, which is its own piece of work.
