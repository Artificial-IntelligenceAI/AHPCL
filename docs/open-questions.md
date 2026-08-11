# Open questions

The running agenda. **Resume from here.**

Design decisions belong to Tankun Sriket. Ask **one question per message**; present the
trade-offs first, then a single question.

---

## Arrays — model B chosen, details outstanding

**1a. Does `dyn` prefix every array type name** (`dynvector`, `dyntensor`)? And should
`dyntensor` mean *unknown rank* — a distinction `?` notation cannot express — rather than being
a pure synonym for `[?, ?]`?

**1c. May the shape be omitted when a literal determines it?** `var:matrix:num 'm' = {{'1','2'},{'3','4'}}.`
is unambiguously `[2, 2]`, and precision already works this way. Also: how is an empty array written?

**1g. Is `tensor` legal at rank 1 or 2**, or strictly rank 3 and above?

**1h. Confirm the element-type order** — `matrix:num` was assumed, never stated.

**1i. Do selectors double as general indexing?** Would settle #13, the `(a)[(i)]` clash with
call brackets.

**1j. Is there a text type at all?** Strings appear only as `print` arguments so far — no type
name, and nothing decided about text as a value. See [types.md](types.md).

**1d. Resolved by the summing rule** — `math { (a) x (b) }` is sum(a) × sum(b), and
`math { (a):all; x (b):all; }` is elementwise, identical to `⊙`.

---

## Awaiting the sample Tankun offered

**2. The error message template.** See [diagnostics.md](diagnostics.md) for what it needs to
settle: error codes, source excerpts and labels, suggestion lines, placement relative to
Informer output.

---

## Syntax

**3. Which Unicode symbols are aliases.** "Probably every, idk". The safe list
(`≤ ≥ ≠ ÷ √ π ∞ ≈ ∧ ∨ ¬ ∈ →`) is still unconfirmed; `· × ⊙ ⊗` are now reserved operations
rather than aliases, and `∘` is unassigned.

**4. LaTeX notation — which reading?** `\(2^{3}\)` could mean grouping braces on `^` (cheap,
easy) or genuinely embedded LaTeX math mode (`\frac`, `\sum`, `\int` — a signature feature, and
a second parser). Note `\` is already the escape character.

**5. Is `math { }` required for all arithmetic**, or only when the extended symbols are wanted?

**6. Is `[…]` how all calls work** — `add[1 2]`, space-separated, no commas anywhere?

**7. Is juxtaposition-concatenation general** or specific to `print`? Does `(x)` interpolate in
any string context?

**8. Does `\` escape outside strings** too?

**9. Does `x`-as-multiplication still need whitespace?** The rule existed to tell `(2x)` the
name from `((x) x 20)` the expression. Quoted references removed that job — so what does `2x`
mean now inside `math { }`?

**11. Rewrite the temporary comment-overrun error message** once the Error Handler template exists — currently "What the heck am I supposed to do?"

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
