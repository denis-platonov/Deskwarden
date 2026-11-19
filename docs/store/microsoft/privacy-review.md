# Microsoft Store: review of `PRIVACY.md` against Store requirements

Reviewed 2026-11-19 against the Microsoft Store Policies' privacy requirements
as understood then (an app that accesses, collects or transmits personal
information needs a privacy policy: a link in Partner Center *and* reachable
from inside the app; it must say what is collected, how it is used, stored and
shared, with whom, and how to contact the publisher or have data removed).
Re-read the live policy text at submission time; numbering and wording move.

## What `PRIVACY.md` already does well

- States plainly there is no developer-side collection, analytics, crash
  reporting or telemetry.
- Lists every network destination, what each receives, and the switch that
  turns each off.
- Explains the sensitive local capabilities in plain words: UI Automation
  (what it reads and, as important, what it does not), the camera, screen
  capture, clipboard handling.
- Has a contact address, a "Children" section and a "Changes" section.

## Gaps to close before submitting (in order of weight)

1. **The Store edition is not described, and the text promises it will be.**
   Section 4 says "Should one ever ship, this section will say what it
   actually does before it does it." The Store edition ships: add the
   sentence that in the Microsoft Store edition **there is no update check at
   all (the Store updates the app) and no request to GitHub**, and that the
   Updates page says so. *Must change before submission.*
2. **The CLI download (section 5)** must say the Store edition **never
   downloads the Bitwarden CLI**; accounts on bitwarden.com / bitwarden.eu
   use a CLI the user installed. (Same edit as 1.)
3. **A false-looking claim.** Section 4 says the installer "is verified
   against Deskwarden's signing certificate before anything can launch it".
   The release workflow, `updater.rs` and `docs/code-signing-policy.md` say
   builds are **unsigned** and verified by a SHA-256 digest GitHub publishes.
   A policy that overstates a safeguard is a worse defect than one that omits
   it. Correct the sentence (it concerns the direct-download build, so keep it
   there, accurately worded). *Fix regardless of the Store.*
4. **No in-app link.** The Store expects the privacy policy to be reachable
   from within the app (typically Settings -> About). Nothing in `src/` shows
   a "Privacy policy" link to the public URL (`PRIVACY.md` is referred to in
   code comments only). Add a one-line link in the About page. This is a
   `prefs_ui` change and so is **not** made here; it is listed in
   `CHECKLIST.md` as a code follow-up. *Must change before submission.*
5. **Where the data lives under MSIX.** The policy says "deleting
   `%APPDATA%\Deskwarden` removes it". In the packaged app that folder is the
   package's virtualised copy and is removed by **uninstalling the app**. Add
   one sentence for the Store edition: uninstalling removes it.
6. **A stable, public URL.** Partner Center wants a URL that renders without
   signing in and does not move. `github.com/.../blob/main/PRIVACY.md` works
   but is a rendered Markdown file on a branch name; a page on a domain the
   owner controls is better. The text in the repository should remain the
   source of truth.
7. **"Last updated" date** (2026-09-10) predates the 1.0 redesign and the
   edits above; bump it with them.
8. **What "personal information" the Store form sees.** If Partner Center asks
   whether the app accesses personal information, the honest answer is yes
   (it handles the user's passwords and, optionally, camera and screen
   pictures *locally*), with the policy URL supplied. The policy already
   explains that none leaves the machine except to the user's own server;
   keep that sentence prominent.
9. **Third-party recipients.** Section "What leaves your machine" is the
   list of recipients the Store form may ask for: `icons.bitwarden.net`
   (a domain name), `api.pwnedpasswords.com` (five hash characters), the
   user's own vault server, and sites from the user's vault (icons, off by
   default). In the Store edition GitHub is no longer a recipient.

## Not gaps (checked)

- Children: "not directed at children", no personal information collected:
  consistent with the IARC answers (`age-rating-iarc.md`).
- Contact: `denis@napps.pw` and the issue tracker. The owner must be willing
  to publish that address in the listing; otherwise change both.
- Camera / screen capture disclosure: present and specific.
- No advertising, no sale or sharing of data: stated.
