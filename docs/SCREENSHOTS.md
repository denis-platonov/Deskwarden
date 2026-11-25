# Screenshots

Every image here is rendered by `cargo run --example ui_preview -- --all`, the
same example CI runs on every push.

**They are fixtures.** The example's own documentation is explicit: no surface
it draws reads a real vault, touches the network, or spawns `bw`. That is what
makes it safe to publish pictures of a password manager's UI at all — these
show the real interface and nobody's real logins.

The full set is 152 surfaces, including every error state, empty state
and spinner. It is uploaded as the `ui-screenshots` artifact on
[any CI run](https://github.com/denis-platonov/Deskwarden/actions/workflows/ci.yml).
The six below are the ones worth looking at first.

## The vault

![The vault](screenshots/vault_window.png)

*The whole window, rendered in one piece: the navy rail, the list and a
card's detail pane. The card face takes its bank's or its network's colour,
and its bank is named from the login it is linked to.*

## One item

![An item](screenshots/detail_login.png)

## Password health

Weak, reused, old and breached passwords in one place. Every distinct password
is checked against Have I Been Pwned's k-anonymity range
API — the password never leaves the machine, only a five-character hash prefix
does.

![Password health](screenshots/vault_password_health.png)

## Adding a two-factor code

![Adding a two-factor code](screenshots/totp_add_picker.png)

## Settings

![Settings](screenshots/prefs_clipboard.png)

## Signing in

![Signing in](screenshots/login_signin.png)
