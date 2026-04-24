# Review red flags — anti-patterns a coordinator catches before merge

> Field guide for any team running multi-agent work on twapp.
> The reviewer agent answers "is this code correct?" The coordinator
> asks a different question — "what real-world inputs used to work
> that this rewrite now rejects?" These four red flags are the
> recurring shapes of that question.

A clean CI run does not mean a PR is safe to merge. An implementer
operates with blinders on: it reads a narrow briefing, writes narrow
tests, and confirms only that the cases the briefing considered still
work. Production input distributions are wider than briefing
authors anticipate. Four anti-patterns recur often enough to deserve
their own checklist.

Run the four checks below on every implementer PR, regardless of CI
status. Each one is a 30-second scan of the diff plus one targeted
question. None of them require running the code.

---

## 1. Tolerant → strict rewrites

**Rule:** when a diff replaces permissive parsing or validation with
exhaustive match arms, ask "what inputs did the old code accept that
the new code rejects?"

The old code probably did not have an explicit list of accepted
inputs — it accepted whatever happened to flow through, and the
production input distribution shaped its behavior over time. The new
code has a list, and that list was written from the briefing, not
from the data.

**Worked example.** Imagine a parser that splits a record into tokens
and pulls the first three:

```
fn parse(s: &str) -> Option<Record> {
    let mut it = s.split(',');
    Some(Record {
        a: it.next()?.parse().ok()?,
        b: it.next()?.parse().ok()?,
        c: it.next()?.parse().ok()?,
    })
}
```

A briefing arrives that says "make this parser strict — reject
malformed input". An implementer rewrites it as an exhaustive match:

```
fn parse(s: &str) -> Option<Record> {
    match s.split(',').collect::<Vec<_>>().as_slice() {
        [a, b, c] => Some(Record { a: a.parse().ok()?, b: b.parse().ok()?, c: c.parse().ok()? }),
        _ => None,
    }
}
```

The tests pass. The diff is clean. CI is green. But the old parser
silently ignored extra tokens — and a non-trivial fraction of real
production inputs are 4-token records (a recent format extension that
nobody updated the parser briefing for). The new code rejects all of
them.

**What to ask before merging:** "Name three inputs that the old code
accepted and the new code rejects. If you can't name any, are you
sure you sampled real production data?"

---

## 2. Fix too aggressive

**Rule:** the scope of the fix should match the scope of the bug. A
presentational bug gets a presentational fix. A controller-lifecycle
bug gets a controller-lifecycle fix. Mixing the two is scope creep
disguised as completeness.

When the original bug is "the UI shows stale data after a context
switch", the fix is "hide or refresh the stale view". It is not "tear
down the underlying background process". The background process may
be doing legitimate work that survives the context switch.

**Worked example.** A bug report says: "when the user changes
profiles, the inbox panel still shows messages from the previous
profile until I refresh."

The right fix is presentational — clear or hide the inbox panel on
profile change, then re-render once the new profile's inbox loads.

The wrong fix, which keeps showing up in implementer PRs because it
*also* makes the test pass, is to also stop the background sync
process on profile change. That kills legitimate work — a long-poll
that was holding the connection open, an in-flight upload, a queued
action waiting for the user to come back. The presentational symptom
goes away, and a new class of bug ("uploads sometimes vanish if I
switch profiles mid-upload") replaces it.

**What to ask before merging:** "Does this fix change behavior
*beyond* the symptom that was reported? If yes, is the additional
behavior change justified by a separate, named requirement?"

---

## 3. Tests cover documented cases, not real ones

**Rule:** a clean CI run confirms that the cases the briefing
considered still work. It does not confirm the change is safe. Sample
inputs from actual runtime data and add at least one real-data test
before merging parser, validator, or transformer changes.

Implementers write tests against the examples in the briefing. The
briefing examples were written by the briefing author from memory or
from a small sample. Real production traffic has shapes neither the
briefing author nor the implementer thought to enumerate.

**Worked example.** A briefing says: "validate that timestamps are
ISO-8601." The implementer writes tests for `2025-01-01T00:00:00Z`
and a few obvious malformed cases, all passing.

In production, half the upstream feeds emit `2025-01-01T00:00:00.000Z`
(milliseconds), some emit `2025-01-01T00:00:00+00:00` (explicit
offset), and a handful emit `2025-01-01T00:00:00` with no zone at
all. The validator the implementer wrote rejects all three. CI was
green because the test corpus was the briefing's two examples.

**What to ask before merging:** "Is at least one test case sourced
from a real production payload, log line, or recent record? If
not, sample one before merging."

If the implementer can't easily get real samples, the coordinator can
provide them — copy-paste a few recent inputs from logs into the
mailbox and tell the implementer to add tests for each. Don't merge
on a promise that real-data tests will come "in a follow-up PR";
that follow-up rarely materializes.

---

## 4. Silent-failure paths downstream

**Rule:** when a function returns `Option::None`, falls back to a
weaker comparator, or otherwise quietly degrades, ask where that
fallback ends up being observed. Often it surfaces as a second-order
bug (sort order, rendering, label) that looks like a different
system's problem.

A `None` that propagates up through three or four call sites can
manifest as a UI element that doesn't render, a sort that goes
lexicographic when it should be chronological, a color that defaults
to grey, or a log line that quietly drops a field. Reviewers fixated
on the diff itself rarely follow the `None` to its observation
point — but that's where the bug actually shows up.

**Worked example.** A diff changes a date-parser to return
`Option<Date>` instead of `Date`, with `None` for malformed input.
The reviewer approves: "looks correct, malformed input shouldn't
crash."

But the date is consumed by a sort comparator three levels up the
call stack. The comparator was written assuming the parser always
returns a `Date`; on `None`, it falls back to comparing the raw
strings. The list now sorts lexicographically — `"2025-10-01"` before
`"2025-9-01"` — and a UI that was chronological an hour ago is now
visibly wrong.

The bug report comes in as "the sort is broken" and gets routed to
the UI team. The actual cause is two systems away.

**What to ask before merging:** "Follow every new `None` /
`Result::Err` / fallback at least one level out from the diff. Where
does it get observed? Is the observer prepared to handle the
degraded value, or does it silently produce a wrong-but-plausible
output?"

---

## Where this fits in the coordinator's review pass

These four checks are the coordinator's own review pass, run *after*
the reviewer's "is this correct?" pass and *before* merge. They
complement — they don't replace — the holistic-PR-review checklist
in the agent-coordinator skill's "Reviewing implementer PRs
holistically" section, which covers subsumption, replay against
recent reality, blast-radius judgment, and observer-asks. The four
red flags above are the *common shapes* of the questions that
checklist asks; the checklist is the framework, the playbook is the
field manual.

Block the merge when any of these surface, and mailbox the
implementer with the *specific* failing input case (or call path).
"Add a test for 4-token inputs" produces a fix in one round.
"Please add more tests" produces three rounds of clarifying
questions.
