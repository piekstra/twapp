# Safety-critical scoping — structural beats behavioral

> Field guide for any team running multi-agent work on twapp.
> When a new feature could accidentally affect a safety-critical
> adjacent code path, scoping is a code-organization problem, not a
> review-discipline problem.

## The pattern

When you add a feature whose flag could be misread by an adjacent
safety-critical code path, **thread the flag through as a parameter
on every relevant function signature** rather than reading it from
shared state inside those functions.

The compiler then enforces the scoping. Every call site has to pass
a value, so non-feature call sites visibly pass the safe default at
the call site itself. Forgetting to pass anything is a build error.

Compare to the alternative: every relevant function reads the flag
from shared state. Now *every* caller implicitly depends on whether
the flag happens to be set, including callers that should never be
affected by the feature at all. Scoping is enforced by convention —
"don't set the flag while these other paths are running" — and
convention erodes the moment a new caller is added.

The slogan: **structural scoping is enforced by the compiler;
behavioral scoping is enforced by hope.**

## Worked example

Imagine a system with a function `submit_order` that has a built-in
safety check (call it `validate_against_limits`) before sending the
order to an external system. The safety check is the *whole point*
of having `submit_order` go through this code path — it exists so
nobody can accidentally submit an unvalidated order.

A new feature lands: a manual override mode, where an authenticated
operator can submit an order that bypasses the safety check. Call
the override flag `armed`.

### The wrong way: state-reading

```rust
struct AppState {
    armed: bool,  // set when the operator opens the override UI
    // ...
}

fn submit_order(state: &AppState, order: Order) -> Result<...> {
    if !state.armed {
        validate_against_limits(&order)?;
    }
    send_to_external(order)
}
```

Why this looks fine: the override UI sets `armed = true`, calls
`submit_order`, the validation is skipped, the override works. Tests
pass. Ship it.

Why it isn't fine: `submit_order` is now globally affected by
`state.armed`. If any *other* code path calls `submit_order` while
`armed` happens to be true — the retry queue, the scheduled-batch
sender, the cancel-and-resubmit logic, a background reconciliation
job — *that* call also bypasses the safety check. Six months from
now, somebody adds a "resubmit recent failed orders" button, calls
`submit_order` from its handler, and never thinks to check the
global override flag because they didn't write the override feature
and don't know it exists. The first time the new button fires while
the override UI is open, an unvalidated order goes out.

There is no test you can write today that catches this future bug.
The bug is in code that doesn't exist yet.

### The right way: parameter-threading

```rust
fn submit_order(order: Order, bypass_checks: bool) -> Result<...> {
    if !bypass_checks {
        validate_against_limits(&order)?;
    }
    send_to_external(order)
}

// Override UI — the only caller that passes true:
submit_order(order, /* bypass_checks */ true)?;

// Every other caller — retry queue, scheduled-batch, resubmit
// handler, background reconciliation — passes false at the call site:
submit_order(order, /* bypass_checks */ false)?;
```

Now the scoping is structural. A reviewer reading any caller of
`submit_order` can see at the call site whether validation is being
bypassed. The future "resubmit recent failed orders" handler the
six-months-from-now developer adds will pass `false` because every
*other* caller passes `false`, and the developer copy-paste-pattern
will inherit the safe default. Forgetting to pass *any* value is a
build error, not a runtime bypass.

Pair this with an invariant test that asserts non-override call
sites pass `false`:

```rust
#[test]
fn no_auto_path_bypasses_validation() {
    // Grep-style assertion: every call to submit_order outside
    // src/override_ui.rs passes bypass_checks=false.
    // (Or a lint, or a structural codemod test — the mechanism
    // matters less than that the invariant is named and checked.)
}
```

## Why state-reading keeps tempting people

The state-reading version is shorter to write the *first* time. The
override flag already exists in shared state for the UI to check, so
"just read it inside `submit_order`" feels natural. Threading a new
parameter through the whole call graph is more typing, more diff,
and you have to update every caller — which is exactly the property
that makes it safe.

If the brevity of the state-reading version is what attracts an
implementer to it, that's a signal the briefing should call out the
parameter-threading requirement explicitly. Don't let the
implementer choose; the implementer doesn't have visibility into
which adjacent call paths might consume the same flag wrongly.

## When to reach for this pattern

The cost of parameter-threading is small but real (more diff, more
function-signature churn). Reach for it when:

- The new feature's flag could be misread by an adjacent code path
  whose failure mode is **safety-critical** — money, units, real
  external effects, anything that doesn't have an undo.
- The function whose behavior the flag changes has *more than one
  caller*, especially callers that may grow over time (a public API,
  a widely-used helper, a module-level function).
- The flag's "armed" state is held in shared mutable state that any
  caller could observe.

Skip it when:

- The flag and the function are in the same module, with one caller,
  and adding a new caller would obviously require touching the
  flag-handling code anyway.
- The "wrong" behavior under the flag is recoverable — a UI glitch,
  a logged warning, a missing label. Not catastrophic.

## Where this fits in the coordinator's review pass

This pattern is a review red flag of its own: when an implementer's
diff adds a new feature gated by a flag-on-shared-state, the
coordinator should ask "could a non-feature caller of this function
be affected by the flag?" If the answer is "yes, in principle, but
no caller does that today", require a parameter-threading rewrite
before merge. The compiler-enforced version is the only one that
stays correct as the call graph grows.

Cross-reference: the holistic review checklist in the
agent-coordinator skill covers blast-radius judgment more generally;
this playbook is the specific structural pattern to require when
the blast radius includes a safety-critical adjacent path.
