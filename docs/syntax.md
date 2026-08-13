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
| `#` / `#3` / `#+3` | Comment — this line, or a line count | **DECIDED** |
| `.` | Statement terminator | **DECIDED** |
| `,` | Extends the current statement instead of ending it | **DECIDED** |
| `:` … `;` | Element selector on a reference | **DECIDED** |
| `\` | Escape character | **DECIDED** |
| `'…'` | A name, or a literal value | **DECIDED** |
| `"…"` | Text string; `\"` escapes a quote | **DECIDED** |
| `('name')` | Reference the variable called *name* — quotes always | **DECIDED** |
| `(expr)` | Group a subexpression | **DECIDED** |
| `[…]` | Call arguments — space-separated, no commas | **DECIDED** |
| `math { … }` | Arithmetic block | **DECIDED** |
| `[n bit]` | Precision, after the name in a declaration | **DECIDED** |
| `[3, 4]` | Array shape, before precision in a declaration | **DECIDED** |

### The `.` / decimal-point clash — **DECIDED: context decides**

Inside `math { }`, `.` is a **decimal point**. Outside, it **terminates a statement**.

No lookahead and no special cases, because a bare decimal can only ever appear inside a math
block — everywhere else numbers are quoted (`'1000'`) or necessarily whole:

```
[3, 4]        shape — integers
[32 bit]      precision — integer
('a'):1, 3;   selector — integers
#3            comment count — integer
```

`.5` is therefore legal inside math, which the earlier digit-lookahead proposal would have cost.

This rests on one assumption: a `math { }` block never needs a statement terminator inside it.
True today — a math block holds a single expression — but revisit if that changes.

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

- **Overrunning the end of the file is an error**, code `AHPCL-LEX-0001`. `#10` with four lines
  left does not clamp. Message still the explicitly temporary *"What the heck am I supposed to
  do?"* — the template now exists, so rewriting it is actionable. See
  [diagnostics.md](diagnostics.md).

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

**Every reference is quoted. There is no bare form.** The quotes separate reference from
grouping completely:

```
('x')                      # the variable named x
(3 + 4)                    # a grouped subexpression
math { 87 x (('x') x 20) }
```

This replaced an earlier bare form (`(x)`, `(2x)`) on 2026-08-11. The bare form needed a
whitespace rule to tell a name from an expression, and forced quoting anyway for names
containing spaces, brackets or the statement terminator. Requiring quotes everywhere removes
the ambiguity entirely and makes `('.')`, `('my variable')` and `('😂')` ordinary rather than
special cases.

The cost, accepted deliberately, is verbosity — `(('x') x 20)` where `((x) x 20)` would have
done.

## Math blocks — **DECIDED**

Inside `math { }`, numbers are written bare (no quotes):

```
var:num '2x' = math { 10 x ('x') }.
var:num 'y'  = math { 1 + 2 x 4 }.
var:num 'z'  = math { 87 x (('x') x 20) }.
```

Variables must be referenced with `('…')`. `math { 10 x x }` is wrong; the second `x`
is another multiplication sign, not the variable.

### Always required — **DECIDED**

`math { }` is required for **all** arithmetic and comparison, with no bare-expression
shorthand:

```
var:num 'y' = math { 5 + 3 }.
if math { ('x') > 5 } { … }
```

The reason is that **`math { }` is a lexer mode**. Inside it, numbers are bare, `.` is a decimal
point, and `x` is an operator. Outside it, numbers are quoted, `.` terminates statements, and
`x` is just a letter. Those are two different rule sets, and one clearly marked boundary is far
simpler than letting the mode leak into unmarked places.

It is also what the `.`-versus-decimal-point decision rests on: allowing bare arithmetic outside
math blocks would put bare numbers back into ordinary statements, costing `.5` and forcing a
digit-lookahead rule.

## Operators

| Operation | Spellings | Status |
|---|---|---|
| Multiply | `*`, `x` (spaced) | **DECIDED** |
| Divide | `/`, `÷` | **DECIDED** |
| Integer divide | `//` | **DECIDED** |
| Remainder | `mod` | **DECIDED** |
| Power | `^`, `**`, `xx` | **DECIDED** |
| Add / subtract | `+`, `-` | **DECIDED** (implied throughout) |

What division *produces* is a separate question — see [types.md](types.md).

`%` was **rejected** for remainder: in mathematics it means *percent*, and a calculations language
should keep it free for that. `mod` is the mathematical name anyway, and keywords cost nothing
since names are quoted.

**INFERRED:** `var:int 'q' = math { 10 / 4 }.` is an error rather than silently truncating to 2 —
consistent with refusing information loss elsewhere. `//` is how truncation is requested.

`×` was briefly a multiplication alias and was **removed** on 2026-08-10 — it now means cross
product, below.

### Comparison — **DECIDED**

`=` means **equality inside `math { }`** and **assignment outside it** — the same context-decides
rule already used for `.` versus the decimal point, and for the same reason: math blocks are a
distinct lexer mode.

```
math { ('i') = 3 }              # is i equal to 3
var:num 'x' = math { 5 + 3 }.   # assignment
```

No new symbol, and `math { ('i') = 3 }` reads exactly like mathematics.

