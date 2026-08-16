# Open questions

The running agenda. **Resume from here.**

Design decisions belong to Tankun Sriket. Ask **one question per message**; present the
trade-offs first, then a single question.

---

## Arrays — model B chosen, details outstanding

**1k. What else may `nna` hold** besides text (booleans? dates?), may one `nna` mix kinds, does
it have a rank, and does the summing rule make bare `('names')` concatenate?

**1a. Does `dyn` prefix every array type name** (`dynvector`, `dyntensor`)? And should
`dyntensor` mean *unknown rank* — a distinction `?` notation cannot express — rather than being
a pure synonym for `[?, ?]`?

**1c. How is an empty array written?** (Shape-may-be-omitted is decided — a literal determines it.)

**1g. Is `tensor` legal at rank 1 or 2**, or strictly rank 3 and above?

**1h. Confirm the element-type order** — `matrix:num` was assumed, never stated.

---

## Diagnostics

**2. Machine-readable diagnostics** (JSON for editors) — the last open item in
[diagnostics.md](diagnostics.md). Everything else about the Error Handler and Informer is
decided.

---

## Gaps found by writing sample programs

**31. More `parse` options** — `binary`/`octal`, `allow-empty`, and `percent` (reading `"50%"` as
0.5, using the `%` kept free from `mod`). The roster so far is decided. Format-aware reading (CSV
straight to a typed array) remains a **later** addition.

**32. Runtime failure wording** — the greeting line, and whether `AHPCL-RUN-0001` is the right code
category. The behaviour is decided: failures stop the program.



**25. Confirm that `var:int 'q' = math { 10 / 4 }.` errors** rather than truncating. Recorded as
INFERRED. Operators themselves are decided (`//`, `mod`).

**26. Confirm the comparison family** — `!=`/`≠`, `<`, `>`, `<=`/`≤`, `>=`/`≥`. Equality itself is
decided (`=` inside `math { }`).

## Syntax

**3. Which Unicode symbols are aliases.** Mostly **blocked**: half the old "safe list" needs
operations that do not exist. `≤ ≥ ≠ ÷` alias things that do exist. `∧ ∨ ¬` need boolean operators;
`∈` needs sets; `√` needs square root; `≈` needs a tolerance rule. `∧ ∨ ¬` are **decided**, and
`π` `e` `τ` `∞num` are **decided** (see [syntax.md](syntax.md)).
`· × ⊙ ⊗` are reserved operations, and `∘` is unassigned.

**3c. The maths operator roster** — the shape is decided (all operators, words plus symbols).
Still open: the full list, and which of `log`/`ln` means which base.

**4. LaTeX notation — which reading?** `\(2^{3}\)` could mean grouping braces on `^` (cheap,
easy) or genuinely embedded LaTeX math mode (`\frac`, `\sum`, `\int` — a signature feature, and
a second parser). Note `\` is already the escape character.

**6. Resolved** — `[…]` with space-separated arguments and no commas, for both `print` and user
functions.

**6b. Resolved** — bare means builtin, quoted means user-defined.

**7. Is juxtaposition-concatenation general** or specific to `print`? Does `(x)` interpolate in
any string context?

**8. Does `\` escape outside strings** too?

**11. Rewrite the temporary comment-overrun error message** — the template now exists, so this
is actionable. Currently "What the heck am I supposed to do?", and it needs code
`AHPCL-LEX-0001`.

**14c. Confirm `,` extends a `change:`** — recorded as INFERRED from the language-wide comma
rule. Everything else about `change:` is decided.

**14f. Is `none` legal outside a return type?** `var:none 'x'` is meaningless; presumably an error.

---

## Types

**29. Symbolic values — parked, not rejected.** Keeping π (and `√2`, `e`) exact and unevaluated
until forced, rather than computing digits. Fits the exact/symbolic domain; a much bigger feature
than anything decided so far. See [types.md](types.md).

**15. `float`** — still deferred. Wanted by the scientific domain for speed; `rat` was added
2026-08-12.

**16. Review the sign algebra** table in [types.md](types.md) — derived, not dictated.

**30. A declaration with no value — DECIDED 2026-08-13: it is an error.** `var:int 'x'.`
is now `AHPCL-TYPE-0005`. Nothing can be read before it is written, so there is no unset
state to represent and no silent 0 to explain. Rejected: the zero of the type (a silent
default), and a real "unset" value (every type gains a state and every read gains a check).
Moved to [types.md](types.md).

**31. Should `parse` read a fraction — DECIDED 2026-08-13: yes, under the option
`fraction`.** Implemented. See [syntax.md](syntax.md).

**32. Does `handback` end a loop body — DECIDED 2026-08-13: yes.** Moved to
[syntax.md](syntax.md).

**34. Multi-file builds compile each file separately.** `buildfile:main.ahpcl, lib.ahpcl.`
runs an independent check/build/run per file, so a function declared in one is not visible
in the other — while [cli.md](cli.md) presents this as *the* multi-file mechanism. Needs a
decision on what "multi-file" means (one program from several files, in order? a module
system? something else) before it can be implemented. Found by stress-testing, 2026-08-13.

**35. `⊗` disagrees with its own documentation.** [syntax.md](syntax.md) shows
`[1,2,3] ⊗ [4,5,6]` giving a flat 9-element vector; the implementation gives shape `[3,3]`,
and declaring `[9]` is a shape error. Both are defensible — the tensor product of two
vectors is conventionally a matrix, which is what the code does — so this is a question of
which one moves. Found by stress-testing, 2026-08-13.

**36. `read` into an array is documented but unimplemented.** [types.md](types.md) and
[syntax.md](syntax.md) both show `var:vector:num 'data' [?] = read["measurements.csv"].`
`read` only ever produces `str`, so this is a type error. Needs a decision on how a file
becomes numbers — split on what, parsed with which options — before it can be built.

**37. A real `rule conditions:` line.** The label used to sit above a plain statement of the
problem, which is not a rule's conditions; that line is now `what went wrong:`. The intended
feature remains: show the conditions of the rule that was broken, quoted from the rule, so a
reader learns the rule and not just this instance of breaking it. Needs deciding where the
text comes from — written per error code, or generated from the checker's own conditions —
and whether it appears always or only when asked. Raised by the user, 2026-08-13.

*(Numbering has gaps where items were resolved or removed. Kept stable so references hold.)*

---

## CLI

**19. `loop-evaluation` value vocabulary** beyond `limit`, where the numeric limit is supplied,
and whether defaults vary per task (`build` unbounded vs `check` limited).

**20. Is `task:build buildfile:….` one directive or two?** Recorded as two.

**21. Do `resultname:` and `to:` apply to `task:check`**, which produces no output?

---

## Toolchain

**22. LLVM bindings** — `inkwell` (Claude's recommendation), raw `llvm-sys`, or a bespoke
wrapper?

**23. GPU** — never, later-designed-for (Claude's recommendation), or now?

**24. Check the LLVM version** available on this machine before any codegen work, and pin it
across `inkwell`, Homebrew, and CI.
