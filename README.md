# AHPCL

**Advanced High-Performance Calculations Language**

A programming language, built from scratch — one where numbers are taken seriously.

AHPCL targets **scientific**, **exact/symbolic**, and **financial** numerics, which means
exactness is a first-class concern rather than an afterthought. It is a hobby project.

```
var:num 'x' = '1000'.
var:num '2x' = math { 10 x (x) }.
print["The variable \"x\" is " (x) "."].
```

## Status

Design in progress. Nothing is implemented yet.

Planned implementation: Rust host language, LLVM backend, ahead-of-time and JIT compilation.

## Design

See [docs/](docs/) — every item is marked as decided, inferred, proposed, or open.
[docs/open-questions.md](docs/open-questions.md) is the running agenda.

## License

Apache License 2.0. See [LICENSE](LICENSE).