Rejected: `==` (a programmer's convention, not a mathematical one, in a language built around real
notation) and `≡` (most honest, but not on a keyboard, so it would need an ASCII fallback anyway).

The rest of the family — **PROPOSED**, implied but not separately confirmed:

| Operation | Spellings |
|---|---|
| Equal | `=` |
| Not equal | `!=`, `≠` |
| Less / greater | `<`, `>` |
| Less or equal | `<=`, `≤` |
| Greater or equal | `>=`, `≥` |

### Constants — **DECIDED**

Constants are **bare keywords inside `math { }`**, not variables:

```
math { 2 x π x ('r') }
math { pi }                # ASCII spelling
```

They are not variables precisely so that nothing can reassign them. Everything in AHPCL is
mutable, so a predefined variable `('π')` could be overwritten with `change:var:deci 'π' = '3'.`
and the compiler could not object. Making constants immutable would have created an exception to
"everything is mutable"; making them keywords avoids needing one.

Math mode already has bare keywords (`to`, `by`, `mod`, `and`), so this needs no new mechanism.

Rejected: predefined variables (mutable, so `π` could be redefined) and no builtin constants
(everyone writes their own π to a different number of digits, and `∞` has no expression at all).

The roster — **DECIDED**:

| Symbol | ASCII | Value |
|---|---|---|
| `π` | `pi` | 3.14159… |
| `e` | `e` | 2.71828… — Euler's number |
| `τ` | `tau` | 6.28318… — two π |

How much of an irrational you get is set by `[n digits]` on the declaration — see
[types.md](types.md).

**`∞` is not a value.** It appears only in the type name `∞num`, the Unicode spelling of `infnum`.
That keeps it out of arithmetic entirely, so the awkward cases (`∞ - ∞` is undefined, not zero)
never arise, and it needs no type able to hold it.

### Square root — **DECIDED**

A **prefix operator** inside `math { }`, not a function call. Both spellings legal:

```
math { sqrt ('x') }
math { √ ('x') }
```

It joins the prefix-operator family with `not` and unary `-` — one operand, taken on the right.

Square root asks "what number times itself gives this": `√9` is 3, `√16` is 4. Most square roots
are **irrational** — `√2` is 1.41421… with no exact decimal *and* no exact fraction — so they use
the same `[n digits]` mechanism as π:

```
var:infnum 'd' [50 digits] = math { √2 }.
var:int 'a' = 'sqrt-of-9-somehow'.     # √9 is exactly 3, so an int can hold it
```

The result type must be able to hold the answer, which existing rules already enforce — an
irrational root cannot land in an `int`.

### Maths operations are operators — **DECIDED**

Every mathematical operation is an operator inside `math { }`, never a function call. Symbols where
mathematics has them, words always:

| Operation | Word | Symbol |
|---|---|---|
| Square root | `sqrt` | `√ ('x')` |
| Absolute value | `abs` | `\|('x')\|` |
| Floor | `floor` | `⌊('x')⌋` |
| Ceiling | `ceil` | `⌈('x')⌉` |
| Trigonometry | `sin`, `cos`, `tan` | — |
| Logarithm | `log`, `ln` | — |

**`|('x')|` is why `|` was kept free** — it was deliberately rejected as a selector separator so
absolute value could have its real notation.

Word-operators with no symbol are not an oddity: `mod`, `and`, `or`, `not`, `to` and `by` are
already bare words inside `math { }`, safe because names are quoted.

This means there are **no value-returning builtin functions in the maths domain at all** — which
settles the bare-versus-quoted builtin question by making it not arise there. It remains open for
non-maths builtins such as file reading.

Rejected: splitting operations into "has real notation → operator" and "no symbol → function",
which creates two categories and a boundary question for every addition; and treating `sqrt` alone
as an operator, which would make it an arbitrary exception and leave `|x|` unused.

**OPEN:** the full roster, and which of `log`/`ln` means which base.

### Precedence — **DECIDED: standard mathematical order**

Highest binds first:

| | Operators |
|---|---|
| 1 | Brackets, and self-delimiting notation — `√‾`, `\|x\|`, `⌊x⌋`, `⌈x⌉` |
| 2 | Powers — `^` `**` `xx` |
| 3 | **Unary minus** |
| 4 | `x` `*` `/` `//` `mod` |
| 5 | `+` `-` |
| 6 | Comparisons — `<` `>` `=` `≤` `≥` `≠` |
| 7 | `not` `¬` |
| 8 | `and` `∧` |
| 9 | `or` `∨` |

The non-obvious entry is **unary minus sitting between powers and multiplication**:

```
math { -('x') xx 2 }        →  -(x²)        with x = 3, that is -9
math { -('x') x 2 }         →  (-x) x 2     with x = 3, that is -6
```

Negation loses to the power but beats the multiplication. That is genuinely what mathematics
says — `−3²` is `−9` — and it surprises people in every language that implements it correctly, so
it is worth an Informer note or prominent documentation.

An earlier proposal that *all* prefix operators simply bind tighter than binary ones was
**wrong**: it would have made `-('x') xx 2` equal `9`.

In AHPCL's favour, operands are already delimited — `('x')` is unmistakably one thing — so
`math { sqrt ('x') + 1 }` reads as `(√x) + 1` at a glance. That recovers much of the clarity the
radical's horizontal bar provides in typeset mathematics and which linear text otherwise loses.

