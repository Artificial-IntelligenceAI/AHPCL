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

**Decimals are normalised after every operation.** Multiplication adds scales, so without
dropping trailing zeros a chain of operations compounds 15 digits into 30, then 60, until
the value overflows into nonsense. This was not cosmetic: an average printed as a 500-digit
integer until the runtime normalised the way the interpreter does.

## Repository

- Apache 2.0, private on GitHub
- Commits authored `Tankun Sriket <tankun.sriket@users.noreply.invalid>`
- Commit messages contain only `Tankun Sriket`, with Claude as co-author
