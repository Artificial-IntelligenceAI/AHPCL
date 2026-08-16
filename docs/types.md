# Types, precision and verification

See [README.md](README.md) for the status legend.

## Numeric families — **DECIDED**

| Type | Holds |
|---|---|
| `num` | Any exact number, negative and positive, **including 0** |
| `rat` | Exact rationals — a numerator and a denominator, kept in lowest terms |
| `deci` | Decimals (IEEE decimal formats) |
| `int` | Integers |
| `infnum` / `∞num` | Arbitrary precision — the bignum equivalent. Both spellings legal |

`rat` was added 2026-08-12, forced by division: `math { 1 / 3 }` has no exact decimal answer.
Adding it widened `num` from "integers and decimals" to **any exact number**.

### v1 implementation bounds — **not language decisions**

Every exact type is backed by `i128`, so values are bounded at roughly 1.7 × 10³⁸. Within
that range the arithmetic is genuinely exact: decimals are scaled integers, rationals are
reduced fractions, powers and division use exact algorithms, and square roots are computed
by integer Newton iteration rather than through floating point.

Two consequences worth stating plainly:

- **`infnum` is not yet unbounded.** It is exact and behaves correctly up to the `i128`
  limit. A wider backend is a v1-stable concern.
- **Irrationals have a digit ceiling.** π, e and τ are known to 36 decimal places; square
  roots are computed to 18. Asking for more is an error rather than a silent approximation,
  because computing them from an `f64` would be wrong past the 16th digit.

### The hierarchy — **DECIDED**

```
        num
         |
        rat
         |
        deci
         |
        int
```

It follows the mathematics: every integer is a rational (`5` is `5/1`), every terminating decimal
is a rational (`2.5` is `5/2`), but not every rational is a decimal (`1/3` is not).

A narrower type satisfies a wider one, so a function taking `rat` accepts integers and decimals
with no conversion. The reverse never holds.

Rejected: `rat` as a sibling of `int` and `deci` (flatter, but passing an integer to a `rat`
parameter would fail even though every integer *is* a rational), and `rat` outside `num` entirely
(which would break generic numeric code the moment fractions appear).

## Sign refinement — **DECIDED**

Each family takes an optional sign prefix:

| Prefix | Range |
|---|---|
| *(none)* | Negative, zero, or positive |
| `+` | Strictly positive (excludes 0) |
| `-` | Strictly negative (excludes 0) |

So `+int`, `-deci`, `+infnum`, and so on. Zero lives **only** in the unprefixed types.

This is a **refinement type** — a type plus a constraint. Ada has had them for decades
(`Natural`, `Positive`); Rust's `u32` is a crude form of `+int`. Well-founded prior art.

### Unsigned comes free

Because a `+int` cannot be negative, the sign bit is unnecessary:

| Type | 8-bit range |
|---|---|
| `int [8 bit]` | −128 … 127 |
| `+int [8 bit]` | 1 … 255 |
| `-int [8 bit]` | −255 … −1 |

No separate `uint` family needed. That last row has no mainstream equivalent.

### The sign algebra — **PROPOSED**

Arithmetic must compute the result's sign refinement. The non-obvious entries:

| Expression | Result | Why |
|---|---|---|
| `+int + +int` | `+int` | |
| `+int × +int` | `+int` | |
| `-int × -int` | **`+int`** | two negatives multiply positive |
| `+int × -int` | `-int` | |
| `+int - +int` | **`int`** | `7 - 7 = 0`, `5 - 10 = -5` — subtraction always widens |
| `-num + +num` | `num` | sign unknowable |
| `-int ^ n` | `int` | `(-2)²` is positive, `(-2)³` is not — parity unknown |

Derived from Tankun's definitions rather than dictated by them; needs review.

## Precision — **DECIDED**

```
var:int 'x' [32 bit] = '1000'.
```

Widths: **8, 16, 32, 64, 128**.

`deci` follows IEEE 754, whose decimal formats are only **decimal32 / decimal64 /
decimal128** — so `deci [8 bit]` and `deci [16 bit]` must error. decimal128 gives 34
significant digits, which is what financial systems actually use.

