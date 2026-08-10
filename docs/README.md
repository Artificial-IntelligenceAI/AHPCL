# AHPCL design docs

These record the language design as it is decided. Nothing here is implemented yet.

| Document | Covers |
|---|---|
| [syntax.md](syntax.md) | Lexical rules and surface syntax |
| [types.md](types.md) | The numeric type system, precision, refinements, verification |
| [diagnostics.md](diagnostics.md) | The Error Handler and the Informer |
| [toolchain.md](toolchain.md) | Implementation stack: Rust, LLVM, AOT + JIT |
| [open-questions.md](open-questions.md) | **The running agenda.** Start here to resume. |

## Status legend

Every item carries one of these. It matters: AHPCL's design belongs to Tankun Sriket, so
Claude's suggestions must never be recorded as though they were decisions.

| Marker | Meaning |
|---|---|
| **DECIDED** | Tankun decided this explicitly. |
| **INFERRED** | Read off Tankun's own code examples, but never stated outright. Needs confirmation; may be a misreading. |
| **PROPOSED** | Claude suggested it. Not accepted. Do not build on it. |
| **OPEN** | Undecided. See [open-questions.md](open-questions.md). |
