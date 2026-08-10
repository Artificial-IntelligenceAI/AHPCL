# Open questions

The running agenda. **Resume from here.**

Design decisions belong to Tankun Sriket. Ask **one question per message**; present the
trade-offs first, then a single question.

---

## The one with the longest reach

**1. The array model.** Answered `[no preference]`, discussed at length, still open. Whether
maths works on whole collections at once. All three options still have arrays as *storage* —
the question is whether they are values.

- **A — scalar only.** The language knows single numbers; you write every loop. C, Rust, Go.
- **B — arrays are values.** `math { (a) + (b) x (c) }` operates on a million numbers. NumPy,
  MATLAB, APL. Claude's recommendation: it makes the Unicode symbol plan meaningful, and it is
  cheap if fusion is deferred.
- **C — middle ground.** Looks like A; the compiler understands an array type well enough to
  optimise later. Fortran 90+, Julia.

Why it blocks things: it decides whether `·`, `×`, `⊙`, `⊗` are reserved as distinct operations
(dot, cross, elementwise, tensor) or aliased to plain multiplication. Note `×` has *already*
been decided as a multiplication alias, which spends it.

Asymmetry worth knowing: A → B later is a middle-end rewrite; B → A is merely wasted effort.

Two sub-questions if B:
- **Shapes in the type system** (`vec[3] + vec[4]` is a compile error — the Futhark/Dex design,
  consistent with every other choice made so far, but a serious step up in difficulty) or
  **checked at runtime** (simpler, crashes later)?
- **Broadcasting** — does `math { (a) + 1 }` add 1 to every element? Convenient; also a famous
  source of shape bugs.

---

## Awaiting the sample Tankun offered

**2. The error message template.** See [diagnostics.md](diagnostics.md) for what it needs to
settle: error codes, source excerpts and labels, suggestion lines, placement relative to
Informer output.

---

## Syntax

**3. Which Unicode symbols are aliases.** "Probably every, idk". `×` confirmed. The safe list
(`≤ ≥ ≠ ÷ √ π ∞ ≈ ∧ ∨ ¬ ∈ →`) versus the ones worth reserving — blocked on #1.

**4. LaTeX notation — which reading?** `\(2^{3}\)` could mean grouping braces on `^` (cheap,
easy) or genuinely embedded LaTeX math mode (`\frac`, `\sum`, `\int` — a signature feature, and
a second parser). Note `\` is already the escape character.

**5. Is `math { }` required for all arithmetic**, or only when the extended symbols are wanted?

**6. Is `[…]` how all calls work** — `add[1 2]`, space-separated, no commas anywhere?

**7. Is juxtaposition-concatenation general** or specific to `print`? Does `(x)` interpolate in
any string context?

**8. Does `\` escape outside strings** too?

**9. Confirm the whitespace rule** for `x`-as-multiplication (INFERRED in
[syntax.md](syntax.md)). The whole `( )` design depends on it.

**10. Approve the `.`-versus-decimal-point rule** (PROPOSED in [syntax.md](syntax.md)).

**11. Approve the `]`-inside-comment fix** — bracket counting plus `\]` escape (PROPOSED).

**12. Does `#` still comment to end of line** when not followed by `[`? (INFERRED.)

**13. Indexing syntax.** Discussion used `(a)[(i)]`, which clashes with `[…]` for calls.

**14. Function definitions, control flow, and the mutation statement.** `set 'x' = …` and
`loop while … { }` are Claude's invented placeholders, not decisions.

---

## Types

**15. `float` and `rat`** — deferred by Tankun, but both are wanted by the stated domains. Type
names not chosen.

**16. Review the sign algebra** table in [types.md](types.md) — derived, not dictated.

**17. Function parameter precision** — must it be stated explicitly, since range analysis cannot
follow values across callers?

**18. Does `task:check` tolerate unresolved placeholders?** The error itself is decided; whether
`check` type-checks around them (so half-written programs verify) is not.

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
