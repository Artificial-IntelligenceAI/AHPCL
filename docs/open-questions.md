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

## Gaps found by writing a sample program (2026-08-12)

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

**3c. The rest of the maths operations** — `sin`, `cos`, `log`, `abs`, `floor`, `round`. `sqrt`
is decided as a prefix operator with word and symbol spellings; whether the others follow the same
shape is not. Note `|x|` for absolute value is available — `|` was deliberately kept free.

**3d. Prefix operator precedence** — does `math { sqrt ('x') + 1 }` mean `√x + 1` or `√(x + 1)`?

**4. LaTeX notation — which reading?** `\(2^{3}\)` could mean grouping braces on `^` (cheap,
easy) or genuinely embedded LaTeX math mode (`\frac`, `\sum`, `\int` — a signature feature, and
a second parser). Note `\` is already the escape character.

**6. Resolved** — `[…]` with space-separated arguments and no commas, for both `print` and user
functions.

**6b. Do future builtins stay bare like `print`**, or become ordinary quoted functions?

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
