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

## The Error Handler — **DECIDED: template**

```
AHPCL Error Handler:
Hello, I think that there's something wrong.
file:line:col
file:
line:
column:
rule conditions:
suggest fix:
```

The greeting appears **once**, not per error — with two or more errors it is not repeated.

Rendered with a real error:

```
AHPCL Error Handler:
Hello, I think that there's something wrong.
main.ah:12:17
file: main.ah
line: 12
column: 17
rule conditions: matrix multiplication requires inner dimensions to agree.
                 'a' is [3, 4] and 'b' is [5, 2] — 4 ≠ 5.
suggest fix: declare 'b' as [4, 2], or transpose it before multiplying.
```

Note the template carries both a compact `file:line:col` line and the same three values spelled
out beneath it. Recorded as written; **confirm** whether that repetition is intended.

**OPEN:** whether the `AHPCL Error Handler:` header also appears only once, or per error — only
the *greeting* was specified.

**OPEN:** what separates consecutive errors.

**OPEN:** whether errors may point at **more than one location**. Several already-decided errors
are inherently two-place — a refinement violation is "this promise, made here, is broken by that
assignment, over there" — and the template has one location slot.

**OPEN:** whether the source line itself is shown.

**OPEN:** error codes (`AHPCL-E0012`) for searchability; whether the Informer shares this
template; machine-readable output for editors.

## Errors decided so far

Every one of these is a compile error, per
[types.md](types.md):

- Numeric literal with no type context to pin it
- Precision unstated and not knowable at compile time
- `infnum` given an explicit width
- `deci` given a width that is not an IEEE decimal format (8-bit, 16-bit)
- Overflow
- A sign refinement the compiler cannot prove — *unless* layer 3 inserts a runtime check
- A comment line-count running past the end of the file. Message **temporary**: *"What the
  heck am I supposed to do?"* — rewrite once the template below is settled.