`infnum [n bit]` is an **error** — it is unbounded by definition.

### `[n digits]` for irrationals — **DECIDED**

`infnum` accepts a **digit** count, which is how much of an irrational value you want:

```
var:infnum 'x' [100 digits] = math { π }.
var:infnum 'c' [50 digits]  = math { 2 x π x ('r') }.
```

Not a contradiction with the rule above: **bits are about storage size**, which is meaningless for
an unbounded type, while **digits are about how much of an infinite value to compute**.

Without this, `var:infnum 'x' = math { π }.` would have to be an error — and a subtle one. It is
not that `infnum` is too small for π; π is irrational, so *no* finite number of digits is exact,
and "as many as needed" never terminates. `infnum` holds exact numbers of unlimited size; π is not
exact at any size. `[n digits]` is how you say "this much of it".

**Precision belongs to the computation, not to one operand.** `math { 2 x π x ('r') }` at
50 digits carries 50 digits through the whole calculation, which is how real arbitrary-precision
arithmetic works (MPFR, mpmath). Attaching a digit count to the constant instead would give "π to
50 digits, then multiplied", whose result has a quietly different accuracy than requested.

Rejected: a selector on the constant (`math { π:100; }`) and a keyword form
(`math { π to 100 digits }`), both for that reason.

**PARKED, not rejected:** keeping π **symbolic** — `math { 2 x π }` staying exactly `2π` until
something forces digits, as Mathematica and SymPy do. Genuinely on-brand, since exact/symbolic is
one of the three stated domains, but far larger than anything decided so far: it needs symbolic
values, simplification, and rules for when they collapse to numbers.

### When precision is omitted — **DECIDED**

The compiler **never guesses a width.** It infers one by **range analysis**: examining every
use of the variable in scope and choosing a width that fits them all. If the value is not
knowable at compile time, that is an **error**.

```
var:int 'x' = '1000'.                        # knowable → inferred
var:int 'y' = math { ('x') x ('x') }.            # constant-foldable → inferred
var:int 'z' = <read at runtime>.             # ERROR: state a precision
var:int 'z' [32 bit] = <read at runtime>.    # fine
var:infnum 'z' = <read at runtime>.          # fine — explicitly unbounded
```

Range analysis looks at *all* uses, not just the initialiser, so this widens `x` to 32-bit:

```
var:int 'x' = '1000'.
var:int 'y' = math { ('x') x 100 }.    # x reaches 100,000
```

This is why `infnum` is not redundant: it is how unbounded is requested explicitly.

The Informer reports every inference — see [diagnostics.md](diagnostics.md).

Function **parameters** may state precision, because parameters are ordinary declarations — see
[syntax.md](syntax.md). That matters, since range analysis cannot follow a value across every
possible caller.

## Literal types — **DECIDED**

Numeric literals are **polymorphic until pinned by context** (the "Package 3" model — Swift
and Haskell do versions of this). The surrounding annotation decides:

```
var:deci  'a' = '0.1'.    # exact decimal
var:num   'b' = '0.1'.    # narrowed by inference
```

If nothing in the surroundings pins a type, that is an **error** — no silent default. So
`print[math { 0.1 + 0.2 }]` is rejected until a type is supplied.

`num` therefore means "must be numeric; narrow by inference; error if you can't."

**OPEN:** what width the compiler picks for a literal in isolation — the *smallest* fitting
width is tight but brittle, so range analysis over all uses is the **DECIDED** mechanism.
The interaction with mutation is settled below.

## Mutability — **DECIDED**

**Everything is mutable.** No immutable-by-default, no `const` distinction.

The consequence, accepted deliberately: sign refinements must be verified **flow-sensitively**
— the compiler tracks what is provably true about each variable at each point in the program,
because a promise like `+int` can be broken by any later assignment.

## Overflow — **DECIDED**

Overflow is an **error**.

Because unstated precision is inferred or rejected, and `infnum` is available, overflow can
essentially only occur where a fixed width was explicitly requested. Correct by default; fast
on request.

