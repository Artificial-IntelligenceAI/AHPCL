# Open questions

The running agenda. **Resume from here.**

Design decisions belong to Tankun Sriket. Ask **one question per message**; present the
trade-offs first, then a single question.

---

## In flight

**1. Unbounded compile-time evaluation where nobody is watching.**
Layer 1 verification runs to completion with no cap, relying on a human seeing the Informer's
progress and choosing to intervene. That fails in CI (pipeline hangs until timeout), in
editors (an on-every-keystroke `check` freezes), and with piped output. Options put to
Tankun: unbounded everywhere; unbounded only when output is a real terminal and capped
otherwise (Claude's recommendation); or an interactive "press S to skip" keypress.

---

## Blocking other decisions

**2. The array model — `[no preference]`, discussed at length, still unanswered.**
Whether maths works on whole collections at once. All three options still have arrays as
*storage*; the question is whether they are values.

- **A — scalar only.** The language knows single numbers; you write every loop. C, Rust, Go.
- **B — arrays are values.** `math { (a) + (b) x (c) }` operates on a million numbers. NumPy,
  MATLAB, APL. Claude's recommendation, since it makes the Unicode symbol plan meaningful and
  is cheap if fusion is deferred.
- **C — middle ground.** Looks like A; the compiler understands an array type well enough to
  optimise later. Fortran 90+, Julia.

Why it blocks things: it decides whether `·`, `×`, `⊙`, `⊗` are reserved as distinct
operations (dot, cross, elementwise, tensor) or aliased to plain multiplication. Note `×` has
*already* been decided as a multiplication alias, which spends it.

Asymmetry worth knowing: A → B later is a middle-end rewrite; B → A is merely wasted effort.

Two sub-questions if B:
- **Shapes in the type system** (`vec[3] + vec[4]` is a compile error — the Futhark/Dex
  design, consistent with every other choice made so far, but a serious step up in
  difficulty) or **checked at runtime** (simpler, crashes later)?
- **Broadcasting** — does `math { (a) + 1 }` add 1 to every element? Convenient; also a famous
  source of shape bugs.

---

## Awaiting the sample Tankun offered

**3. The error message template.** See [diagnostics.md](diagnostics.md) for the questions it
needs to settle: error codes, source excerpts and labels, suggestion lines, placement relative
to Informer output.

---

## Syntax

**4. Which Unicode symbols are aliases.** "Probably every, idk". `×` confirmed. The safe list
(`≤ ≥ ≠ ÷ √ π ∞ ≈ ∧ ∨ ¬ ∈ →`) versus the ones worth reserving — blocked on #2.

**5. LaTeX notation — which reading?** `\(2^{3}\)` could mean grouping braces on `^`
(cheap, easy) or genuinely embedded LaTeX math mode (`\frac`, `\sum`, `\int` — a signature
feature, and a second parser). Note `\` is already the escape character.

**6. Is `math { }` required for all arithmetic**, or only when the extended symbols are wanted?

**7. Is `[…]` how all calls work** — `add[1 2]`, space-separated, no commas anywhere?

**8. Is juxtaposition-concatenation general** or specific to `print`? Does `(x)` interpolate
in any string context?

**9. Does `\` escape outside strings** too?

**10. Confirm the whitespace rule** for `x`-as-multiplication (marked INFERRED in
[syntax.md](syntax.md)). The whole `( )` design depends on it.

**11. Approve the `.`-versus-decimal-point rule** (marked PROPOSED in
[syntax.md](syntax.md)).

**12. Indexing syntax.** Discussion used `(a)[(i)]`, which clashes with `[…]` for calls.

**13. Function definitions, control flow, and the mutation statement.** `set 'x' = …` and
`loop while … { }` are Claude's invented placeholders, not decisions.

---

## Types

**14. `float` and `rat`** — deferred by Tankun, but both are wanted by the stated domains.
Type names not chosen.

**15. Review the sign algebra** table in [types.md](types.md) — derived, not dictated.

**16. Function parameter precision** — must it be stated explicitly, since range analysis
cannot follow values across callers?

---

## Toolchain

**17. LLVM bindings** — `inkwell` (Claude's recommendation), raw `llvm-sys`, or a bespoke
wrapper?

**18. GPU** — never, later-designed-for (Claude's recommendation), or now?

**19. Check the LLVM version** available on this machine before any codegen work, and pin it
across `inkwell`, Homebrew, and CI.
