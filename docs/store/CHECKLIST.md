# Store submission checklist (owner's steps, in order)

Everything marked **[you]** needs an account, a sign-in, a payment or a click
that Claude and CI are not allowed to do. Everything else is a command or a
file already in the repository. Nothing here has been done yet.

Related: [`microsoft/packaging.md`](microsoft/packaging.md) (how the package
is built), [`microsoft/listing.md`](microsoft/listing.md) (the text),
[`macos.md`](macos.md).

## Decisions first

- **D1. Which accounts does the Store edition serve?** Today (see
  packaging.md, Rulings): self-hosted servers work with the built-in client;
  bitwarden.com / bitwarden.eu accounts need a Bitwarden CLI the user installs
  themselves, because the Store edition may not download programs. Decide
  whether that is acceptable. The alternative, routing cloud accounts through
  the built-in client, needs real testing against bitwarden.com first and is
  not in this branch.
- **D2. The product name.** "Deskwarden" sits near Bitwarden's trademark
  (listing.md, "Product name"). Decide whether to reserve it as is.
- **D3. Support address.** `PRIVACY.md` publishes `denis@napps.pw`; the Store
  listing shows the support contact publicly.

## Microsoft Store (Windows)

**M0. Code follow-ups before the build you submit** (small, listed in
packaging.md and privacy-review.md; none is in this branch):
- [ ] Add the Updates-page hook: `updater::store_updates_notice()` in
      `src/prefs_ui` (a few lines), so the page says "Updates come from the
      Microsoft Store" instead of showing the check and download controls.
- [ ] Add a "Privacy policy" link to the About page (Store policy: reachable
      from inside the app).
- [ ] Make autofill start work in the package: `autostart_repair` writes the
      `Run` registry key, which a packaged app cannot use; offer the manifest's
      startup task instead, and treat a startup-task activation like
      `--autostart` (a startup task launches with no arguments).
- [ ] Walk `cargo test --lib --features store` once more on the final tree.

**M1. Fix the privacy policy** (privacy-review.md, gaps 1-7), publish it at a
stable public URL, and put that URL in `listing.md`.

**M2. [you] Create the Partner Center developer account.**
`partner.microsoft.com/dashboard` -> Windows & Xbox. Individual accounts pay a
one-time registration fee (about US$19 at the time of writing; the page shows
the current figure; company accounts cost more and need verification).
Identity verification can take days. *Claude cannot do this step.*

**M3. [you] Reserve the app name.** Partner Center -> Apps and games -> New
product -> MSIX or PWA app -> reserve "Deskwarden" (or the name chosen in D2).
Then open *Product management -> Product identity* and copy three values:
`Package/Identity/Name`, `Package/Identity/Publisher` (`CN=...`) and
`Package/Properties/PublisherDisplayName`.

**M4. Build the package** (PowerShell, repository root; needs Rust, Python and the
Windows SDK's `makeappx.exe` as described in packaging.md):

```powershell
scripts\store\build-msix.ps1 `
  -PackageName '<Package/Identity/Name>' `
  -Publisher '<Package/Identity/Publisher>' `
  -PublisherDisplayName '<PublisherDisplayName>'
```

- [ ] Output: `store-package\Deskwarden_<version>_x64.msix` (unsigned; the Store
      signs it) and `store-package\generated-assets\listing\*.png`.
- [ ] Optional: sideload-test first (packaging.md, "Building the file to
      upload"), with the test-certificate-signed copy, on your own PC. Test:
      launches, tray icon, sign in, autofill into a demo app, the startup
      task appears under Windows Settings -> Apps -> Startup, uninstall is
      clean.

**M5. Take screenshots** (screenshots.md): run `cargo run --example ui_preview
-- --all`, pick and upscale the listed surfaces, take the two real autofill
captures on a test profile.

**M6. [you] Fill the submission in Partner Center.**
- [ ] *Pricing and availability*: Free; markets; visibility.
- [ ] *Properties*: category (Productivity or Utilities and tools), the
      privacy policy URL, support contact, "system requirements" (Windows 10
      1809+, 64-bit; webcam optional).
- [ ] *Age ratings*: run the IARC questionnaire with `microsoft/age-rating-iarc.md`.
- [ ] *Packages*: upload the `.msix` from M4. Confirm the declared
      capabilities show `runFullTrust`; the form asks you to justify restricted
      capabilities, paste the "WHY THE APP DECLARES..." paragraph from
      certification-notes.md.
- [ ] *Store listings* (English): name, description, features, search terms,
      what's new, logos and screenshots, copyright and trademark line, website,
      support, from `microsoft/listing.md`.
- [ ] *Submission options -> Notes for certification*: certification-notes.md,
      with the real test account on a throwaway server (never a real vault).
- [ ] Publish date: "as soon as possible" or manual release.

**M7. [you] Submit for certification** (the *Submit to the Store* button) and
watch the email for the result (typically hours to a few business days). If
it fails, the report names the policy; the two likeliest are the full-trust
capability (answer with certification-notes.md; the fallback route is an
EXE/MSI listing, which needs a code-signing certificate first) and the
privacy-policy link inside the app (M0).

**M8. After it goes live**: smoke-test the Store install; keep the CI artifact
(`deskwarden-store-package-unsubmittable`) as a build-health signal only. Each later
release: bump the version, rebuild with M4, upload a new package to a new
submission.

## macOS (not the Mac App Store)

See [`macos.md`](macos.md). The Mac App Store is very likely impossible for
the full app; the route is Developer ID + notarization + DMG, after the Mac
build exists (Mac plan, phase M7). Nothing to do before then except, optionally:

- [ ] **[you]** Enrol in the Apple Developer Program (US$99/year;
      `developer.apple.com/programs`, requires an Apple ID with two-factor
      authentication and identity verification; organisations also need a
      D-U-N-S number).
- [ ] **[you]** When the Mac build exists: create the Developer ID Application
      certificate, an App Store Connect API key for notarization, and choose
      the permanent bundle identifier.
- [ ] Decide Mac open decisions 1 (Sparkle vs own updater) and 2 (list a
      sandboxed vault-only build: recommended no).
