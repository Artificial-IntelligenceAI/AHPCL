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

## Recommended sequencing — **PROPOSED**

**AOT first, JIT second.** Same decisions, ordered by difficulty. AOT is: build the module,
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

## Repository

- Apache 2.0, private on GitHub
- Commits authored `Tankun Sriket <tankun.sriket@users.noreply.invalid>`
- Commit messages contain only `Tankun Sriket`, with Claude as co-author
