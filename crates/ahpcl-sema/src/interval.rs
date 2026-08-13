//! Interval analysis — layer 2 of verification.
//!
//! Tracks the range a value can take, rather than its exact value. That is enough to
//! prove a refinement without a theorem prover:
//!
//! ```text
//! enter loop:        n ∈ [100, 100]
//! condition n > 1:   n ∈ [2, 100]
//! body n = n - 1:    n ∈ [1, 99]
//! back to top:       n ∈ [1, 100]      ← stable, fixed point reached
//! ⇒ n is never 0 or negative. +int verified.
//! ```
//!
//! Its known limit is honest and predictable: each variable is tracked on its own, so
//! relationships *between* variables cannot be expressed. When it cannot prove
//! something it says so, which is the safe direction to fail in.

use std::collections::HashMap;

/// A closed range. `None` on either end means unbounded in that direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interval {
    pub lo: Option<i128>,
    pub hi: Option<i128>,
}

impl Interval {
    pub const UNKNOWN: Interval = Interval { lo: None, hi: None };

    pub fn exact(v: i128) -> Interval {
        Interval { lo: Some(v), hi: Some(v) }
    }

    pub fn new(lo: i128, hi: i128) -> Interval {
        Interval { lo: Some(lo), hi: Some(hi) }
    }

    pub fn is_known(&self) -> bool {
        self.lo.is_some() && self.hi.is_some()
    }

    /// The single value, when the range has collapsed to one.
    pub fn singleton(&self) -> Option<i128> {
        match (self.lo, self.hi) {
            (Some(a), Some(b)) if a == b => Some(a),
            _ => None,
        }
    }

    pub fn contains(&self, v: i128) -> bool {
        self.lo.map_or(true, |l| v >= l) && self.hi.map_or(true, |h| v <= h)
    }

    /// Everything either range can hold. Used where two paths meet.
    pub fn join(self, other: Interval) -> Interval {
        Interval {
            lo: match (self.lo, other.lo) {
                (Some(a), Some(b)) => Some(a.min(b)),
                _ => None,
            },
            hi: match (self.hi, other.hi) {
                (Some(a), Some(b)) => Some(a.max(b)),
                _ => None,
            },
        }
    }

    /// What both ranges allow. Used to apply a condition, which narrows.
    pub fn meet(self, other: Interval) -> Interval {
        Interval {
            lo: match (self.lo, other.lo) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (Some(a), None) | (None, Some(a)) => Some(a),
                _ => None,
            },
            hi: match (self.hi, other.hi) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) | (None, Some(a)) => Some(a),
                _ => None,
            },
        }
    }

    /// Whether the range is empty, which means a branch is unreachable.
    pub fn is_empty(&self) -> bool {
        matches!((self.lo, self.hi), (Some(a), Some(b)) if a > b)
    }

    /// The trick that makes the analysis terminate.
    ///
    /// Without it, a counter climbing `[0,1]`, `[0,2]`, `[0,3]` … would iterate
    /// forever. When an endpoint keeps moving outward, jump straight to unbounded.
    pub fn widen(self, next: Interval) -> Interval {
        Interval {
            lo: match (self.lo, next.lo) {
                (Some(a), Some(b)) if b < a => None,
                (_, b) => b,
            },
            hi: match (self.hi, next.hi) {
                (Some(a), Some(b)) if b > a => None,
                (_, b) => b,
            },
        }
    }

    pub fn add(self, o: Interval) -> Interval {
        Interval {
            lo: pair(self.lo, o.lo, |a, b| a.checked_add(b)),
            hi: pair(self.hi, o.hi, |a, b| a.checked_add(b)),
        }
    }

    pub fn sub(self, o: Interval) -> Interval {
        Interval {
            lo: pair(self.lo, o.hi, |a, b| a.checked_sub(b)),
            hi: pair(self.hi, o.lo, |a, b| a.checked_sub(b)),
        }
    }

    /// Multiplication has to consider every corner, because signs flip the ordering.
    pub fn mul(self, o: Interval) -> Interval {
        let (Some(al), Some(ah), Some(bl), Some(bh)) = (self.lo, self.hi, o.lo, o.hi) else {
            return Interval::UNKNOWN;
        };
        let corners = [
            al.checked_mul(bl),
            al.checked_mul(bh),
            ah.checked_mul(bl),
            ah.checked_mul(bh),
        ];
        if corners.iter().any(Option::is_none) {
            return Interval::UNKNOWN;
        }
        let values: Vec<i128> = corners.into_iter().flatten().collect();
        Interval {
            lo: values.iter().copied().min(),
            hi: values.iter().copied().max(),
        }
    }

    pub fn neg(self) -> Interval {
        Interval {
            lo: self.hi.and_then(|v| v.checked_neg()),
            hi: self.lo.and_then(|v| v.checked_neg()),
        }
    }

    /// Strictly greater than `bound`.
    pub fn above(self, bound: i128) -> Interval {
        self.meet(Interval { lo: bound.checked_add(1), hi: None })
    }

    /// Strictly less than `bound`.
    pub fn below(self, bound: i128) -> Interval {
        self.meet(Interval { lo: None, hi: bound.checked_sub(1) })
    }

    pub fn at_least(self, bound: i128) -> Interval {
        self.meet(Interval { lo: Some(bound), hi: None })
    }

    pub fn at_most(self, bound: i128) -> Interval {
        self.meet(Interval { lo: None, hi: Some(bound) })
    }

    pub fn render(&self) -> String {
        let l = self.lo.map(|v| v.to_string()).unwrap_or_else(|| "-∞".into());
        let h = self.hi.map(|v| v.to_string()).unwrap_or_else(|| "∞".into());
        format!("[{l}, {h}]")
    }
}

