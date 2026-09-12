# The preflight mutations, as something you can run

The preflight gate in `app.rs`'s sequence arm has recorded mutations --
escapes the suite is supposed to catch. Until now they existed only as English
sentences in doc comments, and a figure ("3 / 2 / 1 / 2") was quoted from
those sentences as a merge gate in
`docs/superpowers/plans/2026-08-18-the-overlays-other-states.md`.

> **Two of the four cases have been retired.** `03-confirm-deleted` and
> `04-confirm-answer-ignored` mutated design 4b's send confirmation -- the
> first deleted the `confirmed_by_preflight` call, the second read its answer
> and threw it away. The owner had that confirmation removed outright ("user
> intentionally sends the whatever needed - their right"), so mutation 03 is
> now simply what `app.rs` says and mutation 04 has no function left to call.
> A mutation whose "after" state is the shipped source measures nothing, so
> both directories are gone rather than left to apply against text that is not
> there -- `run.ps1` treats a `find.txt` that matches zero times as a hard
> error, which is exactly the right behaviour and exactly why they cannot
> stay. **Cases 01 and 02 are untouched**: they mutate the GATE, which
> survived the card and is the thing this directory was built for. See
> `deskwarden/src/vault_window/preflight.rs`'s module doc.

The figure was not reproducible. Two readers independently re-derived the
mutations from the prose, wrote slightly different code, and measured
4 / 3 / 3 / 2 -- against an unchanged gate. **The prose was the defect, not
the gate.** This directory replaces it: each mutation is now an exact,
anchored source replacement, and the numbers are produced by running
something.

## Run it

```powershell
pwsh -File mutations/run.ps1                        # all four
pwsh -File mutations/run.ps1 -Case 02-gate-neutralised
pwsh -File mutations/run.ps1 -Commit e22805f        # measure a specific commit
```

It is a developer tool, run by hand, and is deliberately **not** part of
`cargo test` -- it builds and runs the whole suite four times and takes
several minutes. Nothing is written to your working tree: each case gets a
fresh detached `git worktree` under the system temp directory, the run shares
one fresh `CARGO_TARGET_DIR` also outside the repository, and both are removed
afterwards, including on failure. Exit status is non-zero if any mutant
survived or failed to build.

## How a case is defined, and why not a patch

Each directory under `cases/` holds:

| file | meaning |
| --- | --- |
| `target.txt` | repository-relative path of the file to mutate |
| `find.txt` | the exact text to replace |
| `replace.txt` | what to put there |
| `about.md` | the escape it stands for, and why it is spelled this way |

Not a line-numbered `.patch`: line numbers drift on every unrelated edit above
them, and a patch that fails to apply against drifted context is noise rather
than signal. **The anchor must occur exactly once.** Zero or two occurrences
is a hard error and stops the run -- a mutation that silently fails to apply
would report 0 red and read as a catastrophically weakened gate, which is the
worst possible failure mode for this tool.

`find.txt`/`replace.txt` are compared with newlines normalised to LF and one
trailing newline stripped, so a CRLF checkout (which `.gitattributes` pins for
`*.rs`) and an LF one measure the same thing. The mutated file is written back
with the line endings it was read with.

## Read the names, not the count

The count is what drifts between spellings. The killing test **names** are
what say whether the gate still catches the same escape. If a count changes,
compare the names against the run below before concluding anything.

## Last measured

**This run is history, not a current expectation**: it names four cases and
two of them no longer exist, and the two killing tests listed under 03 and 04
were deleted with the confirmation they were about. Cases 01 and 02, and their
killers, are unchanged -- re-run to get current numbers.

Commit `e22805f`, 2026-08-19, on Windows 11 / `pwsh` 7. Re-run unchanged at
`d750693` (this work merged with the password-health branch, 2026-08-19):
same counts, same killing test names.

```
deskwarden preflight mutations -- e22805f -- 2026-08-19

case                         red  status
01-gate-deleted                2  killed
02-gate-neutralised            2  killed
03-confirm-deleted             2  killed
04-confirm-answer-ignored      1  killed

killing tests
  01-gate-deleted
    app::fill_dispatch_tests::a_password_fill_types_nothing_when_the_preflight_refuses
    app::preflight_call_site_tests::the_sequence_sender_is_reached_only_from_inside_the_gate
  02-gate-neutralised
    app::fill_dispatch_tests::a_password_fill_types_nothing_when_the_preflight_refuses
    app::preflight_call_site_tests::the_sequence_sender_is_reached_only_from_inside_the_gate
  03-confirm-deleted
    app::fill_dispatch_tests::a_bare_secret_fill_asks_the_confirmation_before_it_types
    app::fill_dispatch_tests::a_cancelled_confirmation_types_nothing
  04-confirm-answer-ignored
    app::fill_dispatch_tests::a_cancelled_confirmation_types_nothing
```

Every mutant compiled; every mutant was killed; the bin target (228 tests) was
green under all four, so the whole gate is pinned by the lib suite.

## Was "3 / 2 / 1 / 2" ever right?

Probably not as a whole, and it cannot be recovered. Under the spellings here
the answer is **2 / 2 / 2 / 1**. Two of its four entries do fall out of
plausible readings -- "delete: 2" matches case 01 exactly, and "1" for the
third matches case 03 if you count only the test named in that comment
(`a_bare_secret_fill_asks_the_confirmation_before_it_types`) and miss that
`a_cancelled_confirmation_types_nothing` also goes red. The other two do not
reconcile with anything measurable, and the later 4 / 3 / 3 / 2 does not
either. Both figures are best read as artefacts of the spelling their author
used, recorded without the spelling.

The lesson is narrow and worth keeping: a mutation-testing figure is only a
fact about a *specific mutant*, so the mutant has to be the thing that is
written down.

## What this exercise said about the gate itself

- The gate is genuinely pinned, and by tests that name the escape rather than
  the mechanism. `the_sequence_sender_is_reached_only_from_inside_the_gate`
  kills both gate mutations for the right reason: it is about the sender's
  reachability, not about `dispatch_with` being called.
- Cases 01 and 02 are killed by an identical pair. Deleting the gate and
  neutralising it are, to this suite, the same escape -- which is correct and
  worth knowing, but it means "neutralise vs delete" was never two data
  points.
- Case 04 had exactly one killer. `a_cancelled_confirmation_types_nothing` was
  the only test in the crate that noticed the confirmation's answer being
  discarded, which is precisely why it was a separate test from the one above
  it. A single killer is not weak, but it has no redundancy: delete that test
  and the escape becomes free. That turned out not to matter in the end --
  the confirmation itself was removed, and the test went with it -- but the
  observation is the reason this file records killer *names* rather than only
  counts.
