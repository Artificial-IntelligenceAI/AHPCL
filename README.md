# AHPCL

**Advanced High-Performance Calculations Language**

A programming language, built from scratch — one where numbers are taken seriously.

AHPCL targets **scientific**, **exact/symbolic**, and **financial** numerics, which means
exactness is a first-class concern rather than an afterthought. It is a hobby project.

```
var:num 'x' = '1000'.
var:num '2x' = math { 10 x ('x') }.
print["The variable \"x\" is " ('x') "."].
```

## Status

**v1 iteration, in progress.** Not a stable release — but AHPCL programs run.

```
ahpcl task:run. buildfile:examples/exactness.ahpcl.
```
```
0.1 + 0.2 = 0.3
is it exactly 0.3? true
1/3 = 1/3
three thirds = 1
sqrt 9 = 3
sqrt 2 = 1.414213562373095
```

Decimals are stored as scaled integers and rationals as reduced fractions, never as
binary floating point — which is why the first two lines say what they say.

| Stage | |
|---|---|
| Lexer | done |
| Parser | done |
| Type checker | done — hierarchy, sign refinements, shapes, precision |
| Interpreter | done — `task:run` |
| Diagnostics | done — Error Handler, Informer |
| Verification | done — three layers, interval analysis, precision inference |
| LLVM code generation | **partial** — `int`, `bool` and `deci` compile to native binaries |
| Runtime library | done — exact decimal arithmetic for native code |

```
ahpcl task:build. buildfile:examples/native.ahpcl. resultname:native. to:/tmp.
/tmp/native
```

Produces a real standalone executable — Mach-O arm64, linking only against libc.
Summing 1 to 3,000,000 takes **0.01s** native against 413ms on the interpreter.

Programs using `rat`, arrays or text are not rejected: the backend declines them and the
driver runs them on the interpreter instead, saying so.

Implementation: Rust, LLVM 22 via `inkwell`, ahead-of-time and JIT.

## Design

See [docs/](docs/) — every item is marked as decided, inferred, proposed, or open.
[docs/open-questions.md](docs/open-questions.md) is the running agenda.

## License

Apache License 2.0. See [LICENSE](LICENSE).