## Verifying refinements — **DECIDED**

When the compiler must prove a refinement survives mutation, it tries three layers in order:

```
1. All values known at compile time?
      → execute the loop/code and observe. Exact answer.
      → runs to completion, NO step budget.
2. Not knowable?
      → interval (range) analysis with widening. Sound; needs no theorem prover.
3. Neither proved it?
      → insert a runtime check.
```

Layer 1 is compile-time execution, as in Zig's `comptime` or C++ `constexpr`. It is unusually
powerful in AHPCL because the precision rules already push the whole language toward
compile-time-known values.

Layer 2 tracks each variable's possible range and iterates to a fixed point, with *widening*
to guarantee termination. Worked example — proving `+int` holds for a countdown:

```
enter loop:        n ∈ [100, 100]
condition n > 1:   n ∈ [2, 100]
body n = n - 1:    n ∈ [1, 99]
back to top:       n ∈ [1, 100]      ← stable, fixed point reached
⇒ n is never 0 or negative. +int verified, with no solver.
```

Its known limit: intervals track variables independently, so relationships *between*
variables (`a > b` evolving together) cannot be expressed. It fails to layer 3, which is the
safe direction.

### Layer 1 has no cap — **DECIDED**

Compile-time evaluation runs until it succeeds, however long that takes. Instead of a budget,
the Informer narrates progress live, reports elapsed time, and prints a command for skipping
to the next layer.

Two facts this rests on: a compile-time interpreter runs perhaps 100–1000× slower than
compiled code, so a loop taking 1 second at runtime can take ~10 minutes to evaluate; and
progress output must be rate-limited (~250 ms) or printing costs more than the evaluation.

**Where no human is watching** (CI, editors running `check` on every keystroke, piped output),
behaviour is **set by the caller**, not sniffed from the environment — via
`flag:loop-evaluation = limit` on the command line. See [cli.md](cli.md).

**OPEN:** the default when the caller says nothing, and whether it varies per task
(`build` unbounded vs `check` limited).

### Implementation notes (2026-08-12)

All three layers are built, in `crates/ahpcl-sema/src/verify.rs`, over the interval
analysis in `interval.rs`.

Reaching a fixed point needs **two phases**, which is standard abstract interpretation:

* **Widening** guarantees termination — a range that keeps moving outward jumps straight
  to unbounded rather than creeping one step per round forever.
* **Narrowing** then recovers precision. Widening over-approximates: the documented
  countdown widens to `[-∞, 100]`, and re-running the body without widening pulls it back
  to the true `[1, 100]`.

The loop condition is re-applied at the top of every round, which is what actually proves
the countdown stays positive.

**Width comes from every use, not the initialiser.** Verification runs two passes: the
first gathers the range a variable takes across all its assignments, the second reports
from it. Otherwise a counter declared `= '0'` and accumulated to 5050 would be inferred as
8-bit.

**"Not knowable at compile time" means *from input*.** A value flowing from `read`,
`parse` or `clock` genuinely cannot be known, and requires a stated width. A function call
or a selector has a range the analysis simply does not track — a limit of the analysis,
not of the program — so those default to 64-bit and say so rather than erroring.

**Width inference applies to `int` only.** A decimal's width is an IEEE *format* chosen
for significant digits, not something derived from a value's range; rationals have no
width at all. Inferring one from an integer range would be a category error.

**Layer 3 is enforced by the interpreter and by generated code**, not merely announced.
A refinement that cannot be proved is checked on every assignment, and a broken promise
stops the program with `AHPCL-SIGN-0004`. An earlier build reported "runtime check
inserted" and inserted nothing.

**A refinement constrains the result, not each operand.** `math { 0 - 5 }` assigned to a
`+int` is broken by its answer, not by the `0`; and `math { ('n') > 0 }` compares against
an ordinary zero. Pinning the refinement onto operands made the documented idiom for
keeping a `+int` positive unwritable.

**A sign-only mismatch is not a type error.** The sign algebra is conservative —
`+int - +int` widens to `int`, because `7 - 7` is 0 — so rejecting in the type checker
would pre-empt the very thing verification exists to decide. Assignments defer to the
verifier; call arguments and handbacks stay strict, because the analysis does not reason
across function boundaries.

