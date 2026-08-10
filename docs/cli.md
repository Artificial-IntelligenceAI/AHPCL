# Command line interface

See [README.md](README.md) for the status legend.

## Guiding idea — **DECIDED**

**The CLI speaks the same syntax as the language.** Directives are `key:value`, statements end
with `.`, and `,` extends rather than ends.

```
ahpcl task:build. buildfile:main.ah. resultname:myprogram. to:/Users/ts/build.
```

Binary name is `ahpcl` — **INFERRED**, written `ahcpl` once, assumed a slip.

## Directives — **DECIDED**

| Directive | Purpose |
|---|---|
| `task:` | What to do — `build`, `check` |
| `buildfile:` | Source file(s) to compile |
| `resultname:` | Name of the resulting output file |
| `to:` | Full path to write the result to |
| `flag:` | Compiler flags |

## `,` extends, `.` ends — **DECIDED**

A comma continues the current directive; a full stop closes it. So multi-file builds need no
new syntax:

```
ahpcl task:build. buildfile:main.ah, lib.ah, math.ah. resultname:myprogram.
```

And flags are one directive with comma-separated assignments:

```
ahpcl task:build. flag:loop-evaluation = limit, verbose = true.
```

## Values are unquoted — **DECIDED**

The CLI is a *dialect*: values carry no quotes. This is forced from outside the language —
the shell strips single quotes before the program ever sees them, so `flag:x = 'limit'`
would arrive as `limit` regardless.

```
ahpcl task:build. flag:loop-evaluation = limit.
```

**PROPOSED:** a stray quote is tolerated rather than an error.

### Paths with spaces — **PROPOSED, load-bearing**

The parser must treat **each argv element as one whole token** and never re-split on spaces.
Shell quoting then handles paths containing spaces, because the shell removes the quotes but
keeps the argument intact:

```
ahpcl task:build. to:"/Users/ts/Advanced High-Performance Calculations Language (AHPCL)/build".
```

`to:` receives the full path, spaces included. If the CLI ever joins argv with spaces and
re-parses, every path with a space breaks — this project's own directory included.

## Flags

| Flag | Values | Status |
|---|---|---|
| `loop-evaluation` | `limit`, …others unnamed | **DECIDED** that `limit` exists |

Controls layer 1 of verification — compile-time evaluation, which by default runs to
completion with no cap. See [types.md](types.md).

**OPEN:** the rest of the value vocabulary (something like `unlimited` and `off`), where the
actual numeric limit is given when limited, and whether defaults vary per task
(`task:build` unbounded vs `task:check` limited).

## Open

- Is `task:build buildfile:….` one directive with `task:` as a header, or two directives?
  Recorded as two.
- Do `resultname:` and `to:` apply to `task:check`, which produces no output?
- A project manifest (Cargo-style) declaring the entry point, so bare `ahpcl task:build.`
  works. Later; complements `buildfile:` rather than replacing it.