### Associativity — **DECIDED: follow mathematics**

Powers group **right to left**; everything else groups **left to right**.

```
math { 10 - 3 - 2 }     →  (10 - 3) - 2   =  5
math { 100 / 5 / 2 }    →  (100 / 5) / 2  =  10
math { 2 xx 3 xx 2 }    →  2 xx (3 xx 2)  =  512
```

That last one is 512, not 64 — `2^3^2` is `2^(3^2)` in mathematics. Powers are the only operator
that behaves this way, which people forget, but the alternative was worse: grouping powers left to
right would make AHPCL quietly disagree with a calculator, in a language whose premise is getting
the maths right.

Rejected: uniform left-to-right (one rule, wrong answers) and requiring brackets on chained powers
(an error for something mathematics considers well-defined).

### Boolean operators — **DECIDED**

Words **and** symbols, both legal — the pattern already used for multiplication and power:

| Operation | Spellings |
|---|---|
| And | `and`, `∧` |
| Or | `or`, `∨` |
| Not | `not`, `¬` |

```
if math { ('a') > 5 and ('b') < 10 } { … }.
if math { ('a') > 5 ∧ ('b') < 10 } { … }.
if math { not ('ready') } { … }.
```

**Bare words are unambiguous inside `math { }`** — a payoff of quoted names. `to`, `by`, `mod`,
`and`, `or` and `not` can all be plain words with no whitespace rule, because a variable called
`'and'` is written `('and')` and can never be confused with the operator. Only `x` needs spaces,
being a single letter.

### Unary minus — **DECIDED**

`-` does both jobs, told apart by **position**: nothing on its left means negation, something on
its left means subtraction.

```
math { 5 - 3 }        # subtraction — two operands → 2
math { -('x') }       # negation — one operand → the opposite sign
```

Negation flips a number's sign: `5` → `-5`, `-3` → `3`, `0` → `0`. It is what makes absolute
value work — when `('x')` is `-5`, `-('x')` is `5`.

The same positional logic already sorts out `x`: where a value is expected, `-` negates; where an
operator is expected, it subtracts.

Rejected: a separate keyword (`neg ('x')`), which nobody writes maths with; and no unary minus at
all (`math { 0 - ('x') }`), which obscures intent.

Note `-` now has **three** jobs: subtraction, negation, and the negative type prefix (`-int`,
`-num`). No conflict — the prefix appears in type position, the other two in expression position.

### `x` requires whitespace — **DECIDED**

`x` is multiplication **only with a space on each side**. `2 x 4` is arithmetic; `2x4` is not.

The rule originally existed to tell the bare reference `(2x)` from the expression
`((x) x 20)`. Quoted references removed that job, and the rule was kept anyway: requiring
spaces around a *letter* used as an operator is what makes it readable, and it stays consistent
with rejecting juxtaposition for matrix multiplication. Implicit multiplication (`2x` meaning
2 × x) was rejected for the same reason — a missing operator must never become valid code.

Multiple spellings per operation are intentional. The cost is on readers, not the compiler —
a canonicalising formatter (`ahpcl fmt`) is the usual answer. **PROPOSED**, not scheduled.

### Unicode aliases — **OPEN**

Tankun: "probably every, idk". Nothing confirmed — `×` was reassigned to cross product.

Safe to alias — nothing else uses them:

```
≤  ≥  ≠  ÷  √  π  ∞  ≈  ∧  ∨  ¬  ∈  →
```

## Array literals — **DECIDED**

Braces, **nested to mirror the shape**. `{ }` is unambiguous here because `math { }` always
carries its keyword.

```
var:vector:num 'v' [3]    = {'1', '2', '3'}.
var:matrix:num 'm' [2, 2] = {{'1', '2'}, {'3', '4'}}.
```

Values keep the mandatory quoting that applies everywhere outside `math { }`.

Nesting and declared shape **cross-check each other**, the same way rank names do:

```
var:matrix:num 'm' [3, 2] = {{'1', '2'}, {'3', '4'}}.
```
```
error: literal is [2, 2] but 'm' is declared [3, 2]
```

Rejected: a flat list folded by the shape (loses the cross-check, and a 3×3 becomes nine values
with nothing marking the rows), and bare comma-extended values (collides with multi-variable
declarations, forcing the parser to peek for an `=`).

**OPEN:** whether the shape may be omitted when a literal already determines it — the same
"infer when knowable" rule already used for precision would suggest yes. Also: empty arrays.

## Element selection — **DECIDED**

A reference may carry a selector, introduced by `:` and closed by `;`:

```
math { ('a'):all; + 1 }        # add 1 to every element → an array
math { ('a'):1, 3, 9; + 1 }    # only the 1st, 3rd and 9th elements
```

`;` is needed because the selector list uses `,`, which otherwise means "extend" — so the
semicolon marks where the selection stops and the expression resumes.

**Indices are 1-based** — `1` is the first element. Consistent with mathematical notation and
with MATLAB, Julia, Fortran and R, rather than with C-family languages.

Selection results have statically computable shapes, which fits shapes-in-types: selecting 3
elements from a `vector [10]` yields a `vector [3]`.