### Layer 4: `invariant` clauses — **PROPOSED, roadmap**

Programmer-supplied loop invariants, verified by induction — the compiler checks the claim
holds on entry and is preserved by each iteration, then uses it as a fact.

```
loop:while math { ('n') > 1 } invariant math { ('n') >= 1 } { … }
```

This is what SPARK (avionics, rail), F\* (shipped TLS code), Dafny and Liquid Haskell do. The
full version wants an SMT solver such as Z3. Purely additive — it composes on top of layers
1–3 and breaks nothing. Not scheduled.

## Arrays — **DECIDED: model B**

**Arrays are first-class values**, and arithmetic works on whole collections at once — the
NumPy / MATLAB / APL model, rather than writing loops over elements (C, Rust) or a
compiler-blessed array type inside an otherwise scalar language (Fortran, Julia).

### Bare references reduce — **DECIDED**

A bare array reference in arithmetic **sums its elements**. Operating position-by-position
requires an explicit `:all;` selector (see [syntax.md](syntax.md)):

```
math { ('a') + ('b') }              # sum(a) + sum(b) — a single number
math { ('a'):all; + ('b'):all; }    # elementwise: matching positions added
math { ('a') + 1 }                  # sum(a) + 1
math { ('a'):all; + 1 }             # 1 added to every element
```

The **one exception** is the array operators `·`, `×`, `⊙`, `⊗`, which imply `:all;` — see
[syntax.md](syntax.md).

A redundancy worth knowing: `math { ('a'):all; x ('b'):all; }` and `math { ('a') ⊙ ('b') }` are the
same operation, spelled two ways.

It supersedes
earlier wording in this file that showed `math { ('a') + ('b') ⊙ ('c') }` acting on whole arrays;
under the rule as decided, that expression sums.

Broadcasting is therefore **scalar-only, and only under `:all;`**.

This is what gives `·`, `×`, `⊙`, `⊗` real jobs — see [syntax.md](syntax.md).

### Dimensionality — **DECIDED**

**N-dimensional arrays underneath, with 1-D and 2-D as named cases.** Julia's approach: one
general array type, with the common shapes given their own names and their own operators.

| Shape | Name | Carries |
|---|---|---|
| 0-D | scalar | ordinary arithmetic |
| 1-D | vector | `·` dot, `×` cross (3 elements only), `⊙` elementwise |
| 2-D | matrix | matrix multiplication |
| N-D | tensor | the general case |

"Array" is the storage word; vector/matrix/tensor describe shape and mathematical role.

Operators are therefore **shape-dependent**: `·` requires 1-D, `×` requires two 3-element
vectors, matrix multiplication requires 2-D operands whose inner dimensions agree.

**Matrix multiplication is not elementwise** and must be a distinct operation — the classic
NumPy hazard:

```
[[1,2],[3,4]]    ·   [[5,6],[7,8]]  =  [[19,22],[43,50]]
[[1,2],[3,4]]    ⊙   [[5,6],[7,8]]  =  [[5,12],[21,32]]
```

**OPEN:** which symbol matrix multiplication gets, and the type names — `vec` was Claude's
invention, and "vector" is ambiguous because C++ and Rust use it to mean "growable array".

Fusion — collapsing `a + b ⊙ c` into a single pass over memory instead of three — is
**explicitly deferred**. Correct first, fast later. Naive lowering is acceptable.

Sign refinements extend elementwise: a vector of `+int` means every element is positive, so
its sum and its dot product with another `+int` vector are both `+int`.

**The sign is written after the rank**, because it refines the *element* type:

```
var:vector:+num 'widths' [3] = {'3', '5', '2'}.
```

Found while building the parser (2026-08-12): the docs said "a vector of `+int`" without
saying where the `+` goes. `+vector:num` would describe a "positive vector", which means
nothing; `vector:+num` describes a vector of positive numbers, which is the intent.

