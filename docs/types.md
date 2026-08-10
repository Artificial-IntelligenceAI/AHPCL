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

```
math { (a) + (b) ⊙ (c) }        # operates on every element
math { (velocity) · (direction) }
```

This is what gives `·`, `×`, `⊙`, `⊗` real jobs — see [syntax.md](syntax.md).

Fusion — collapsing `a + b ⊙ c` into a single pass over memory instead of three — is
**explicitly deferred**. Correct first, fast later. Naive lowering is acceptable.

Sign refinements extend elementwise: a vector of `+int` means every element is positive, so
its sum and its dot product with another `+int` vector are both `+int`.

**OPEN:** array type syntax and array literals. `var:vec:num 'a' = …` appears in discussion
but `vec` is Claude's invention, not a decision.

**OPEN:** shapes in the type system vs checked at runtime; broadcasting.

## Deferred types — **DECIDED to defer**

- **`float`** — binary IEEE floating point. Absent so far. It is what makes numerical
  computing fast, since GPUs and SIMD units speak it natively; exact decimals are correct but
  much slower. Matters for the scientific domain.
- **`rat`** — exact rationals. `deci` cannot represent ⅓ (`0.333…` never terminates in base
  10), so exact fraction maths needs numerator/denominator. Matters for the symbolic domain.

Sign prefixes are expected to extend to both.