### Higher ranks — **DECIDED**

One selector per dimension, **chained**:

```
math { ('m'):1, 3;:2, 4; }    # rows 1 and 3, then columns 2 and 4
math { ('m'):all;:2; }        # every row, column 2 only
math { ('t'):1;:2;:3; }       # scales to any rank
```

A selector with fewer dimensions than the rank means "all of the rest". Whitespace is free, so
`('m'):1, 3; :2, 4;` is the same thing and easier to read.

Each `:…;` is one self-contained operation applied to the previous result — select rows, then
select columns from *that* — so chaining composes rather than being one atomic multi-axis index.

Rejected: a single selector with an internal separator, `('m'):1, 3 | 2, 4;`. It reads better,
but **spending `|` would foreclose `|x|` for absolute value** — and the two genuinely cannot
coexist, since `|a|b|c|` is ambiguous. A forgotten separator is also silent there:
`('m'):1, 3, 2, 4;` is legal on a `[4, 4]` matrix and simply wrong.

Also rejected: dropping the repeated `:` (`('m'):1, 3; 2, 4;`). Indices can be variables, so after
a `;` the parser cannot tell whether `('b')` opens another dimension or is the next operand.

### Selector keywords — **DECIDED**

Selectors carry keywords as well as indices. `length` asks how long an array is:

```
var:vector:num 'data' [?] = read["measurements.csv"].
loop:var:int 'i' = math { 1 to ('data'):length; } { … }.
```

Without this, `[?]` shapes were nearly unusable — an array read from a file could not be looped
over, because the range had nothing to count to.

Selectors therefore do two jobs: pick elements (`:all;`, `:3;`, `:1, 3, 9;`, `:1 to 100;`) and ask
about the array (`:length;`, `:shape;`).

For higher ranks the two questions get two keywords:

```
('m'):length;      → 12          total element count of a [3, 4] matrix
('m'):shape;       → {3, 4}      a vector [2]
```

`:shape;` hands back an **ordinary vector**, so selectors compose on it with nothing new invented:

```
('m'):shape;:1;    → 3           the row count
('m'):shape;:2;    → 4           the column count

loop:var:int 'row' = math { 1 to ('m'):shape;:1; } { … }.
```

Rejected: `:length;` meaning the first dimension with chaining for the rest — chaining already
means "select within", and reusing it for dimensions would give one syntax two unrelated jobs. Also
rejected: restricting `:length;` to vectors, which would stop rank-generic code asking a simple
question.

Rejected: a quoted builtin function (`'length'[('data')]`), which would have settled the
builtin-call rule as a side effect; and a bare builtin (`length[…]`), which makes bare-versus-quoted
a per-builtin judgement.

**Note:** because `length` is not a function, the question of whether value-returning builtins are
quoted remains open.

### Ranges — **DECIDED**

```
math { ('a'):1 to 100; }
math { ('a'):1 to 100 by 2; }    # with a step
```

`..` was **unavailable**: `1..100` breaks the decimal-point rule, since a `.` not followed by a
digit terminates the statement.

Keywords cost nothing here — because names are quoted, a variable called `'to'` is written
`('to')`, so bare `to` can never collide with it. AHPCL can add keywords freely in a way most
languages cannot.

### Selectors are the indexing syntax — **DECIDED**

The number of indices decides the result's rank. **One index gives a plain value**, not a
one-element array:

```
var:vector:num 'a' [5] = {'10', '20', '30', '40', '50'}.

('a'):3;           → 30              a num
('a'):1, 3;        → {10, 30}        a vector [2]
('a'):all;         → the whole thing  a vector [5]
```

So *n* indices give *n* elements, and one index gives a value. Selectors are therefore the
general indexing mechanism — no separate syntax is needed, and the `('a')[('i')]` clash with call
brackets never arises.

Rank depends on how many indices were written, so `('a'):1;` and `('a'):1, 2;` have different
types. That is fully knowable at compile time given shapes-in-types; it is a rule to state, not a
hazard.

Rejected: selectors always producing arrays (uniform, but getting a plain number would need a
second step, and `math { ('a'):3; + 1 }` would become array-plus-scalar rather than ordinary
arithmetic); and a separate indexing syntax (two mechanisms for one job, needing spare
punctuation).

### Writing to an element — **DECIDED**

A selector on the left of a `change:`, with the **element** type stated:

```
var:vector:num 'a' [5] = {'10', '20', '30', '40', '50'}.
change:var:num 'a':3; = '99'.
```

The stated type describes what is being written, so it agrees with the value on the right — the
line reads as "change the num at position 3 of a". The selector says which part.

Multi-dimensional writes chain the same way reads do:

```
change:var:num 'm':2;:3; = '99'.
```

Rejected: stating the variable's type (`change:var:vector:num 'a':3; = '99'.`), where the line
would claim `vector:num` while assigning a single value; and disallowing element writes entirely
(clean for an array-first language, but filling a table or updating a running result would take a
whole comprehension to change one number).

**DECIDED:** a *bare* array reference in arithmetic sums its elements — see
[types.md](types.md). `:all;` is what makes an operation position-by-position.

### Array operators — **DECIDED**

Reserved as genuinely distinct operations, *not* aliases. These **imply `:all;`** — a bare
reference stays an array in their presence, rather than summing:

```
math { ('velocity') · ('direction') }      # dot product, no selector needed
```

The reason: `·`, `×`, `⊙`, `⊗` have no scalar meaning at all, so the summing rule could only
ever produce nonsense. It exists to disambiguate `+ - * x`, which genuinely do work on both.

On `[1, 2, 3]` and `[4, 5, 6]`:

| Symbol | Operation | Result |
|---|---|---|
| `⊙` | elementwise (Hadamard) product | `[4, 10, 18]` |
| `·` | dot product | `32` |
| `×` | cross product (3-element vectors only) | `[-3, 6, -3]` |
| `⊗` | tensor / Kronecker product | `[4, 5, 6, 8, 10, 12, 12, 15, 18]` |

**`·` also means matrix multiplication** — not a collision but a unification. The dot product
*is* matrix multiplication: treat `a` as a 1×3 row and `b` as a 3×1 column, multiply as
matrices, and the result is exactly `a₁b₁ + a₂b₂ + a₃b₃`. One operation; the vector case is a
special shape of it. Shapes are in the type system, so the compiler always knows which case it
is looking at.

```
math { ('velocity') · ('direction') }    # two vectors → a number
math { ('a') · ('b') }                   # [3, 4] · [4, 5] → [3, 5]
```

Real-world notation usually writes matrix products by juxtaposition (`AB`). That was rejected
deliberately: an invisible operator means a forgotten operator becomes valid code, which is the
one mistake the compiler could never catch. `·` is standard for dot product regardless.

Because `*` and `x` stay scalar-only under the summing rule, AHPCL avoids MATLAB's confusion
where `*` is matrix multiply and `.*` is elementwise.

`∘` (function composition) is still unassigned.

**OPEN:** what plain `*` or `x` does to two arrays — the same as `⊙`, or an error demanding
the explicit symbol?

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

Names may contain **anything** — Unicode, emoji, spaces, special characters, and the
delimiters themselves via `\`:

```
'Δx'        'ความเร็ว'       'θ'
'Lol😂'      'my variable'   'it\'s'
```

Because names are quoted, they are unconstrained — `'2x'` (leading digit) is legal, and so
are names that shadow operators or keywords.

### Escaping — **DECIDED**

Inside `'…'`, **only `'` and `\` need escaping**. Emoji, spaces, dots, punctuation and every
other character are literal. Same inside `"…"` for `"` and `\`.

### Referencing names — **DECIDED**

Always quoted:

```
print[('name')].
print[('😂')].
print[('my variable')].
print[('.')].
```

No name is a special case — spaces, emoji and the statement terminator all work identically,
because the quotes do the delimiting.

**PROPOSED:** warn on confusable pairs such as Cyrillic `А` vs Latin `A`.

**OPEN:** the escape list beyond `\'`, `\"`, `\\` — `\n` for a newline, and a codepoint form
such as `\u{1F602}` (which would also let lookalike characters be written explicitly rather
than pasted and hoped for).

## Output — **DECIDED**

```
print[('x')].
print["The variable \"x\" is " ('x') " and that is that."].
```

Items inside `print[…]` are **space-separated, no commas**.

**Arguments are string literals and references only** — **DECIDED**. Math blocks, function calls
and conditionals are **not** print arguments; compute first, then print:

```
var:deci 'total' = math { ('areas') }.
print["Total: " ('total')].
```

Accepted cost: showing a calculation always needs a temporary variable. Rejected alternative was
allowing any expression, which would have let `print["Total: " math { ('areas') }]` work directly.

**INFERRED:** adjacent items concatenate. **OPEN:** whether that is specific to `print` or
a general rule, and whether `('x')` interpolation works in any string context.

## LaTeX notation — **OPEN**

Tankun wrote `\(2^{3}\)` as the "real world" power notation. Two readings, never resolved:

- **Reading A** — LaTeX-style grouping braces on `^`: `2^{3}`, `x^{n+1}`. Cheap, familiar,
  removes precedence guesswork.
- **Reading B** — actual LaTeX math embedded in source:
  `let total = \( \sum_{i=1}^{n} price_i \)`. A second notation inside the language:
  `\frac`, `\sqrt`, `\sum`, `\int`, Greek escapes. Genuinely a signature-feature idea; also
  a second parser, and `\sum` needs to bind its index variable and become a loop.