**OPEN:** array type syntax and array literals. `var:vec:num 'a' = …` appears in discussion
but `vec` is Claude's invention, not a decision.

### Shapes — **DECIDED**

**Shapes live in the type when knowable, with an explicit opt-out when not** — the same rule
already used for precision, where `infnum` is the "I am not bounding this" marker.

```
var:matrix:num 'a' [3, 4] = …
var:matrix:num 'b' [4, 5] = …
var:matrix:num 'c' = math { ('a') · ('b') }.    # inferred [3, 5]
```

Mismatches are **compile** errors, before the program runs:

```
error: shape mismatch — [3, 4] · [5, 2]
       inner dimensions must agree: 4 ≠ 5
```

This is the main prize: dimension mismatch is the most common bug in numerical code, and in
most languages it surfaces as a crash partway through a run.

Shapes are written `[3, 4]`, **not** `[3 x 4]` — `x` is the multiplication operator, so
`3 x 4` would evaluate to 12. Comma already means "extend", which suits a dimension list:
`[3]` is 1-D, `[3, 4]` 2-D, `[3, 4, 5]` 3-D. (**PROPOSED** notation.)

**Size-polymorphic functions** — one function accepting any size, via size variables such as
`[R, C]` — are **deferred**. Option 3 exists precisely so this isn't a prerequisite; it can be
added later, since a size-polymorphic signature is strictly more precise than an unshaped one.

### Unknown shapes — **DECIDED**

Both spellings exist:

```
var:matrix:num 'data' [?, 3] = read["measurements.csv"].
var:dynmatrix:num 'data' = read["measurements.csv"].
```

`?` marks a dimension that isn't knowable at compile time. **Partial shapes are the point** —
`[?, 3]` says "unknown row count, definitely 3 columns", which is the normal situation with
real data, and it keeps compile-time checking on the dimension you *do* know:

```
var:matrix:num 'data'    [?, 3] = read["measurements.csv"].
var:matrix:num 'weights' [4, 2] = …
math { ('data') · ('weights') }
```
```
error: shape mismatch — [?, 3] · [4, 2]
       inner dimensions must agree: 3 ≠ 4
```

Caught at compile time despite nobody knowing the row count.

`dynmatrix` is shorthand for a fully-unknown shape. **PROPOSED** distinction that would earn
the `dyn` family a job of its own rather than pure redundancy: `[?, ?]` means *2-D with
unknown sizes*, while `dyntensor` could mean *unknown number of dimensions* — something `?`
notation cannot express, since you must write one `?` per dimension.

**OPEN:** whether `dyn` prefixes every array type name (`dynvector`, `dyntensor`).



### Declaration form — **DECIDED**

Shape and precision are **two bracket groups**, shape first:

```
var:matrix:num 'm' [3, 4] [32 bit] = …
```

Scalar declarations are unchanged — precision keeps the position it already had. The groups
can't be confused, since precision always carries the word `bit`.

### Type names — **DECIDED**

Named by rank, and the name is **required**:

```
var:vector:num 'v' [3]       = …
var:matrix:num 'm' [3, 4]    = …
var:tensor:num 't' [3, 4, 5] = …
```

There is no general "any rank" name. Name and shape **cross-check each other**, so a
disagreement is an error:

```
var:matrix:num 'm' [3] = …
```
```
error: 'matrix' is 2-dimensional, but shape [3] is 1-dimensional
```

Element type is written after the rank name — `matrix:num`, a matrix *of* nums, reading as a
narrowing chain the same way `var:num` does (**INFERRED**: the order was assumed, not stated).

**OPEN:** whether `tensor` is legal at rank 1 or 2, or strictly rank 3 and above.

## Irrational results — **DECIDED**

A `deci` **may hold a rounded irrational**, and the Informer reports every rounding:

```
var:deci 'sd' [64 bit] = math { sqrt (('diffs') / ('diffs'):length;) }.
```
```
informer: main.ahpcl:9:20 — result is irrational; rounded to decimal64 (16 digits)
```

`sqrt`, `sin`, `log` and friends usually produce irrationals — √2, √10 — which no exact decimal can
hold. Without this, every square root, distance and standard deviation would have to be
`infnum [n digits]`, and `deci` would be a money type and nothing else.

