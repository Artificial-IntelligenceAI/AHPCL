# Toolchain

See [README.md](README.md) for the status legend.

## Decisions

| Layer | Choice | Status |
|---|---|---|
| Host language | **Rust** | **DECIDED** — safety, helpful errors, strict compiler |
| Backend | **LLVM** | **DECIDED** — going straight at it, no interim bytecode VM |
| Execution | **AOT and JIT** | **DECIDED** |
| LLVM bindings | `inkwell` | **PROPOSED** — never confirmed |

## What these force

Consequences, not further choices:

**The compiler must be a library first, a CLI second.** In-process JIT means something
embeds the compiler — a REPL, a notebook, a host application. So: a cargo workspace with the
`ahpcl` CLI as a thin shell over a driver crate. Retrofitting this boundary later is a painful
refactor.

**One codegen path, two consumers.** AOT and JIT should differ only at the end — emit an
object file, or hand the module to ORC. Any lowering that works in only one mode is a bug.

**The runtime must be linkable two ways** — statically into AOT binaries, and resolvable as
in-process symbols for JIT'd code.

**Textual LLVM IR is not an option as the interface.** Emitting `.ll` and shelling out to
`llc` is the low-friction route to AOT but cannot do in-process JIT. So a hard link against
libLLVM is required, which makes build times and CI caching a real concern.

**A REPL becomes cheap**, and for a calculations language, probably expected.

**The compiler contains an interpreter for AHPCL.** Needed anyway for constant folding, and
it is exactly what layer 1 of verification requires (see [types.md](types.md)). One piece of
work paying for three features — const-eval, verification, and the REPL.

## Recommended sequencing — **AOT done, JIT next**

**AOT first, JIT second.** AOT is built and covers every type — see below. The JIT is
untouched. Same decisions, ordered by difficulty. AOT is: build the module,
write a `.o`, call `clang` to link. The JIT means LLVM's ORC — executable memory permissions,
runtime symbol resolution, relocations — and `inkwell`'s ORC coverage is thinner than its
IR-building surface, so parts of it mean raw `llvm-sys` C calls in `unsafe` Rust. It is the
most likely thing to stall early progress.

The front end (lexer, parser, type checker) is identical regardless of backend, so it can be
built before any of this is settled.

## Risks

**LLVM version pinning.** Checked 2026-08-12: this machine has Homebrew LLVM **22.1.8**, and
`inkwell 0.10.0` offers an `llvm22-1` feature. No version lag, so the risk flagged here is
closed. `LLVM_SYS_221_PREFIX` will need setting.

**ORC binding maturity.** Budget more than a weekend for the JIT.

## GPU — **OPEN**

Never explicitly answered. Claude's recommendation was "later, designed for": no GPU work
now, but the IR avoids a handful of corners (single-address-space assumptions, pervasive
recursion or unbounded allocation in hot paths, keeping data-parallel regions
distinguishable). Roughly 10–15% extra care in IR design; nearly free insurance.

Practical note: this is an Apple Silicon machine, so there is no CUDA locally. GPU work would
mean Metal or remote NVIDIA hardware.

## The runtime library — **built**

`crates/ahpcl-runtime` is a C-ABI staticlib that generated code links against. It exists
because **an exact decimal has no native LLVM representation**: there is no machine
instruction for `0.1 + 0.2` that lands exactly on `0.3`, so the arithmetic has to live
somewhere the compiler can call.

Two implementation facts worth recording, both found the hard way:

**Decimals cross the boundary by pointer, never by value.** LLVM IR performs no platform
ABI lowering. A frontend that writes `{ i128, i32, i32 }` in a signature gets register
passing, while the AArch64 C ABI passes a 24-byte struct indirectly. The two disagree
*silently* — the call runs and does nothing. Pointers avoid the question everywhere.

**Decimal stack slots need explicit 16-byte alignment.** LLVM defaults a struct `alloca`
to 8; an `i128` requires 16, and Rust's `#[repr(C)]` layout assumes it. Reading across the
mismatch is undefined behaviour that works in release and aborts under debug assertions.

**Integer division and remainder go through the runtime.** LLVM's `sdiv`/`srem` truncate
toward zero while the interpreter is Euclidean, so `-7 // 3` was -2 natively and -3
interpreted. Routing through the runtime also makes division by zero fail rather than
being undefined.

**Booleans print as `true`/`false`**, matching the literals and the interpreter, rather
than as 0/1.

**Output is flushed on every write.** A compiled program's entry point is LLVM's C `main`,
not Rust's, so Rust's flush-on-exit never runs and buffered output would be lost.

## What compiles natively — **every type** — DECIDED

AOT covers the whole type system, not a machine-word subset. Each type reaches native code
by the cheapest representation that stays exact:

| Type | Native representation |
|---|---|
| `int`, `bool` | machine words |
| `deci`, `infnum` | `{ i128, i32, i32 }`, by pointer |
| `rat` | `{ i128, i128, i64 }` — numerator, denominator, failed — by pointer |
| `str` | `{ ptr, len }`, UTF-8 |
| arrays (`vector`/`matrix`/`tensor`) | opaque pointer to a runtime object |
| `num` | opaque pointer to a *tagged* value |

