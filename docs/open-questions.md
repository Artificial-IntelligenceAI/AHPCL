# Open questions

The running agenda. **Resume from here.**

Design decisions belong to Tankun Sriket. Ask **one question per message**; present the
trade-offs first, then a single question.

---

## Arrays — model B chosen, details outstanding

**1a. Does `dyn` prefix every array type name** (`dynvector`, `dyntensor`)? And should
`dyntensor` mean *unknown rank* — a distinction `?` notation cannot express — rather than being
a pure synonym for `[?, ?]`?

**1f. Where does the shape go in a declaration**, given precision already uses `[…]`? Two
bracket groups (`'m' [3, 4] [32 bit]`), shape attached to the type (`matrix[3, 4]:num`), or
combined.

**1b. Broadcasting** — does `math { (a) + 1 }` add 1 to every element? Convenient; also a
famous source of shape bugs.

**1c. Array type syntax, type names, and array literals.** `var:vec:num 'a' = …` appears in
discussion, but `vec` is Claude's invention — and "vector" is ambiguous, since C++ and Rust use
it to mean "growable array" with no geometric meaning.

**1e. Which symbol does matrix multiplication get?** It is distinct from `⊙` elementwise and
from `·` dot. Common choices elsewhere are `@` (Python), `*` (MATLAB), and `·`.

**1d. What do plain `*` and `x` do to two arrays** — the same as `⊙`, or an error demanding
the explicit symbol?

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

**9. Confirm the whitespace rule** for `x`-as-multiplication (INFERRED in
[syntax.md](syntax.md)). The whole `( )` design depends on it.

**10. Approve the `.`-versus-decimal-point rule** (PROPOSED in [syntax.md](syntax.md)).

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