fn pair(a: Option<i128>, b: Option<i128>, f: impl Fn(i128, i128) -> Option<i128>) -> Option<i128> {
    match (a, b) {
        (Some(x), Some(y)) => f(x, y),
        _ => None,
    }
}

/// What each variable can hold at one point in the program.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub vars: HashMap<String, Interval>,
}

impl State {
    pub fn get(&self, name: &str) -> Interval {
        self.vars.get(name).copied().unwrap_or(Interval::UNKNOWN)
    }

    pub fn set(&mut self, name: &str, i: Interval) {
        self.vars.insert(name.to_string(), i);
    }

    /// Where two paths meet, a variable can hold whatever either path allowed.
    pub fn join(&self, other: &State) -> State {
        let mut out = State::default();
        for (name, a) in &self.vars {
            let b = other.get(name);
            out.set(name, a.join(b));
        }
        for name in other.vars.keys() {
            if !self.vars.contains_key(name) {
                out.set(name, Interval::UNKNOWN);
            }
        }
        out
    }

    /// Widen every variable, so loop analysis reaches a fixed point.
    pub fn widen(&self, next: &State) -> State {
        let mut out = State::default();
        for (name, a) in &self.vars {
            out.set(name, a.widen(next.get(name)));
        }
        for (name, b) in &next.vars {
            if !self.vars.contains_key(name) {
                out.set(name, *b);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_tracks_ranges() {
        let a = Interval::new(1, 10);
        let b = Interval::new(2, 3);
        assert_eq!(a.add(b), Interval::new(3, 13));
        assert_eq!(a.sub(b), Interval::new(-2, 8));
        assert_eq!(a.mul(b), Interval::new(2, 30));
    }

    #[test]
    fn multiplication_considers_sign_flips() {
        // A negative range flips the ordering, so every corner has to be tried.
        let a = Interval::new(-3, 2);
        let b = Interval::new(-4, 5);
        assert_eq!(a.mul(b), Interval::new(-15, 12));
    }

    #[test]
    fn a_condition_narrows_a_range() {
        let n = Interval::new(0, 100);
        assert_eq!(n.above(1), Interval::new(2, 100));
        assert_eq!(n.below(50), Interval::new(0, 49));
    }

    #[test]
    fn the_countdown_from_the_docs_reaches_a_fixed_point() {
        // n = 100; while n > 1 { n = n - 1 }  ⇒  n ∈ [1, 100], never 0 or negative.
        //
        // Widening first, so it terminates; then narrowing, to recover precision.
        let entry = Interval::exact(100);
        let mut n = entry;
        for _ in 0..8 {
            let body = n.above(1).sub(Interval::exact(1));
            let next = n.widen(n.join(body));
            if next == n {
                break;
            }
            n = next;
        }
        // Widening over-approximates.
        assert_eq!(n.lo, None, "widened to unbounded below");

        for _ in 0..4 {
            let body = n.above(1).sub(Interval::exact(1));
            let next = entry.join(body);
            if next == n {
                break;
            }
            n = next;
        }
        assert_eq!(n, Interval::new(1, 100), "narrowing recovers the true range");
        assert!(!n.contains(0), "+int is verified: n is never 0");
    }

    #[test]
    fn widening_stops_a_climbing_range_from_iterating_forever() {
        let mut i = Interval::new(0, 1);
        for _ in 0..3 {
            let next = i.add(Interval::exact(1)).join(i);
            i = i.widen(next);
        }
        assert_eq!(i.hi, None, "an endpoint that keeps moving goes to unbounded");
        assert_eq!(i.lo, Some(0), "the stable end stays put");
    }

    #[test]
    fn states_join_across_branches() {
        let mut a = State::default();
        a.set("x", Interval::exact(1));
        let mut b = State::default();
        b.set("x", Interval::exact(5));
        assert_eq!(a.join(&b).get("x"), Interval::new(1, 5));
    }

    #[test]
    fn an_empty_range_marks_an_unreachable_branch() {
        assert!(Interval::new(5, 10).below(3).is_empty());
    }
}
