# Types, precision and verification

See [README.md](README.md) for the status legend.

## Numeric families — **DECIDED**

| Type | Holds |
|---|---|
| `num` | Integers and decimals, negative and positive, **including 0** |
| `int` | Integers |
| `deci` | Decimals (IEEE decimal formats) |
| `infnum` | Arbitrary precision — the bignum equivalent |

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

### When precision is omitted — **DECIDED**

The compiler **never guesses a width.** It infers one by **range analysis**: examining every
use of the variable in scope and choosing a width that fits them all. If the value is not
knowable at compile time, that is an **error**.

```
var:int 'x' = '1000'.                        # knowable → inferred
var:int 'y' = math { (x) x (x) }.            # constant-foldable → inferred
var:int 'z' = <read at runtime>.             # ERROR: state a precision
var:int 'z' [32 bit] = <read at runtime>.    # fine
var:infnum 'z' = <read at runtime>.          # fine — explicitly unbounded
```

Range analysis looks at *all* uses, not just the initialiser, so this widens `x` to 32-bit:

```
var:int 'x' = '1000'.
var:int 'y' = math { (x) x 100 }.    # x reaches 100,000
```

This is why `infnum` is not redundant: it is how unbounded is requested explicitly.

The Informer reports every inference — see [diagnostics.md](diagnostics.md).

**OPEN:** function parameters almost certainly must state precision explicitly, since
analysis cannot follow a value across all possible callers. Not confirmed.

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

**OPEN — in flight:** what happens where no human is watching (CI, editors running `check`
on every keystroke, piped output). An unbounded loop there hangs with nobody to intervene.

### Layer 4: `invariant` clauses — **PROPOSED, roadmap**

Programmer-supplied loop invariants, verified by induction — the compiler checks the claim
holds on entry and is preserved by each iteration, then uses it as a fact.

```
loop while math { (n) > 1 } invariant math { (n) >= 1 } { … }
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
math { (a) + (b) }              # sum(a) + sum(b) — a single number
math { (a):all; + (b):all; }    # elementwise: matching positions added
math { (a) + 1 }                # sum(a) + 1
math { (a):all; + 1 }           # 1 added to every element
```

The **one exception** is the array operators `·`, `×`, `⊙`, `⊗`, which imply `:all;` — see
[syntax.md](syntax.md).

A redundancy worth knowing: `math { (a):all; x (b):all; }` and `math { (a) ⊙ (b) }` are the
same operation, spelled two ways.

It supersedes
earlier wording in this file that showed `math { (a) + (b) ⊙ (c) }` acting on whole arrays;
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
[[1,2],[3,4]] matmul [[5,6],[7,8]]  =  [[19,22],[43,50]]
[[1,2],[3,4]]    ⊙   [[5,6],[7,8]]  =  [[5,12],[21,32]]
```

**OPEN:** which symbol matrix multiplication gets, and the type names — `vec` was Claude's
invention, and "vector" is ambiguous because C++ and Rust use it to mean "growable array".

Fusion — collapsing `a + b ⊙ c` into a single pass over memory instead of three — is
**explicitly deferred**. Correct first, fast later. Naive lowering is acceptable.

Sign refinements extend elementwise: a vector of `+int` means every element is positive, so
its sum and its dot product with another `+int` vector are both `+int`.

**OPEN:** array type syntax and array literals. `var:vec:num 'a' = …` appears in discussion
but `vec` is Claude's invention, not a decision.

### Shapes — **DECIDED**

**Shapes live in the type when knowable, with an explicit opt-out when not** — the same rule
already used for precision, where `infnum` is the "I am not bounding this" marker.

```
var:matrix:num 'a' [3, 4] = …
var:matrix:num 'b' [4, 5] = …
var:matrix:num 'c' = math { (a) matmul (b) }.    # inferred [3, 5]
```

Mismatches are **compile** errors, before the program runs:

```
error: shape mismatch — [3, 4] matmul [5, 2]
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
math { (data) matmul (weights) }
```
```
error: shape mismatch — [?, 3] matmul [4, 2]
       inner dimensions must agree: 3 ≠ 4
```

Caught at compile time despite nobody knowing the row count.

`dynmatrix` is shorthand for a fully-unknown shape. **PROPOSED** distinction that would earn
the `dyn` family a job of its own rather than pure redundancy: `[?, ?]` means *2-D with
unknown sizes*, while `dyntensor` could mean *unknown number of dimensions* — something `?`
notation cannot express, since you must write one `?` per dimension.

**OPEN:** whether `dyn` prefixes every array type name (`dynvector`, `dyntensor`).

**OPEN:** broadcasting — does `math { (a) + 1 }` add 1 to every element?

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

## Deferred types — **DECIDED to defer**

- **`float`** — binary IEEE floating point. Absent so far. It is what makes numerical
  computing fast, since GPUs and SIMD units speak it natively; exact decimals are correct but
  much slower. Matters for the scientific domain.
- **`rat`** — exact rationals. `deci` cannot represent ⅓ (`0.333…` never terminates in base
  10), so exact fraction maths needs numerator/denominator. Matters for the symbolic domain.

Sign prefixes are expected to extend to both.
