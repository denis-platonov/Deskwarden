# The Local Vault Service

**One process holds the vault. Everything else is a client.**

## Why this exists now

Three consumers, not two:

1. the daemon (matching, filling),
2. the vault window (browsing, editing),
3. **scripts the owner writes**, over REST.

The third is what makes this a service rather than a shared library. Two
in-process consumers can each hold their own client and share an encrypted
disk cache -- which is what `DirectRest` does today, and it works. A third
consumer that is an arbitrary program cannot link against anything, so there
has to be a door, and the door has to be a process.

## What it replaces

`bw serve`. Not immediately, and not by deleting it -- `VaultBackendChoice`
keeps `BwServe` for as long as anyone depends on the CLI -- but this is the
thing that makes dropping it possible, because it provides the one capability
`bw serve` had that the in-process REST client does not: **a local HTTP
endpoint another program can call.**

## Two lifetimes, one mechanism

The owner asked for both:

- **24/7.** Installed and running whether or not any Deskwarden app is open,
  so a script at 3am finds it.
- **Consumer-driven.** No service installed; the first app to need the vault
  starts it, the last one out ends it.

**These are not two designs.** The 24/7 mode is the consumer-driven mode with
one permanent attachment: the service host claims a `vault_service` slot when
it is installed and holds it until it is uninstalled. Everything downstream --
`anyone_attached`, `release`, `supervise` -- is unchanged and does not know
which mode it is in.

That matters because a second lifetime scheme would be a second set of exit
conditions, and the exit condition is the part that has already proved
delicate. One rule, and the mode is just who is holding a slot.

## What this resolves

`2026-08-27-the-switch-over.md` blocked on a hole: with `bw serve` out of the
kill-on-close job, the daemon dying and the window closing leaves an unlocked
vault on localhost with nobody holding a handle to end it. The three options
were "let any of our apps kill a verified service" (needs a pid file),
"a supervisor process of ours" and "keep the job and start a backend per
app".

**This is that supervisor process, arrived at from the other direction.** The
service owns its own lifetime, so nothing has to reach across a process
boundary to end something it did not start, and `stop_action`'s rule -- a
handle, never a port -- holds without exception.

The switch-over is therefore **superseded, not merely deferred**. `bw serve`
stays in its job object until it is retired.

## The security surface, stated before it is built

A local HTTP endpoint serving a decrypted vault is the most dangerous thing
this project has built, and it is dangerous in a way `bw serve` already is:
any process running as this user can call it. That is not a reason to skip
it -- it is the reason to write the limit down now rather than discover it.

**Open question, deliberately not settled here:** how a client authenticates.
The candidates are a bearer token in a DPAPI-protected file (any process
running as the owner can still read it -- an honest limit, and the same one
`bw serve`'s session token has) or loopback-only with no auth (what `bw serve`
does today, and the weakest). This needs its own decision before any endpoint
is written, and it is the first question the implementation plan must answer.

Two things are settled: **loopback only**, and **never a secret on a command
line** -- `ui_process`'s rule, for its reason.

## What this is NOT

- **Not the two-binaries split.** That is `2026-08-26-two-binaries-design.md`,
  still gated on the updater swapping a set atomically. This service is a
  third process either way.
- **Not a new cache.** `vault_disk_cache` and `CachingBackend` already sit
  behind the one door; the service is where that door is, not a second one.
- **Not a rewrite of the REST client.** `rest::` is the client half and stays
  exactly as it is. This wraps it.

## Status

Design only. No plan, no code. The auth question above is the gate.