**This narrows what exactness means, deliberately.** The guarantee was always about
*representation choices* — decimals not being binary floats, `0.1` meaning exactly one tenth — not
about pretending irrational numbers are representable. `0.1 + 0.2 = 0.3` still holds; `sqrt 2`
is rounded, and says so.

It is not silent: the Informer is on by default at full detail, so every rounding is reported.

Rejected: requiring `infnum [n digits]` for all irrational results (consistent, but exiles `deci`
from scientific work), and an explicit `round` operator (which fails because **whether a result is
irrational often is not knowable at compile time** — `sqrt ('x')` is exact when `'x'` is 4 and
irrational when it is 2, so `round` would be required almost everywhere).

## Booleans — **DECIDED**

`bool` holds a truth value. Comparisons produce one, and conditions require one:

```
var:bool 'is_big' = math { ('x') > 5 }.
if ('is_big') { … }.
```

Added 2026-08-12, when it turned out comparisons had no type at all — `math { ('x') > 5 }` could
not be stored, passed, or combined, and `if` had nothing to require.

It also gives `∧ ∨ ¬` (and / or / not) something to operate on.

Rejected: comparisons legal only inside conditions (no stored results, no `and`/`or`, and `if`
becomes the only place a comparison may appear); and numbers-as-booleans, where
`if math { ('x') }` compiles when `('x') > 0` was meant — precisely the mistake the language
exists to catch.

`bool` is outside the numeric hierarchy — it is not a `num`.

Literals are `'true'` and `'false'`, quoted like every other value:

```
var:bool 'ready' = 'true'.
```

## Text — **DECIDED**

`str` holds a single piece of text; `nna` ("non-numerical-array") holds many.

```
var:str 'name'  = "Alice".
var:nna 'names' = {"hello", "John Doe", "Lol😂", "8473ijldkm"}.
```

Non-numeric arrays deliberately do **not** reuse `vector` / `matrix` / `tensor`. A 2-D grid of
text is a real and common structure (a spreadsheet, a CSV, a game board), but it is not a
*matrix* — matrix operations need elements you can add and multiply with inverses, which text
has none of. The cost accepted here is two naming schemes for identical shapes.

Shapes may be **omitted when a literal determines them** — `{"a", "b"}` is unambiguously 2
elements, the same "infer when knowable" rule already used for precision.

Text is an **opaque scalar**, not a vector of characters. It has to be: `{"Alice", "Bob"}` as
char vectors would be rows of length 5 and 3 — *ragged* — and the shape system assumes
rectangles.

Precision (`[n bit]`) and sign prefixes (`+`/`-`) are numeric-only, so the type grammar must
allow types that take neither.

`nna` **is** a vector of `str` — DECIDED 2026-08-13. It is not a base of its own, so
indexing one gives a `str`:

```
var:nna 'names' = {"hello", "John Doe", "Lol😂"}.
var:str 'second' = ('names'):2;.        # John Doe
var:int 'n' [64 bit] = ('names'):length;.   # 3
```

Modelling it as a separate base meant `('names'):2;` came back as `nna`, so it could not be
assigned to the `str` it plainly was, and a literal of text would not go *into* one either.
Both needed special cases; being the type removes them. The word stays as a spelling —
`var:nna` says "an array of text" more directly than `var:vector:str` — and it still refuses
numbers, which is what "non-numerical" means.

**OPEN:** what else `nna` may hold besides text (booleans? dates?), whether one `nna` may mix
kinds, and whether the summing rule makes bare `('names')`
concatenate.

## Division — **DECIDED**

What `/` produces is **decided by context**, and ambiguity is an error — the same rule already
used for numeric literals, applied a second time rather than inventing a new one:

```
var:rat  'third' = math { 1 / 3 }.      # exactly one third
var:deci 'half'  = math { 10 / 4 }.     # 2.5
var:num  'what'  = math { 1 / 3 }.      # ERROR: rat or deci?
```

Exactness is available without being forced on ordinary arithmetic.

