# Deskwarden on macOS: store and distribution notes

No code here. This is the distribution half of
`docs/superpowers/plans/2026-11-19-mac-version.md`; it starts to matter at that
plan's phase M7, after a macOS build exists. Nothing in it can be done today
except buying the Apple Developer Program membership early if the owner wants
the paperwork out of the way.

## The Mac App Store is very likely not possible for the full app

Every Mac App Store app must run in the **App Sandbox** (the
`com.apple.security.app-sandbox` entitlement is mandatory for submission).
Deskwarden's defining feature is the opposite of what the sandbox allows:

| Deskwarden needs | In the sandbox |
|---|---|
| Read the focused field and window of **other apps** (Accessibility API, `AXUIElement`) | Not permitted. There is no sandbox entitlement that grants the Accessibility permission; the user can grant it only to a non-sandboxed app |
| Type into other apps (`CGEventPost` synthetic keyboard events) | Not permitted for the same reason: posting events into other processes needs the Accessibility permission, and App Review has historically rejected apps that control other apps this way |
| Notice which app is in front, its path and signer (`NSWorkspace`, `SecStaticCode` on another app's bundle) | Partly available (`NSWorkspace.frontmostApplication` works), but inspecting other bundles' code signatures and paths is restricted |
| Screen Recording for QR-from-screen | Possible with a user grant, but it is a second sensitive permission reviewers scrutinise |
| Run a bundled helper that holds the unlocked vault, plus a user-installed `bw` child process | Helper processes are allowed (XPC / login items), but executing a user-installed arbitrary binary is not |
| Self-update (Sparkle) | Not allowed in the store build: the Mac App Store updates apps |

So the **full app cannot be a Mac App Store app**, and App Review is very
likely to reject any attempt to declare around it. This is a judgement from
the rules as they stand, not a certainty: only a submission settles it, and
a rejection costs a review cycle, not money beyond the membership.

What would be possible is a **vault-window-only build** (sign in, browse, edit,
copy, generator, TOTP codes; no autofill into other apps, no helper). Whether
that is worth listing is an open decision for the owner (open decision 2 in
the Mac plan). It would be a different product in the store than the one users
download from the website, and it would compete with the official Bitwarden
app on exactly the features it keeps. **Recommendation: do not list it.**

## The route: Developer ID, notarization, DMG

The way desktop autofill tools distribute outside the store, and the plan
the Mac plan already takes:

1. **Developer ID Application certificate** (from the Apple Developer
   Program) signs the `.app` and every helper inside it.
2. **Hardened Runtime** on (`codesign --options runtime`), with only the
   entitlements Deskwarden needs (no sandbox; Apple Events / disable library
   validation only if a dependency demands it). Accessibility and Screen
   Recording are *user-granted privacy permissions* (System Settings), not
   entitlements.
3. **Notarization**: `xcrun notarytool submit ... --wait` with an App Store
   Connect API key or app-specific password, then `xcrun stapler staple`
   so the ticket travels with the app and Gatekeeper opens it offline.
4. **DMG** (a universal arm64 + x86_64 `.app`, Applications link, a signed and
   notarized disk image) attached to the GitHub release next to the Windows
   installer; the release workflow gets a `macos-14` job (Mac plan M7).
5. **Updates**: Sparkle 2 (EdDSA-signed appcast) or the app's own downloader
   replacing the `.app` (open decision 1 in the Mac plan). Both are allowed
   outside the store.
6. Optional: a **Homebrew cask** pointing at the DMG, once releases are stable.

Gatekeeper opens a notarized Developer ID app with at most the one-time
"downloaded from the internet" prompt. An unnotarized app is blocked by
default on current macOS, so notarization is not optional.

## What the owner must buy or create (Apple)

- **Apple Developer Program**, individual or organisation: **US$99 per year**
  (as of this writing; the enrolment page shows the current price). An
  individual enrolment uses the owner's legal name as the publisher; an
  organisation enrolment needs a D-U-N-S number and takes longer. Enrolment
  requires an Apple ID with two-factor authentication and identity
  verification.
- Inside the account: a **Developer ID Application** certificate (and, for the
  installer package route, Developer ID Installer), created from a certificate
  signing request made on a Mac; an **App Store Connect API key** (or an
  app-specific password) for `notarytool`; a registered **bundle identifier**
  (for example `app.deskwarden.mac`, owner's choice: it is permanent).
- A **Mac** to build and sign on (or a `macos-14` GitHub runner holding the
  certificate and API key as repository secrets; the owner decides whether to
  put signing secrets in CI).

None of this is needed for the Mac App Store route that this document
recommends against; the same membership covers both.

## Listing kit items that carry over

The Windows kit (`docs/store/microsoft/`) is reusable for the website, the
GitHub release and any Homebrew / directory listings:

| Item | Carries over? |
|---|---|
| Name, short and long description, feature bullets | Yes, with the Windows-specific words changed (taskbar -> menu bar, Ctrl -> Command, Windows Hello -> Touch ID) |
| Keywords | Yes |
| "What's new" | Yes (from `CHANGELOG.md`) |
| Support / privacy URLs | Yes; **`PRIVACY.md` needs a macOS section** (Accessibility and Screen Recording permissions, Keychain instead of DPAPI, Touch ID) |
| Trademark / "works with Bitwarden" wording | Yes, unchanged: no Bitwarden name in the app name, icon or publisher, "unofficial" up front |
| IARC age rating | Only for a Mac App Store listing, which is not planned; a website needs none |
| Certification notes | Reusable as a "why these permissions" page in the first-run flow and docs; Apple does not ask for them outside App Review |
| Screenshots | Re-take on macOS (window chrome, menu bar, Command-key hints); fixtures from `ui_preview` carry over once the UI crate is shared |
| Icon | The shield mark is reusable; macOS wants an `.icns` / asset catalog at 16-1024 px, generated the same way as the Windows icon (script, not hand-drawn) |

## Permissions the first run must explain (they are review-free but user-facing)

Accessibility (autofill) and Screen Recording (QR from screen) are granted by
the user in System Settings -> Privacy & Security. The app must say why before
asking and deep-link to the pane. A notarized Developer ID app can request
both; the user sees the same system dialogs any such tool shows.
