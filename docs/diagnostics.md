# Diagnostics

See [README.md](README.md) for the status legend.

AHPCL splits compiler output into two named subsystems — **DECIDED**:

| Subsystem | Reports |
|---|---|
| **AHPCL Error Handler** | Errors |
| **AHPCL Informer** | Everything the compiler decided or inferred on your behalf |

Surfacing implicit decisions as first-class output is deliberate, and unusual — most
compilers never tell you what they silently chose.

## The Informer — **DECIDED**

**On by default, full detail.** Every default applied and every inference made is reported
inline, always.

What it reports, gathered from the decisions made so far:

```
informer: 'x' widened to 32-bit because of line 7
informer: 'n' is mutable; +int refinement tracked across 3 assignments (lines 4, 9, 12)
informer: line 4  — loop evaluated at compile time (99 iterations); +int on 'n' verified
informer: line 12 — range analysis proved 'n' ∈ [1, 100]; +int verified
informer: line 20 — +int on 'n' unproven; runtime check inserted
```

Live progress during compile-time evaluation, rate-limited to roughly every 250 ms, with a
skip command and a closing timing report:

```
informer: line 12 — evaluating loop at compile time...
          1,240,000 iterations · 3.2s elapsed
          skip with: ahpcl build main.ah --no-const-eval

informer: line 12 — loop evaluated at compile time
          8,400,000 iterations · 21.4s · +int on 'n' verified

informer: compile-time evaluation took 21.4s of 24.1s total
```

Known trade-off, accepted: at full detail a large program may emit a great many notes. A
`--quiet` escape hatch is **PROPOSED**, not requested.

## The Error Handler — **OPEN**

Tankun offered an error-message sample/template; it has not been provided yet. Until then
nothing here is decided.

Questions the template needs to settle:

- **Error codes?** Something like `AHPCL-E0012` makes errors searchable and documentable.
- **Source excerpts** — show the offending line with a marker beneath it? Can a single error
  carry more than one label (the error here, "because of this" there)?
- **Suggestion lines** — `help: write :deci instead`?
- **Placement** — is Informer output interleaved with errors, or a separate section?
- **Machine-readable output** — JSON for editor integration. Later, but it shapes the
  internal representation, so worth knowing early.

## Errors decided so far

Every one of these is a compile error, per
[types.md](types.md):

- Numeric literal with no type context to pin it
- Precision unstated and not knowable at compile time
- `infnum` given an explicit width
- `deci` given a width that is not an IEEE decimal format (8-bit, 16-bit)
- Overflow
- A sign refinement the compiler cannot prove — *unless* layer 3 inserts a runtime check
- A comment standing where a value belongs — *"placeholder not yet resolved"*, ideally
  quoting the comment's own text. See [syntax.md](syntax.md).
