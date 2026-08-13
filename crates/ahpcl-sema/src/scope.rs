//! Scopes.
//!
//! Blocks create a scope: a variable declared inside `{ … }` exists from its
//! declaration to the closing brace and no further. Shadowing is legal and reported by
//! the Informer, because AHPCL names may contain spaces, emoji and lookalike
//! characters, which makes accidental shadowing easier here than in most languages.

use std::collections::HashMap;

use ahpcl_diagnostics::{BytePos, Span};

use crate::types::Type;

#[derive(Debug, Clone)]
pub struct Variable {
    pub ty: Type,
    pub declared_at: Span,
    /// Loop counters are read-only inside the body, which is what makes a counted
    /// loop provably terminating.
    pub read_only: bool,
}

#[derive(Debug, Clone)]
pub struct Function {
    pub params: Vec<Type>,
    pub param_names: Vec<String>,
    pub returns: Type,
    pub declared_at: Span,
}

#[derive(Default)]
pub struct Scopes {
    frames: Vec<HashMap<String, Variable>>,
    pub functions: HashMap<String, Function>,
}

/// What happened when a name was declared.
pub enum Declared {
    Fresh,
    /// The new declaration hides one from an enclosing scope.
    Shadows { previous: Span },
}

impl Scopes {
    pub fn new() -> Self {
        Scopes { frames: vec![HashMap::new()], functions: HashMap::new() }
    }

    pub fn push(&mut self) {
        self.frames.push(HashMap::new());
    }

    pub fn pop(&mut self) {
        self.frames.pop();
    }

    pub fn declare(&mut self, name: &str, var: Variable) -> Declared {
        let shadowed = self
            .frames
            .iter()
            .rev()
            .skip(1)
            .find_map(|f| f.get(name))
            .map(|v| v.declared_at);

        self.frames
            .last_mut()
            .expect("at least one scope")
            .insert(name.to_string(), var);

        match shadowed {
            Some(previous) => Declared::Shadows { previous },
            None => Declared::Fresh,
        }
    }

    pub fn lookup(&self, name: &str) -> Option<&Variable> {
        self.frames.iter().rev().find_map(|f| f.get(name))
    }

    /// Names currently in scope, for "did you mean" suggestions.
    pub fn visible_names(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self
            .frames
            .iter()
            .flat_map(|f| f.keys().map(String::as_str))
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// A rough closeness measure, for suggesting the name someone probably meant.
pub fn closest<'a>(target: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let mut best: Option<(usize, &str)> = None;
    for c in candidates {
        let d = edit_distance(target, c);
        // Only suggest something genuinely close.
        let limit = (target.chars().count() / 2).max(1);
        if d <= limit && best.map_or(true, |(bd, _)| d < bd) {
            best = Some((d, c));
        }
    }
    best.map(|(_, c)| c)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Convenience for reporting a position without a full span.
pub fn pos_of(span: Span) -> BytePos {
    span.start
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Base, Type};

    fn var() -> Variable {
        Variable { ty: Type::scalar(Base::Num), declared_at: Span::new(0, 1), read_only: false }
    }

    #[test]
    fn a_block_scope_ends_at_its_closing_brace() {
        let mut s = Scopes::new();
        s.push();
        s.declare("y", var());
        assert!(s.lookup("y").is_some());
        s.pop();
        assert!(s.lookup("y").is_none(), "'y' does not outlive the block");
    }

    #[test]
    fn shadowing_is_legal_and_reported() {
        let mut s = Scopes::new();
        s.declare("y", var());
        s.push();
        match s.declare("y", var()) {
            Declared::Shadows { .. } => {}
            Declared::Fresh => panic!("expected the inner 'y' to shadow the outer one"),
        }
    }

    #[test]
    fn redeclaring_in_the_same_scope_is_not_shadowing() {
        let mut s = Scopes::new();
        s.declare("y", var());
        assert!(matches!(s.declare("y", var()), Declared::Fresh));
    }

    #[test]
    fn a_close_name_is_suggested() {
        let names = ["widths", "heights", "areas"];
        assert_eq!(closest("width", names.into_iter()), Some("widths"));
        assert_eq!(closest("completely-different", names.into_iter()), None);
    }
}
