# Product Documentation Site Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A public documentation site for Deskwarden at `docs.<domain>`, with URLs shaped like `docs.frappe.io/erpnext/introduction`, deployed from this repository — and load-bearing facts pinned against the code so the docs cannot quietly go stale.

**Architecture:** Astro Starlight generates a static site into `docs-site/dist`. **Cloudflare Workers Static Assets** serves it — not Pages. The site lives in this repository so a feature change and its documentation travel in one commit, and deploys on push to `main` when `docs-site/**` changes, independently of release tags.

**Tech Stack:** Astro + Starlight, Node/pnpm for the doc build only (the app itself remains pure Rust), Wrangler, Cloudflare Workers Static Assets.

## Global Constraints

- **Workers Static Assets, not Cloudflare Pages.** Cloudflare's own migration guide (retrieved 2026-08-13, page dated 2026-08-14) directs new projects to Workers: same cost — static asset requests are free on both — and a broader feature set. **Workers Sites is deprecated in Wrangler v4; do not use it.**
- **Verify the recommendation still holds at execution time.** Cloudflare moves faster than any plan. Search the live docs before scaffolding rather than trusting this paragraph.
- **The Rust crate must not gain a Node dependency.** `docs-site/` is a sibling with its own `package.json`. `cargo build` and `cargo test` must not require Node.
- **No secret, key, token or vault content in any doc page.** `login_ui.rs`'s probe scan walks **every tracked file in the repository**, not just `.rs` — a doc page containing the assembled probe needle will red the suite, which is the mechanism working.
- **Never build into `deskwarden/target`.** Fresh `CARGO_TARGET_DIR` outside the repo for any Rust run; confirm each prints its own `Compiling deskwarden`.
- **Commit with explicit paths** and `-F` a message file, never a PowerShell here-string. Never `git stash` (two pre-existing stash entries must survive), `git add -A`, `--amend`, `reset`, `rebase`.
- **Provisioning needs an interactive session.** The Cloudflare MCP servers (`cloudflare-api`, `cloudflare-builds`, `cloudflare-bindings`, `cloudflare-observability`) require OAuth, which cannot be completed in a non-interactive run. Authorize via claude.ai connector settings or `claude mcp` **before** Task 2. Everything else in this plan works without them.

---

## File Structure

| File | Responsibility |
|---|---|
| `docs-site/package.json` *(create)* | Astro + Starlight deps. Node only; nothing here is read by cargo. |
| `docs-site/astro.config.mjs` *(create)* | Starlight config: site title, sidebar, social links, search. |
| `docs-site/wrangler.jsonc` *(create)* | Workers Static Assets config. Deliberately has **no** `main` and **no** `assets.binding`. |
| `docs-site/dist/.assetsignore` *(create, via build)* | `**/node_modules`, `**/.git`, `**/.DS_Store`. Workers does not auto-exclude these; Pages did. |
| `docs-site/src/content/docs/**.md` *(create)* | The pages. |
| `.github/workflows/docs.yml` *(create)* | Build and deploy on push to `main` touching `docs-site/**`. |
| `deskwarden/src/doc_pins.rs` *(create)* | Rust tests asserting doc pages agree with the constants they describe. |

---

## Task 1: Scaffold the site and prove it builds

**Files:**
- Create: `docs-site/package.json`, `docs-site/astro.config.mjs`, `docs-site/wrangler.jsonc`
- Create: `docs-site/src/content/docs/index.mdx`, `docs-site/src/content/docs/introduction.md`

- [ ] **Step 1: Scaffold Starlight**

```bash
cd docs-site && npm create astro@latest -- --template starlight --no-install --yes .
```

- [ ] **Step 2: Write the Wrangler config**

```jsonc
{
  "$schema": "./node_modules/wrangler/config-schema.json",
  "name": "deskwarden-docs",
  // Set this to the date you run it, not the date in this plan.
  "compatibility_date": "2026-08-13",
  "assets": {
    "directory": "./dist",
    // "404-page" and NOT "single-page-application": an SPA setting would
    // answer 200 with index.html for every unknown URL, so a typo'd or
    // retired doc path would silently render the home page instead of a 404.
    "not_found_handling": "404-page"
  }
}
```

**Do not add `"binding": "ASSETS"`.** That field is only valid when a `main` Worker script exists; this site has none, and including it is a configuration error rather than a harmless extra.