Rejected: always-rational (`10 / 4` giving `5/2` when `2.5` was wanted, forcing constant
conversion) and always-decimal (`1 / 3` silently losing information, which the exact/symbolic
domain exists to avoid).

**Consequence:** any function returning a division must declare a **concrete** type. `num` spans
`rat` and `deci`, so it does not pin the result:

```
func:num  'mean' [ … ] { handback math { ('values') / ('values'):length; }. }.   # ERROR: ambiguous
func:deci 'mean' [ … ] { handback math { ('values') / ('values'):length; }. }.   # fine
```

**OPEN:** integer division and remainder have no spelling.

## Deferred types — **DECIDED to defer**

- **`float`** — binary IEEE floating point. Absent so far. It is what makes numerical
  computing fast, since GPUs and SIMD units speak it natively; exact decimals are correct but
  much slower. Matters for the scientific domain.

Sign prefixes are expected to extend to it.

## A declaration must give a value — **DECIDED**

`var:int 'x'.` is an error, `AHPCL-TYPE-0005`:

```
rule conditions: 'x' is declared but never given a value.
suggested fix: give it one, as in var:int 'x' = <value>.
```

Nothing can be read before it is written, so AHPCL has no unset state — which means no
silent zero, and no read-time check on every variable in the language. It also means every
type has exactly one representation in compiled code, with nothing standing for "nothing".

Rejected alternatives: **the zero of the type** (`0`, `false`, `""`, `{}`) — convenient, but
a silent default, which is the thing AHPCL avoids; and **a real "unset" value that errors on
read** — honest, but every type gains a state and every read gains a check, for a case the
error already prevents.

## `int` is 128 bits, compiled and interpreted — **DECIDED**

An AHPCL `int` is backed by a 128-bit signed integer on both paths, so it holds up to
170141183460469231731687303715884105727. Arithmetic past that is an error, never a wrap.

The backend emitted 64-bit integers until 2026-08-13, which meant anything past about
9.2×10¹⁸ diverged: the interpreter kept computing the true value while compiled code
wrapped silently. `99999999999 x 99999999999` gave the answer modulo 2⁶⁴.

Widening the value type is only half of it. Indices, lengths and flags crossing into the
runtime keep the width the C ABI declares — a byte length in `AhpclStr`, the `failed` flag
on a rational, a dimension in a shape, the `parse` option bitmask. Widening those too broke
string printing outright, because the compiler and the runtime then disagreed about the
layout of a struct they share. **One width for values, the declared width for everything
else**, and the two must be kept apart deliberately.

## Naming one array as another is an error — **DECIDED**

```
var:vector:int 'a' [3] = {'1','2','3'}.
var:vector:int 'b' = ('a').        # AHPCL-TYPE-0006
```

```
rule conditions: 'b' would either copy this array or become another name for it, and the
                 program does not say which.
suggested fix: write ('name'):all; to copy it. Naming one array as another is not yet a way
             to share it.
```

Two readings are available and the source cannot tell them apart. `'b'` might be an
independent copy, so that changing `'a'` afterwards leaves it alone; or it might be a second
name for the same array, so that changing either changes both. Nothing in AHPCL's syntax
expresses the difference — there is no reference type, no pointer, no `&`.

The two implementations had quietly picked opposite answers: the interpreter copied, the
backend aliased. Neither was wrong, because nothing had decided.

Refusing costs nothing. The form appears in no example and no document, and `('a'):all;`
already says *copy* explicitly. Rejecting it also keeps both doors open — copy semantics or
shared semantics can still be given to it later without breaking a program that exists
today, which choosing now would not.

Rejected alternatives: **copy**, which is what the interpreter did and is probably what the
form would eventually mean — but it would have been chosen by default rather than decided,
and a silent default is the thing AHPCL avoids; and **share**, which no syntax marks, so
`change:var:int 'a':1; = '99'.` would silently alter a variable named nowhere in that line.

This is the same move as [a declaration with no value](#a-declaration-must-give-a-value--decided):
where the program has not said which of two things it means, the compiler says so instead of
picking.