Note `\` is already **DECIDED** as the escape character, which Reading B would contest.

## Changing a variable — **DECIDED**

`change:` prefixed onto the full declaration form, restating the type:

```
var:num 'x' = '1000'.
change:var:num 'x' = '2000'.
```

**The restated type is documentation for the reader** — in a large codebase you see the type at
the point of change without hunting for the declaration. Tankun's rationale: "a team thing with
big code bases."

It is therefore **required, and verified**:

```
var:num 'x' = '1000'.
change:var:int 'x' = '2000'.
```
```
error: 'x' was declared num, but this says int
```

Documentation that can drift out of sync is worse than none — if the type could differ, or were
optional, a reader could not trust it and would check the declaration anyway, defeating the
purpose.

Rejected: retyping the variable. A variable's type is what range analysis and flow-sensitive
refinement checking are built on; a type that changes partway through would force those analyses
to track a type per *program point* rather than per variable, and `('x')` would mean different
things on different lines.

Also rejected: `set 'x' = …` (a bare keyword) and `'x' = …` (no keyword at all).

**Precision is never written in a `change:`** — **DECIDED**:

```
var:int 'x' = '1000'.
change:var:int 'x' = '2000'.        # no [n bit], ever
```

Width is a whole-variable property that range analysis owns, not a per-assignment one. Requiring
it would mean stating a number the compiler inferred and you may never have written — you would
be reading Informer output to find out what to type.

**INFERRED from the language-wide `,` rule**, not separately confirmed: a change extends the same
way a declaration does.

```
change:var:num 'x' = '1', 'y' = '2'.
```

## Scoping — **DECIDED**

**Blocks create a scope.** A variable declared inside `{ … }` exists from its declaration to the
closing brace and no further; using it afterwards is `AHPCL-NAME-0001`.

```
if math { ('x') > 5 } {
    var:num 'y' = '5'.
}.
print[('y')].            # error: no variable named 'y'
```

Only *creation* is affected. Changing an outer variable from inside a block is ordinary:

```
var:num 'y' = '0'.
if math { ('x') > 5 } {
    change:var:num 'y' = '5'.
}.
print[('y')].            # fine — 'y' lives outside
```

This is a real payoff from splitting `var:` and `change:`. Where one syntax does both jobs, a
reader cannot tell whether a block creates a new variable or modifies an outer one — the
ambiguity behind Python's scoping surprises and JavaScript's `var` hoisting. AHPCL states it at
every site.

Two further benefits: tighter scopes make range analysis and flow-sensitive refinement checking
easier *and* more precise, and the loop-counter question settles itself — `('i')` vanishes at the
closing brace, so "after `1 to 10`, is `('i')` 10 or 11?" never arises.

**Shadowing is reported** by the Informer. Because AHPCL names may contain spaces, emoji and
lookalike characters, accidental shadowing is easier here than in most languages:

```
informer: main.ahpcl:9:5 — 'y' here shadows 'y' declared at 3:1
```

## Control flow — **partially decided**

### Conditionals — **DECIDED**

Bare keyword, condition in a `math { }` block, body in braces:

```
if math { ('x') > 5 } {
    print["big"].
}.
```

Rejected: `if[…]` (condition as a bracketed argument, matching `print[…]`) and `if:…` (colon
style, matching `var:num`).

### else — **DECIDED**

Chained with `,`, because a whole if/else chain is **one statement**:

```
if math { ('x') > 5 } {
    print["big"].
}, else if math { ('x') > 3 } {
    print["medium"].
}, else {
    print["small"].
}.
```

The single `.` at the end follows from the rule rather than being a thing to remember —
`,` extends, `.` ends, exactly as in declarations and on the command line.

Rejected: bare chaining (`} else if`), which works but would be the one place a statement
continues without a comma; and an `elif`/`elseif` shorthand, a third keyword for something two
existing words already say.

### Conditionals are expressions — **DECIDED**

A conditional **has a value**; you may ignore it. Used alone it looks like a statement and its
value is discarded; used in a value position it produces one. Rust works the same way.

```
var:num 'abs' = if math { ('x') < 0 } { … }, else { ('x') }.
```

An `else` is required only when the value is actually used — otherwise a branch would be
missing a value. All branches must agree on a type.

This avoids declaring a throwaway initial value purely to overwrite it, which under AHPCL's
rules would be a real value that range analysis has to account for.

**Consequence:** `,` now does two jobs inside a declaration — extending to a second *variable*
(`var:num 'y' = '1', 'z' = '2'.`) and extending an if/else *chain*. The keyword `else`
distinguishes them, so it parses; it is still two jobs for one mark.

### Block values — **DECIDED**

A block's value is marked **explicitly**, with `handback` or its short form `hb`. Both spellings
are legal.

```
var:num 'abs' = if math { ('x') < 0 } {
    print["it was negative"].
    handback math { -('x') }.
}, else {
    hb ('x').
}.
```

`handback` hands a value out of the block; it is **not** an assignment. Where the value goes
depends on where the conditional sits:

```
var:num 'abs' = if … { hb … }, else { hb … }.   # stored in 'abs'
print[if … { hb … }, else { hb … }].            # goes to print
if … { hb … }.                                   # produced, then discarded
```

Rejected: taking the **last statement** implicitly (Rust-style). It is silent — reordering two
lines would change a block's meaning, and a block that produced nothing would surface as a
confusing type error elsewhere rather than "this branch never hands back a value".

`return` was avoided deliberately: it conventionally means "exit the enclosing function", and
reusing it here would make it ambiguous once functions exist.

**OPEN:** unary minus. `-('x')` as *negation* rather than subtraction has never been decided.

### Loops — **DECIDED: two kinds**

Both a **counted** loop and a **condition** loop exist.

```
loop:var:int 'i' = math { 1 to 10 } {
    print[('i')].
}.
```

The counted form follows the declaration pattern already established by `var:` and
`change:var:`, so the counter is a properly declared variable — and its type and precision can be
stated. That matters more here than elsewhere: **the loop counter is exactly the variable
verification cares about**, since layer 1 evaluates the loop and layer 2 runs range analysis on
it. `loop:var:+int 'i'` or a pinned width is useful, not decoration.

The range needs its `math { }` like all other arithmetic — no exemption.

Note `to` and `by` appear in two contexts where numbers are bare: inside `math { }`, and inside
selectors (`('a'):1 to 100;`). Both are lexer modes; selector indices are always whole, so the
decimal-point rule is unaffected.

Rejected: a bare form (`loop 'i' 1 to 10`), which would be the only place a variable comes into
existence without `var:`; and a range-as-value form (`loop 'i' in math { 1 to 10 }`), which needs
a new `in` keyword.

### The condition loop — **DECIDED**

Same pattern; the `:` says which *kind* of loop:

```
loop:while math { ('n') > 1 } {
    change:var:int 'n' = math { ('n') - 1 }.
}.
```

`loop:var:` is counted, `loop:while` is conditional. Keeping both visibly `loop:` something puts
the distinction that matters right in the keyword — **counted loops always terminate; condition
loops can run the compiler's unbounded evaluation indefinitely.** It also leaves room for more
kinds (`loop:until`, `loop:forever`) without new top-level keywords.

Rejected: `loop while …` (the two forms of one construct punctuated differently) and a separate
`while` keyword (which would hide the fact that both are loops, and with it the verification
distinction).

### The counter is read-only — **DECIDED**

```
loop:var:int 'i' = math { 1 to 10 } {
    change:var:int 'i' = '99'.        # ERROR
}.
```

`loop:var:int 'i' = math { 1 to 10 }` therefore means **exactly ten iterations**, known before
anything runs. That guarantee is the entire reason counted loops exist as a separate kind: it is
what makes layer 1 compile-time evaluation safe and layer 2 range analysis trivially convergent.

This is the one place in AHPCL where something is not mutable. Justification: "everything is
mutable" is about variables *you* declare; a loop counter is closer to a value the loop hands you
each time round.

Rejected: making it mutable (which would collapse counted loops into the same unprovable category
as `loop:while`, so `loop:var:` would only *look* bounded), and mutable-with-an-Informer-note
(which makes the guarantee something to check rather than something structural).

The counter does **not** outlive the loop — see Scoping above.

### Loops produce arrays — **DECIDED**

A loop is an expression. Each `handback` contributes **one element**, so a loop builds an array —
what other languages call a comprehension:

```
var:vector:num 'squares' = loop:var:int 'i' = math { 1 to 10 } {
    handback math { ('i') xx 2 }.
}.
→ {1, 4, 9, 16, 25, 36, 49, 64, 81, 100}
```

**The shape falls out for free.** `1 to 10` is ten iterations known at compile time, so the result
is `vector [10]` — statically, with no analysis. A `loop:while` cannot be counted in advance, so it
produces `vector [?]`. The two loop kinds map exactly onto the known and unknown shapes already in
the type system.

A body must `handback` on **every** iteration or **none**. Mixed would leave holes, and "sometimes
an element" has no meaning under fixed shapes.

Used as a statement with no `handback`, a loop simply produces nothing — the same way a
conditional's value may be discarded.

Rejected: statement-only loops, and producing just the final `handback` (which a variable does
more clearly).

### Nesting builds higher rank — **DECIDED**

A `handback` may itself be an array, and the result gains a dimension:

```
var:matrix:num 'times_table' = loop:var:int 'i' = math { 1 to 3 } {
    handback loop:var:int 'j' = math { 1 to 4 } {
        handback math { ('i') x ('j') }.
    }.
}.
```
```
{{1, 2, 3,  4},
 {2, 4, 6,  8},
 {3, 6, 9, 12}}
