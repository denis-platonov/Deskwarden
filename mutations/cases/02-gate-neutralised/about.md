# 02 -- the preflight gate is neutralised

Escape under test: the gate is still *called*, so anything that only checks
"was the seam consulted" is satisfied, but its answer changes nothing.

Spelling note -- **this is where the prose is ambiguous**, and it is the
reason two agents measured two different numbers. The comment suggests
`let _gated = dispatch_with(..);`. Read literally, with the original closure
left in place, that is *not* a neutralisation at all: `dispatch_with` only
invokes the closure when it allows the send, so the literal reading still
refuses to type -- it merely drops the refusal notice. The only thing it
kills is the notification, not the gate.

The doc's own next sentence says the mutant makes "the two refusal cases
below type the password", which forces the other reading: the verdict is
discarded **and the sender runs unconditionally**. That is the escape worth
measuring, so it is the spelling used here -- `dispatch_with` is still called
(with an inert closure, so the gate's own observable behaviour is unchanged),
its result is bound to `_gated`, and `fill_sequence` is called outside it.
