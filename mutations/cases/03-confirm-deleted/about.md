# 03 -- the 4b confirmation is never shown

Escape under test: the hold-to-send surface stops being hosted, so a bare
secret fill goes straight to the gate with no human in the loop.

The `if !confirmed_by_preflight(..) { return; }` guard is removed outright.
The gate itself is untouched, so every routing test in
`vault_window::preflight` stays green -- which is the point: this mutation
isolates the *hosting* from the gating.