```

Shapes stay computable: `[3]` outer × `[4]` inner = `matrix [3, 4]`, known at compile time. Nest
three deep for a tensor, with no new syntax. This is how a matrix defined by a formula is written.

Every `handback` must produce the **same shape** — automatic for counted loops, checked for
`loop:while`, and a mismatch is a shape error. That is also what keeps arrays rectangular, as the
type system requires.

Rejected: restricting arrays to scalars, which would leave literals as the only way to build a
matrix.

The distinction matters more here than in most languages: **counted loops always terminate**, so
layer 1 of verification can always evaluate them at compile time and layer 2 always converges.
**Condition loops might not terminate**, and they are exactly the case that can run the
compiler's unbounded evaluation until the skip flag is used. See [types.md](types.md).

Rejected: counted-only (cannot express "keep going until something happens") and
condition-only (counting becomes three lines, and every loop lands in the hard-to-analyse
category).

**OPEN:** the actual syntax — the keywords, where the loop variable goes, whether the counted
form declares its variable or uses an existing one, and whether loops are expressions with a
value the way conditionals are.

## Functions — **partially decided**

### Definitions — **DECIDED**

```
func:num 'area' [var:+num 'width', 'height'] {
    handback math { ('width') x ('height') }.
}.
```

The type after `func:` is the **return type** — what the function hands back. Parameters carry
their own types. Both follow the same rule as everywhere else: *the type describes what you get
when you use the name.*

```
var:num 'x'       →  ('x')      gives a num
func:num 'area'   →  area[…]    gives a num
```

The `handback` value is checked against the return type:

```
handback "hello".
```
```
error: 'area' produces num, but this hands back str
```

**Parameters are ordinary declarations**, which is the main reason for this shape — the entire
existing grammar works on them with no new rules:

```
func:num 'total' [var:matrix:num 'data' [?, 3] [32 bit]] { … }.
```

That parameter has an unknown-row shape *and* a precision, neither of which needed inventing.
`,` extends the list, so `[var:+num 'width', 'height']` gives both parameters the same type.

This also settles parameter precision (previously open): yes, because parameters are declarations.

Rejected: a return type after an arrow (`func 'area' […] -> num`), which needs new punctuation and
moves the type out of the position every other type occupies; and parameters without `var:`, which
would create a second declaration grammar and re-open precision, shapes and sign prefixes.

### Calls — **DECIDED**

The name is **quoted**, exactly as it was declared; arguments are space-separated with no commas,
matching `print[…]`:

```
var:num 'kitchen' = 'area'['3' '4'].
var:num 'bedroom' = 'area'[('w') ('h')].
```

Quoting is what makes any legal name callable — `'my helper'[…]` works, where a bare form could
not. Rejected: bare (`area[…]`), which re-creates the "bare when possible, quoted when not" split
that always-quoted references were introduced to remove; and reference-style (`('area')[…]`),
which costs four characters on every call and implies an indirection that does not exist.

### Builtins so far — **DECIDED**

| Builtin | Does | Hands back |
|---|---|---|
| `print[…]` | writes to output | nothing |
| `read["path"]` | reads a file | `str` |
| `parse[…]` | text to number, target type from context | a number |

`parse` uses the same context-pins-the-type rule as numeric literals and division — polymorphic
until pinned, error if nothing pins it. One builtin covers every numeric type:

```
var:str  'raw' = read["count.txt"].
var:int  'n'   = parse[('raw')].
var:deci 'x'   = parse[('raw')].
```

Keeping it a builtin rather than a maths operator is deliberate: `parse` can **fail**, and no maths
operator can. A failure stops the program — see [diagnostics.md](diagnostics.md).

Rejected: one builtin per type (`to-int`, `to-deci` — grows with every type, all doing one job) and
a maths operator (`math { num ('raw') }`, which would make the one fallible thing inside
`math { }`).

**OPEN:** what counts as parseable text.

### Bare means builtin — **DECIDED**

**A bare name is a builtin; a quoted name is user-defined.**

```
print["Hello, World!"].     # builtin statement
read["data.csv"]            # builtin function
'area'['3' '4'].            # user function
```

The two can never collide, because the syntax itself says which category you are in — someone may
define their own `'read'` and both coexist unambiguously:

```
read["data.csv"]        # the builtin
'read'["data.csv"]      # a user function called read
```

This is better than "statements bare, functions quoted", which would have needed a further rule
about which builtins are which. Value-returning builtins (`read`) and value-less ones (`print`) are
both bare.

Accepted cost: a reader must know the builtin roster to know what a bare name does, and adding a
builtin later gives meaning to a name that previously had none.

### Type nesting — **DECIDED**

The numeric families nest, and a narrower type is accepted where a wider one is expected.

```
        num
         |
        rat
         |
        deci
         |
        int