- [ ] **Step 3: Add `.assetsignore`**

Emit `dist/.assetsignore` as part of the build (Astro copies `public/` verbatim, so `docs-site/public/.assetsignore` is the simplest home):

```txt
**/node_modules
**/.DS_Store
**/.git
```

- [ ] **Step 4: Build and check the URL shape**

Run: `cd docs-site && npm install && npm run build`
Expected: `dist/introduction/index.html` exists — that is what serves at `/introduction` with no `.html` suffix, which is the URL shape the brief asks for.

- [ ] **Step 5: Preview locally**

Run: `cd docs-site && npx wrangler@latest dev`
Expected: the site serves, and an unknown path returns **404**, not the home page. If it returns the home page, `not_found_handling` is wrong.

- [ ] **Step 6: Confirm cargo is unaffected**

Run: `CARGO_TARGET_DIR=<outside-repo> cargo test -j 8` from `deskwarden/`
Expected: unchanged counts, 0 failed, 0 warnings. If `job_object`'s file-inventory test or `login_ui`'s probe scan reds, the new files have disturbed something — investigate rather than adjusting the guard.

- [ ] **Step 7: Commit**

```bash
git commit -F msg.txt docs-site/
```

---

## Task 2: Deploy, custom domain, and CI

**Prerequisite:** Cloudflare MCP OAuth completed, or a `CLOUDFLARE_API_TOKEN` available to CI.

**Files:**
- Create: `.github/workflows/docs.yml`

- [ ] **Step 1: First manual deploy**

Run: `cd docs-site && npx wrangler@latest deploy`
Expected: a `*.workers.dev` URL that serves the site.

- [ ] **Step 2: Attach the custom domain.** Add `docs.<domain>` as a Workers custom domain. Verify the certificate is live before wiring CI, so a broken deploy is not diagnosed as a DNS problem.

- [ ] **Step 3: Write the deploy workflow**

```yaml
name: Docs
on:
  push:
    branches: [main]
    paths: ["docs-site/**", ".github/workflows/docs.yml"]
  workflow_dispatch:
permissions:
  contents: read
jobs:
  deploy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - uses: actions/setup-node@v4
        with: { node-version: "22" }
      - run: npm ci
        working-directory: docs-site
      - run: npm run build
        working-directory: docs-site
      - run: npx wrangler@latest deploy
        working-directory: docs-site
        env:
          CLOUDFLARE_API_TOKEN: ${{ secrets.CLOUDFLARE_API_TOKEN }}
```

**`ubuntu-latest`, not `windows-latest`.** The app is Windows-only; the docs are not, and a Linux runner is faster and cheaper. **Path-filtered** so a Rust-only commit does not redeploy the site, and **not** gated on a release tag — a typo fix should not wait for a version bump.

- [ ] **Step 4: Verify the path filter.** Push a Rust-only change and confirm this workflow does **not** run.

- [ ] **Step 5: Commit.**

---

## Task 3: The pages

**Files:**
- Create: `docs-site/src/content/docs/*.md` and subdirectories

Write these, in this order — each is a question a real user asks:

- [ ] **Step 1: Introduction** — what Deskwarden is (a companion to Bitwarden that fills native Windows apps), what it is not (not a vault; `bw serve` holds the vault), and the Windows-only requirement.
- [ ] **Step 2: Install and first run** — installer, the unsigned-build warning while SignPath is pending, signing in, Windows Hello.
- [ ] **Step 3: Matching an app** — binding a vault item to an application, how matches are found, why a match is counted but never named while locked.
- [ ] **Step 4: Fill sequences** — the step builder, the template view, the grammar. **State the grammar exactly**: `{USERNAME}`, `{PASSWORD}`, `{TOTP}`, `{S:Name}`, `{TAB}`, `{ENTER}`, `{DELAY 1000}` to pause and `{DELAY=50}` for typing rate. Say plainly that modifier-plus-letter chords such as `^A` are **not** supported and are refused by name rather than mistyped — see `2026-08-13-send-hardening-followups.md`.
- [ ] **Step 5: Sending a record** — what a Send is, expiry, view limits, access password, and that revoking kills the link but cannot retract what was already opened.
- [ ] **Step 6: Preferences** — one section per settings group.
- [ ] **Step 7: Security and privacy** — where the vault lives, what leaves the machine, the HIBP k-anonymity behaviour, that the Send access URL contains its own decryption key, and what auto-lock does. **This is the page users will actually scrutinise; write it last, when the others have settled, and write it precisely.**
- [ ] **Step 8: Commit** after each page rather than in one batch.

