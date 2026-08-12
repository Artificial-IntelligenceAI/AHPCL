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
          skip with: ahpcl task:build. flag:loop-evaluation = limit.

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

**The source line is shown**, with a marker under the offending span — the single most-praised
feature of Rust's diagnostics, and good errors were the stated reason for choosing Rust as the
host language.

Rendered with a real error:

```
AHPCL Error Handler:
Hello, I think that there's something wrong.
main.ahpcl:12:17
file: main.ahpcl
line: 12
column: 17

    12 |     var:matrix:num 'c' = math { ('a') · ('b') }.
       |                                       ^^^^^^^^

rule conditions: matrix multiplication requires inner dimensions to agree.
                 'a' is [3, 4] and 'b' is [5, 2] — 4 ≠ 5.
suggest fix: declare 'b' as [4, 2], or transpose it before multiplying.
```

Source files use the extension **`.ahpcl`**.

The repetition is **deliberate**: `file:line:col` is the compact form editors and terminals
click on, and the three spelled-out fields beneath are a **legend** teaching you how to read it.
Not redundancy — do not "tidy" it away.

**OPEN:** whether the `AHPCL Error Handler:` header also appears only once, or per error — only
the *greeting* was specified.

**OPEN:** what separates consecutive errors.

### Multiple locations — **DECIDED**

One error may quote **more than one line**, each with its own marker and note. The header fields
name the primary spot; the quoted section tells the whole story:

```
AHPCL Error Handler:
Hello, I think that there's something wrong.
main.ahpcl:3:11
file: main.ahpcl
line: 3
column: 11

     1 | var:+int 'n' = '10'.
       |     ^^^^ 'n' promises to stay above 0 here
     3 | set 'n' = math { ('n') - 20 }.
       |           ^^^^^^^^^^^^^^^^^^ but this can make it -10

rule conditions: a +int must be above 0 at every point in the program.
suggest fix: declare 'n' as :int, or check the value before assigning.
```

Most AHPCL errors are about *relationships* — a refinement promised in one place and broken in
another, a shape declared here and used there — which one location cannot express.

**Implementation note:** an error must carry a *list* of locations from the start. Cheap now,
irritating to retrofit.

### Columns count grapheme clusters — **DECIDED**

A column is one **user-perceived character**, not a byte and not a code point.

| | Columns |
|---|---|
| `C` | 1 |
| (space) | 1 |
| `🧑‍🧑‍🧒‍🧒` | 1 |
| `🧑‍🧑‍🧒‍🧒🧑‍🧑‍🧒‍🧒` | 2 |

That family emoji is 4 people joined by 3 zero-width joiners — 7 code points, about 25 bytes,
and **one** column. Rust's `unicode-segmentation` crate implements the relevant standard
(UAX #29).

**Known friction:** editors disagree. VS Code reports columns in UTF-16 code units, Vim in
bytes. So `main.ahpcl:12:17` may not match the position an editor displays for the same spot.
Unavoidable given the choice, and worth documenting for users rather than fixing.

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
