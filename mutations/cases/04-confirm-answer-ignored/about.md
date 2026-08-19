# 04 -- the confirmation's answer is ignored

Escape under test: the surface is still shown -- so any test that only counts
"was the user asked" is satisfied -- but Cancel does not stop the fill.

`if !confirmed_by_preflight(..) { return; }` becomes
`let _ = confirmed_by_preflight(..);`. This is the neutralisation shape the
crate has measured surviving elsewhere at zero warnings, which is why it is
worth a case of its own rather than being folded into 03.