---

## Task 4: Pin the load-bearing facts against the code

**Why:** Prose drifts. This repository already refuses to let that happen internally — `Cargo.toml` is pinned by content hash, refusal messages are pinned across file boundaries, seams are pinned by struct width. Documentation deserves the same treatment for the handful of facts that are *checkable*. Everything else is prose maintained by hand; these are not.

**Files:**
- Create: `deskwarden/src/doc_pins.rs`
- Modify: `deskwarden/src/lib.rs` (declare the module, `#[cfg(test)]`)

**Interfaces:**
- Consumes: `key_sequence::DEFAULT_SEQUENCE`; the settings description constants in `prefs_ui.rs` (`BACKEND_DESCRIPTION`, `BREACH_DESCRIPTION`, `AUTO_LOCK_DESCRIPTION`, `TOTP_SECRET_DESCRIPTION`).

- [ ] **Step 1: Write the failing test**

```rust
//! Documentation claims that are checkable against the code that makes them
//! true. Prose is maintained by hand; these are not.
#[cfg(test)]
mod tests {
    /// The fill-sequence page states the default sequence. If someone changes
    /// `DEFAULT_SEQUENCE` the page becomes a lie, and nothing else in the
    /// repository would notice.
    #[test]
    fn the_sequence_page_states_the_default_this_crate_actually_uses() {
        let page = include_str!("../../docs-site/src/content/docs/sequences.md");
        assert!(
            page.contains(crate::key_sequence::DEFAULT_SEQUENCE),
            "the fill-sequence page must quote `key_sequence::DEFAULT_SEQUENCE` verbatim; it is \
             `{}` and the page does not contain it",
            crate::key_sequence::DEFAULT_SEQUENCE
        );
    }

    /// Control: the pin is reading a real page, not an empty file that would
    /// satisfy nothing. A `contains` over an empty string fails, but a
    /// `contains` over a page that lost its section passes vacuously if the
    /// needle happens to appear in a nav blob -- so assert the page has body.
    #[test]
    fn the_pinned_pages_are_not_empty() {
        let page = include_str!("../../docs-site/src/content/docs/sequences.md");
        assert!(page.len() > 500, "the sequence page is a stub; the pin above proves nothing");
    }
}
```

- [ ] **Step 2: Run it and watch it fail.** Expected: `include_str!` cannot find the file until Task 3 Step 4 has created it, or the assertion fires because the page omits the default.

- [ ] **Step 3: Make the page quote the constant**, then re-run and watch it pass.

- [ ] **Step 4: Prove the pin bites.** Temporarily change `DEFAULT_SEQUENCE` to `{USERNAME}{ENTER}` and confirm the test **fails**. Revert. A pin that passes under a changed constant is decoration — this repository has shipped two such tests and both were found only by accident.

- [ ] **Step 5: Add the same treatment for the settings descriptions.** The prose in `prefs_ui.rs` (`BREACH_DESCRIPTION`, `AUTO_LOCK_DESCRIPTION`, `BACKEND_DESCRIPTION`, `TOTP_SECRET_DESCRIPTION`) is already user-facing English. Assert the Preferences page contains each verbatim, so the app and the manual cannot disagree about what a setting does.

- [ ] **Step 6: Do NOT pin prose that is only narrative.** A pin on a sentence someone will legitimately reword is a false red, and this repository's own history shows false reds get "fixed" by weakening the guard. Pin values and quoted constants; leave explanation alone.

- [ ] **Step 7: Run the full suite**, confirm 0 failed and 0 warnings, and **commit**.

---

## Notes for the implementer

- Clean URLs are automatic: an SSG emitting `sequences/index.html` serves at `/sequences`. No rewrite rules needed.
- If you later want a product segment in the path (`/deskwarden/introduction`, as `docs.frappe.io/erpnext/…` has), set Astro's `base` rather than nesting content directories — nesting changes every internal link.
- The docs build is the only Node in this repository. Keep it that way; if a task tempts you to add a JS step to the Rust build, that is a signal the task belongs on the docs side.
