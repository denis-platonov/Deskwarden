# 01 -- the preflight gate is deleted

Escape under test: a fill that types a secret into whatever holds focus
without the foreground ever being described or judged.

`app.rs`'s sequence arm calls `preflight::dispatch_with`, which runs the
sender **only** when the foreground can be described and `verdict` allows it.
This mutant removes the gate entirely: the sender is invoked directly and the
three `Gated` arms (send / refuse / no-target) disappear with it.

Spelling note: the prose says "delete the gate from `fill_from_vault_with`'s
sequence arm". Deleting only the `dispatch_with` call does not compile -- the
`match gated` below it has nothing to match on -- so the anchored block runs
from `let gated = ...` through the closing `};` of that match, and the
replacement is the one line that arm would contain if the gate had never been
written. The refusal `notifier` calls go with it, because there is nothing
left that can refuse.