Three decisions behind that table:

**`rat` and `infnum` needed no heap.** A rational is a fixed-size pair of `i128`, so it
reuses the by-pointer ABI decimals already had. `infnum` is `i128`-backed in v1, so it
shares the decimal representation outright — its unboundedness is a v1 limit recorded in
`types.md`, not a codegen gap.

**Arrays are opaque to the compiler.** Generated code holds a pointer and never computes
an element offset; every read and write is a runtime call. That costs a call per element
and buys total immunity from the layout disagreements that made decimals silently
misbehave — the compiler and the runtime cannot disagree about a size neither one names.

**`num` is a tagged value, because it has to be.** `num` is the top of the numeric
hierarchy and holds whichever exact kind flowed into it, so no fixed layout can represent
it. It is the same tagged cell an array element is, and arithmetic promotes the narrower
side in the runtime — the same promotion the interpreter performs, so the two agree
digit for digit rather than approximately.

**Rule A is enforced in the backend too.** A *bare* array reference sums its elements,
while `('a'):all;` stays an array and the operation is elementwise. Getting this wrong is
not a slow path but a wrong answer, and it was the worst bug in the interpreter.

Values built at runtime — text, arrays, boxed `num`s — are not freed during the run; the
process exit reclaims them. Freeing needs ownership tracking, which v1 does not have. A
program that builds a million strings in a loop holds a million strings.

**Every operator compiles, not only the arithmetic ones.** Powers, `//`, `mod`, the array
operators, `sqrt`, `floor`/`ceil`, and the transcendentals all reach native code. Where a
function has no exact decimal answer — `sin`, `cos`, `tan`, `log`, `ln` — the runtime goes
through `f64` exactly as the interpreter does, so the two land on the same digits rather
than merely close ones. Square roots stay on the exact integer-Newton path, capped at the
same 18 places.

**Decimals are normalised after every operation.** Multiplication adds scales, so without
dropping trailing zeros a chain of operations compounds 15 digits into 30, then 60, until
the value overflows into nonsense. This was not cosmetic: an average printed as a 500-digit
integer until the runtime normalised the way the interpreter does.

## Repository

- Apache 2.0, private on GitHub
- Commits authored `Tankun Sriket <tankun.sriket@users.noreply.invalid>`
- Commit messages contain only `Tankun Sriket`, with Claude as co-author

## One execution path — **DECIDED**

AHPCL runs compiled code and nothing else. `task:run` compiles to a temporary binary and
runs it, exactly as `task:build` does, so what you test is what you ship. A program the
backend cannot compile is an **error**, not a quiet fall back to the interpreter — there
is no second way to run it.

### Why the interpreter still exists

It is the **test oracle**, not an execution mode. Nothing user-facing calls it.

It is kept because it is an *independently written* second implementation of the language.
Running a program both ways and diffing the output catches bugs no hand-written test can:
a test encodes the assumptions of whoever wrote it, so it cannot catch a bug that came
from those same assumptions. Two implementations disagreeing can. Most codegen bugs found
so far surfaced exactly this way — loop counters read as the wrong type, selectors indexing
flat storage instead of by dimension, a bare array reference handing back a pointer.

`crates/ahpcl-driver/tests/differential.rs` enforces the agreement over every example plus
a set of programs chosen to cross the seams between the two.

A JIT would **not** replace this: it shares the codegen backend with AOT, so it inherits
its bugs. Only a separately written implementation is a second opinion.

When the two disagree, which one is wrong is a question to answer rather than assume —
several failures have been the interpreter's fault, not the backend's.

### Linking tests against the runtime

A test that links a compiled program must build `ahpcl-runtime` itself. Declaring it as a
dev-dependency is **not** sufficient: that builds the rlib, while linking needs the
`staticlib` artifact, produced only when the crate is built as a target in its own right.
Without that step the tests link whatever `libahpcl_runtime.a` was last left in `target/`,
so a broken runtime passes unnoticed — verified by injecting a fault into `ahpcl_rat_mul`
and watching the suite stay green until the build step was added.

## LLVM optimisation — **DECIDED**

Generated code runs the standard `default<O2>` middle-end pipeline before instruction
selection. Until 2026-08-13 only the *target machine* was set to `OptimizationLevel::Default`,
which governs instruction selection and register allocation; no IR passes ran at all, so
every variable stayed a stack slot loaded and stored on every access. Enabling the pipeline
made a 20-million-iteration accumulate loop about four times faster.

Optimising is safe for AHPCL for a specific reason: **there is no floating point anywhere.**
The usual fear — that an optimiser reassociates arithmetic and changes the answer — is a
floating-point problem. Integer and pointer transforms preserve meaning exactly, and the
overflow checks are ordinary branches on an intrinsic's result, so they survive. The
differential test is what holds this to account rather than the argument above.

### What enabling it exposed

Turning passes on broke nine tests, and in both cases the bug was already in the tree —
the unoptimised build was hiding it.

