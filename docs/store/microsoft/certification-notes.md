# Microsoft Store: notes for certification testers

Paste into Partner Center -> *Submission options* -> **Notes for
certification**. Plain text, no secrets.

```
HOW TO TEST DESKWARDEN

Deskwarden is an unofficial autofill companion for Bitwarden-compatible
password vaults. It is not made by Bitwarden, Inc. It fills saved logins
into other Windows programs' sign-in boxes.

TEST ACCOUNT
 Server: <<test server URL, e.g. a self-hosted instance the owner keeps for this>>
 Email:  <<test account email>>
 Master password: <<test master password>>
 (A throwaway account holding fake logins only. Fill these in at submission
 time from the owner's password manager; they are deliberately NOT stored in
 this repository.) Sign-in works with the built-in client on a self-hosted
 server; no other software is needed for that.
 Accounts on bitwarden.com / bitwarden.eu additionally require Bitwarden's
 own CLI ("bw"), which the user installs themselves; this Store edition never
 downloads programs. Please test with the self-hosted account above.

QUICK TEST
 1. Launch Deskwarden. Choose "Self-hosted", enter the server and the test
    account, sign in. The vault window opens.
 2. Open a test target: <<name a simple program with a sign-in box that the
    testers have, or use the built-in practice window: open a vault item,
    edit its sign-in steps, and press "Test run">>.
 3. Match the program to a vault item (Settings -> Apps, "Add app"), choose
    the "hotkey" trigger, focus the program's sign-in box and press
    Ctrl+Alt+B. The username and password are typed into the box.

WHY THE APP DECLARES WHAT IT DECLARES
 runFullTrust: Deskwarden is a Win32 desktop program. It must run outside the
   AppContainer because its whole purpose is to work with OTHER programs'
   windows (below), to keep a tray icon, and to run a background process
   that holds the unlocked vault, which an AppContainer app cannot do.
 UI Automation (no manifest entry; a normal Win32 API): Deskwarden asks
   Windows' accessibility interface two questions about the window in front:
   which program it belongs to, and whether it contains a masked password
   box. It does not read the contents of any box. This is how it knows when
   to offer a login. It is the same interface screen readers use.
 SendInput: the username and password are typed into the focused box as
   keystrokes, deliberately NOT through the clipboard (which every program
   can read). Nothing is typed until the user presses the hotkey, picks a
   login, or has chosen "fill on focus" for that program.
 Run at login: a startup task, DISABLED by default. The user turns it on
   (Windows Settings -> Apps -> Startup, or from Deskwarden's own settings). It starts the tray icon so
   the hotkey works without opening the app first.
 Background process: a small helper process, started by Deskwarden and
   ended with it, holds the vault in memory so autofill is instant. In
   accounts that use Bitwarden's CLI, the helper runs the user's own "bw"
   program on localhost only.
 internetClient / privateNetworkClientServer: the user's own vault server
   (which may be on the home or office network), the optional icon service
   and the optional breach check. Every one has a switch in Settings. No
   data goes to the developer; there are no servers, accounts or analytics.
 webcam: only the screen "Add a one-time code -> Use a webcam", used to read
   a QR code. The camera is off everywhere else and its light is on while
   used. No picture is stored or sent.

UPDATES
 This Store edition does not check for, download or install updates itself
 and never downloads programs: updates come from the Microsoft Store.
 (Settings -> Updates says so.)

KNOWN BEHAVIOURS THAT ARE NOT BUGS
 - Deskwarden cannot type into a program running as administrator while it
   runs as a normal user (Windows blocks this); it says so.
 - Some programs ignore synthetic keystrokes on purpose; Deskwarden then
   offers the copy buttons instead (copied secrets are cleared after a
   minute).
```

## Before pasting: the owner's checklist

- Replace every `<<...>>` placeholder. The credentials are for a **throwaway
  account on a server the owner controls**, never a real vault; rotate them
  after certification.
- Confirm the "Test run" practice window (the `rehearsal` surface, in the
  item's sign-in steps editor) exists in the build submitted; otherwise name a
  plain test program.
- Keep the server reachable for the whole certification window (days).