```

Sign refinements are orthogonal: `+int` is an `int` with an extra promise, so it goes anywhere an
`int` is wanted.

```
func:num 'area' [var:num 'width', 'height'] { … }.

var:int  'w' = '3'.
var:deci 'h' = '4.5'.
var:num  'a' = 'area'[('w') ('h')].     # both accepted — each is a num
```

Narrower goes in fine, because it is a promise. The reverse does not: a function demanding
`+num` will not take a plain `num`, which might be negative.

This is what lets one function serve several numeric types **without generics**. Generics would
only be needed across genuinely unrelated types (a function accepting both `num` and `str`).
Not needed for numeric domains; deferred until a real case appears.

**Every widening is reported** by the Informer:

```
informer: main.ahpcl:12:20 — 'w' passed as int where num expected; widened
```

Accepted cost: widening is common, so this will be one of the more frequent notes. Chosen
deliberately over silent widening — consistent with the Informer reporting everything the
compiler decides on your behalf.

### Functions producing nothing — **DECIDED**

`none` is the type of "hands nothing back":

```
func:none 'log' [var:str 'message'] {
    print["[log] " ('message')].
}.
```

The type slot is always filled, so `func:` has one shape with no special case.

Rejected: `?`, which **already means "unknown"** in shapes (`[?, 3]`). Unknown and absent are
different ideas the type system deliberately keeps apart, and one symbol for both would blur
them. It also reads wrong in diagnostics — *"'log' produces ?"* looks like the compiler is
unsure, where *"'log' produces none"* states a fact.

Also rejected: omitting the type (absence-means-something, and two shapes for one keyword), and
requiring every function to produce something (which would force useless values on operations
that genuinely have no result).

**OPEN:** whether `none` is legal anywhere other than a return type — `var:none 'x'` is
meaningless and presumably an error.

## Not yet designed

Indexing, modules, error handling, custom types.

`invariant` (see [types.md](types.md), verification layer 4) is still a **PROPOSED** placeholder
spelling. Indexing was written `('a')[('i')]` in discussion, which clashes with `[…]` for calls;
unresolved.
