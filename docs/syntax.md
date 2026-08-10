# Syntax

See [README.md](README.md) for the status legend.

## Guiding idea

Everything is explicitly delimited. There are almost no bare tokens: names are quoted,
values are quoted, references are bracketed, arithmetic lives in a block. The payoff is
that **AHPCL needs no reserved-word list** — a variable can be called `'print'`, `'x'`, or
`'2x'` and never collide with an operator or keyword.

## Punctuation

| Form | Meaning | Status |
|---|---|---|
| `#[…]` | Comment — block, may span lines | **DECIDED** |
| `#` | Comment to end of line, when not followed by `[` | **INFERRED** |
| `.` | Statement terminator (the role `;` plays elsewhere) | **DECIDED** |
| `,` | Extends the current statement instead of ending it | **DECIDED** |
| `\` | Escape character | **DECIDED** |
| `'…'` | A name, or a literal value | **DECIDED** |
| `"…"` | Text string; `\"` escapes a quote | **DECIDED** |
| `(name)` | Reference the variable called *name* | **DECIDED** |
| `(expr)` | Group a subexpression | **DECIDED** |
| `[…]` | Call arguments | **DECIDED** for `print`; **OPEN** whether all calls |
| `math { … }` | Arithmetic block | **DECIDED** |
| `[n bit]` | Precision, after the name in a declaration | **DECIDED** |

### The `.` / decimal-point clash — **PROPOSED**

`.` terminates statements, but it is also the decimal point in a numbers language. Proposed
lexer rule: **a `.` followed by a digit is a decimal point; otherwise it terminates the
statement.**

Consequences to accept if adopted: `.5` must be written `0.5`, and `3.` cannot mean `3.0`.
Also pre-commits `.` away from any future `thing.field` member syntax.

## Comments — **DECIDED**

Line-based, with an optional count. The `#[…]` block form was **scrapped** on 2026-08-10.

| Form | Effect | Lines |
|---|---|---|
| `#` | This line | 1 |
| `#3` | This line and the 2 below | 3 total |
| `#+3` | This line and the 3 below | 4 total |

A bare number is a **total** line count; `+N` counts **additional** lines beyond this one.

```
#3 this comment covers
   these two
   lines as well
print["Hello, World!"].
```

### Recorded assumptions — **INFERRED**

- A `#` marker on a line already inside a commented span is inert text, not a nested comment.
- Blank lines count toward the total.
- `#1` and `#+0` are legal, both meaning the same as bare `#`.

### Also decided

- **There is no `#-3`.** Comments never count upward.
- **`#` immediately followed by digits is always a count**, with no special case for prose.
  `#3 bugs remaining` comments three lines, as the rule says. Write `# 3 bugs remaining` —
  with a space — for a plain one-line comment. No warning, no mitigation: the space is the
  programmer's responsibility.

- **Overrunning the end of the file is an error.** `#10` with four lines left does not clamp.
  Placeholder message, explicitly temporary: *"What the heck am I supposed to do?"* — to be
  rewritten once the Error Handler template exists. See [diagnostics.md](diagnostics.md).

### Placeholders — **removed**

There is no placeholder syntax. The *"placeholder not yet resolved"* error was dropped on
2026-08-10 along with the `#[…]` comment form it depended on.

## Declarations — **DECIDED**

```
var:TYPE 'name' = 'value'.
var:TYPE 'name' [n bit] = 'value'.
```

```
var:num 'x' = '1000'.
var:num 'x' [8 bit] = '20'.
```

Quoting the value is **mandatory** outside `math { }`. `var:num 'x' = 1000.` is illegal.

All variables are **mutable** (see [types.md](types.md)).

### Extending with `,` — **DECIDED**

`,` extends a statement; `.` ends it. One declaration can therefore introduce several
variables, which are **separate variables of the same type**:

```
var:num 'x' = '1000', 'y' = '2000'.
```

The type comes from the shared header. Precision sits *after* each name, so each name carries
its own precision slot (**INFERRED**):

```
var:int 'x' [32 bit] = '1000', 'y' [8 bit] = '20'.
```

This rule is language-wide, and the CLI uses the identical meaning — see [cli.md](cli.md).

## References vs grouping — **DECIDED**

`( )` does double duty, and one whitespace rule keeps it unambiguous:

```
(2x)                       # a single unbroken token → the variable named 2x
((x) x 20)                 # contains a spaced operator → a grouped expression
math { 87 x ((x) x 20) }
```

### The whitespace rule — **INFERRED**