1. A string's length was built as an `i128` while `str_type`'s field, and the runtime's
   `AhpclStr`, are 64-bit. `build_insert_value` **silently does nothing** when the types
   disagree, leaving the field `undef`. That is *valid* IR, so `Module::verify` does not
   catch it — the optimiser simply took `undef` at its word. `build_struct` now asserts
   each field's type, which turns it into an immediate compiler panic.
2. The selector run was passed as an array of a shared `repr(C)` struct containing `i128`.
   LLVM aligns `i128` to 8 inside a struct where Rust aligns it to 16, so every field after
   the first two sat at a different offset on each side. `:all;` kept working because it
   ignores its fields, which made it look like a selector bug rather than a layout one.
   Selectors now cross the boundary as **parallel arrays of a single primitive**, which have
   no layout left to disagree about.

Both are the same underlying mistake, and the third and fourth time it has appeared: the
compiler and the runtime holding different opinions about a shared layout, with nothing
that fails at build time. Prefer flat arrays of one primitive over a shared struct.

## Reading one array element — **the fast path**

A selector that pins every dimension yields one value, and the backend addresses that
element directly: a single index becomes an `ahpcl_array_get_*` call, and several become
one `ahpcl_array_offset` followed by the same. No descriptor arrays, no allocation.

It used to go through the general selector-run machinery, which built four descriptor
arrays and allocated a whole new array — a `Vec` of cells, a shape, and a box — to hold
the single value, then read it back out. Correct, and about 120ns and one **leaked object
per element read**. Summing a million elements took 1.23s and 3.28GB; the same loop in C
takes 2ms and 9MB. With the fast path it is 21ms and 68MB.

Three stress-test passes and a green suite never saw it, because every test asked only
whether the answer was right. It was. See the budget tests below.

## Budget tests

`crates/ahpcl-codegen/tests/budget.rs` bounds the *time* a program takes, not its output.

This is a class the differential oracle cannot cover even in principle: it compares what
two implementations print, and a leaky or quadratic implementation prints exactly the same
thing as a fast one. Correctness testing and resource testing are different questions, and
AHPCL had only ever asked the first.

The bounds are deliberately loose — tripwires for a regression that reintroduces
per-element allocation or scanning, not performance targets.

## Array memory: reference counting — **DECIDED**

An array carries a count. Storing it into a variable retains; the end of a statement
releases whatever that statement built and nobody kept; the end of a scope releases what
its variables held. Zero frees.

Counting is exact here rather than approximate, which is why it was chosen over the
alternatives: an array holds only scalars, so one can never contain another and a cycle
cannot form. The rejected options were **compiler-inserted frees** (no runtime cost, but it
needs escape analysis, and getting that wrong is a use-after-free — the one failure mode
AHPCL avoids everywhere else) and a **per-statement arena** (fastest, but anything outliving
the statement has to be copied out).

Three things about the implementation are easy to get wrong, and each was got wrong first:

- **`math { … }` is a pass-through.** Recording its result as well as its inner
  expression's counts one array twice, so it is released twice against one retain and
  freed while still live. Only expressions that *build* an array own one; a bare reference
  and a `math` wrapper do not.
- **A statement that branches away has no block to release in.** After `handback`, emitting
  a release lands after a terminator and the IR is invalid. Those temporaries are dropped
  from the list instead — a bounded leak on the way out of a block, which is the lesser
  problem.
- **The counted loop manages its scope frames by hand**, not through `scoped`, so its body
  was never released and an array declared inside a loop leaked once per iteration.

### Ownership is decided at the allocation site

Every runtime function that hands back memory is listed in `allocates()` in the backend,
which records the result as the current statement's to release. That is the whole rule.

The first attempt recorded ownership where an *expression* finished instead, and it did not
work: intermediates built inside helpers — the boxed `num` from `ahpcl_array_sum`, say —
never pass through `expr`, so they were never recorded and leaked once per iteration. It
also double-counted anything under `math { … }`, which freed live arrays. Ownership belongs
to the callee's contract, not to a position in the expression tree.

Boxed `num` values are counted the same way. A `CellBox` puts the count beside the cell with
the cell first and `repr(C)` layout, so a pointer to the box *is* a pointer to the cell and
every existing `*const Cell` signature kept working.

Measured on a loop that slices an array and sums it: **flat at 1MB** across 20,000, 200,000
and 800,000 iterations, against 418MB before any of this.

## Text is counted too

`AhpclStr` carries `{ptr, len, owner}`. `owner` is null for a literal — those bytes live in
the binary's constant data and there is nothing to free — and otherwise points at a
`StrBox` holding the bytes and their reference count.

Retain and release take the *owner*, not the string value, so a literal makes them harmless
no-ops and no branch is needed in generated code.

Before this, text built while a program ran was boxed and forgotten. A loop calling `read`
grew by the size of the file every pass: 200,000 reads of a 20KB file would have reached
about 4GB. It is now flat at 1MB.

Two functions hand back owned text through an out-pointer rather than returning it
(`ahpcl_read_file`, `ahpcl_array_get_str`), so the ownership hook on `call_runtime` cannot
see them and they record their result explicitly. Any future runtime function that hands
back memory through an out-pointer needs the same.
