//! The command line, which speaks the same syntax as the language.
//!
//! ```text
//! ahpcl task:build. buildfile:main.ahpcl, lib.ahpcl. resultname:myprogram. to:/tmp/out.
//! ```
//!
//! Values are unquoted — the shell strips quotes before we ever see them. **Each argv
//! element is one whole token**, never re-split on spaces, so a path containing spaces
//! survives ordinary shell quoting. See docs/cli.md.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub task: Option<String>,
    pub buildfiles: Vec<String>,
    pub resultname: Option<String>,
    pub to: Option<String>,
    pub flags: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    pub message: String,
    pub suggestion: String,
}

impl Command {
    fn empty() -> Self {
        Command {
            task: None,
            buildfiles: Vec::new(),
            resultname: None,
            to: None,
            flags: BTreeMap::new(),
        }
    }
}

/// Strip one trailing terminator, reporting whether the directive ended (`.`),
/// continued (`,`), or did neither.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Ending {
    Ends,
    Extends,
    None,
}

fn split_ending(s: &str) -> (&str, Ending) {
    if let Some(rest) = s.strip_suffix('.') {
        (rest, Ending::Ends)
    } else if let Some(rest) = s.strip_suffix(',') {
        (rest, Ending::Extends)
    } else {
        (s, Ending::None)
    }
}

pub fn parse<I, S>(args: I) -> Result<Command, CliError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut cmd = Command::empty();
    // Which directive a bare continuation belongs to, e.g. the `lib.ahpcl` in
    // `buildfile:main.ahpcl, lib.ahpcl.`
    let mut continuing: Option<String> = None;

    for arg in args {
        let arg = arg.as_ref();
        if arg.is_empty() {
            continue;
        }

        // Only the *last* `.` is a terminator; `main.ahpcl` keeps its own dot.
        let (body, ending) = split_ending(arg);

        let (key, value) = match body.split_once(':') {
            Some((k, v)) if !k.is_empty() && !k.contains(char::is_whitespace) => {
                (k.to_string(), v.to_string())
            }
            _ => match &continuing {
                Some(k) => (k.clone(), body.to_string()),
                None => {
                    return Err(CliError {
                        message: format!("'{arg}' is not a directive."),
                        suggestion: "directives look like task:build. or buildfile:main.ahpcl."
                            .to_string(),
                    })
                }
            },
        };

        apply(&mut cmd, &key, &value)?;

        continuing = match ending {
            Ending::Ends => None,
            Ending::Extends | Ending::None => Some(key),
        };
    }

    Ok(cmd)
}

fn apply(cmd: &mut Command, key: &str, value: &str) -> Result<(), CliError> {
    match key {
        "task" => cmd.task = Some(value.to_string()),
        "buildfile" => {
            if !value.is_empty() {
                cmd.buildfiles.push(value.to_string());
            }
        }
        "resultname" => cmd.resultname = Some(value.to_string()),
        "to" => cmd.to = Some(value.to_string()),
        "flag" => {
            let (name, val) = value.split_once('=').unwrap_or((value, "on"));
            cmd.flags
                .insert(name.trim().to_string(), val.trim().to_string());
        }
        other => {
            return Err(CliError {
                message: format!("'{other}:' is not a directive AHPCL knows."),
                suggestion: "try task:, buildfile:, resultname:, to: or flag:.".to_string(),
            })
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_simple_build_parses() {
        let cmd = parse(["task:build.", "buildfile:main.ahpcl."]).unwrap();
        assert_eq!(cmd.task.as_deref(), Some("build"));
        assert_eq!(cmd.buildfiles, vec!["main.ahpcl"]);
    }

    #[test]
    fn only_the_last_dot_is_a_terminator() {
        // `main.ahpcl` keeps its own dot; only the trailing one is stripped.
        let cmd = parse(["buildfile:main.ahpcl."]).unwrap();
        assert_eq!(cmd.buildfiles, vec!["main.ahpcl"]);
    }

    #[test]
    fn a_comma_extends_a_directive() {
        let cmd = parse(["task:build.", "buildfile:main.ahpcl,", "lib.ahpcl,", "math.ahpcl."]).unwrap();
        assert_eq!(cmd.buildfiles, vec!["main.ahpcl", "lib.ahpcl", "math.ahpcl"]);
    }

    #[test]
    fn a_path_with_spaces_survives_intact() {
        // The shell strips the quotes but keeps the argument whole, so the parser must
        // never re-split on spaces. This project's own directory is such a path.
        let cmd = parse([
            "task:build.",
            "to:/Users/ts/Advanced High-Performance Calculations Language (AHPCL)/build.",
        ])
        .unwrap();
        assert_eq!(
            cmd.to.as_deref(),
            Some("/Users/ts/Advanced High-Performance Calculations Language (AHPCL)/build")
        );
    }

    #[test]
    fn flags_take_a_value() {
        let cmd = parse(["task:build.", "flag:loop-evaluation=limit."]).unwrap();
        assert_eq!(cmd.flags.get("loop-evaluation").map(String::as_str), Some("limit"));
    }

    #[test]
    fn an_unknown_directive_is_reported_helpfully() {
        let err = parse(["nonsense:thing."]).unwrap_err();
        assert!(err.message.contains("nonsense:"));
        assert!(err.suggestion.contains("buildfile:"));
    }

    #[test]
    fn a_bare_argument_with_no_directive_is_rejected() {
        let err = parse(["main.ahpcl"]).unwrap_err();
        assert!(err.message.contains("not a directive"));
    }
}