`x` is multiplication **only with whitespace on both sides**. `2x` is one name; `2 x 4` is
arithmetic. This is what makes `(2x)` and `((x) x 20)` coexist. Raku uses the same trick for
its own `x` operator.

Read off Tankun's examples; never stated outright. **Confirm before building the lexer** —
everything about `( )` depends on it.

## Math blocks — **DECIDED**

Inside `math { }`, numbers are written bare (no quotes):

```
var:num '2x' = math { 10 x (x) }.
var:num 'y'  = math { 1 + 2 x 4 × 10 }.
var:num 'z'  = math { 87 x ((x) x 20) }.
```

Variables must still be referenced with `( )`. `math { 10 x x }` is wrong; the second `x`
is another multiplication sign, not the variable.

**OPEN:** is `math { }` required for *all* arithmetic, or only when the extended symbol set
is wanted? A useful property if it is required: `x`-as-multiplication then exists only
inside math blocks, so elsewhere `x` is unambiguously just a name.

## Operators

| Operation | Spellings | Status |
|---|---|---|
| Multiply | `*`, `×`, `x` (spaced) | **DECIDED** |
| Power | `^`, `**`, `xx` | **DECIDED** |
| Add / subtract | `+`, `-` | **DECIDED** (implied throughout) |

Multiple spellings per operation are intentional. The cost is on readers, not the compiler —
a canonicalising formatter (`ahpcl fmt`) is the usual answer. **PROPOSED**, not scheduled.

### Unicode aliases — **OPEN**

Tankun: "probably every, idk". `×` is confirmed. Everything else is open.

Safe to alias — nothing else uses them:

```
≤  ≥  ≠  ÷  √  π  ∞  ≈  ∧  ∨  ¬  ∈  →
```

**Worth reserving instead of aliasing**, because they are genuinely distinct operations in
real mathematics — but only if arrays become first-class values:

| Symbol | Distinct meaning |
|---|---|
| `·` | dot product → a scalar |
| `×` | cross product → a vector |
| `⊙` | elementwise (Hadamard) product |
| `⊗` | tensor / Kronecker product |
| `∘` | function composition |

Note the conflict: `×` is already **DECIDED** as a multiplication alias, which spends it.
Blocked on the array-model question in [open-questions.md](open-questions.md).

### Lookalike characters — **PROPOSED**

Unicode has visually identical characters that will arrive via copy-paste from web pages and
PDFs: `−` (U+2212 MINUS SIGN) vs `-`, `×` (U+00D7) vs the letter `x`, `′` vs `'`. The lexer
should name them explicitly rather than failing vaguely:

```
error: unexpected character '−' (U+2212 MINUS SIGN)
  help: did you mean '-' (U+002D HYPHEN-MINUS)?
        this often happens when copying from a web page
```

## Identifiers — **DECIDED**

Unicode names are allowed: `'Δx'`, `'ความเร็ว'`, `'θ'`.

Because names are quoted, they are unconstrained — `'2x'` (leading digit) is legal, and so
are names that shadow operators or keywords.

**PROPOSED:** follow UAX #31 for which characters may appear in names, and warn on
confusable pairs such as Cyrillic `А` vs Latin `A`.

## Output — **DECIDED**

```
print[(x)].
print["The variable \"x\" is " (x) " and that is that."].
```

Items inside `print[…]` are **space-separated, no commas**.

**INFERRED:** adjacent items concatenate. **OPEN:** whether that is specific to `print` or
a general rule, and whether `(x)` interpolation works in any string context.

## LaTeX notation — **OPEN**

Tankun wrote `\(2^{3}\)` as the "real world" power notation. Two readings, never resolved:

- **Reading A** — LaTeX-style grouping braces on `^`: `2^{3}`, `x^{n+1}`. Cheap, familiar,
  removes precedence guesswork.
- **Reading B** — actual LaTeX math embedded in source:
  `let total = \( \sum_{i=1}^{n} price_i \)`. A second notation inside the language:
  `\frac`, `\sqrt`, `\sum`, `\int`, Greek escapes. Genuinely a signature-feature idea; also
  a second parser, and `\sum` needs to bind its index variable and become a loop.

Note `\` is already **DECIDED** as the escape character, which Reading B would contest.

## Not yet designed

Function definitions, control flow, indexing, modules, error handling, custom types.

`set 'x' = …`, `loop while … { }`, and `invariant` appear in discussion examples as
**PROPOSED** placeholders only — Claude invented those spellings. Indexing was written
`(a)[(i)]`, which clashes with `[…]` for calls; unresolved.
