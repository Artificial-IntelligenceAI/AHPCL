//! The type system.
//!
//! The numeric families nest, and a narrower type is accepted where a wider one is
//! expected:
//!
//! ```text
//!         num
//!          |
//!         rat
//!          |
//!         deci
//!          |
//!         int
//! ```
//!
//! It follows the mathematics: every integer is a rational (`5` is `5/1`), every
//! terminating decimal is a rational (`2.5` is `5/2`), but not every rational is a
//! decimal (`1/3` is not). Sign refinements are orthogonal — `+int` is an `int` with an
//! extra promise. See docs/types.md.

use ahpcl_syntax::ast::{Dim, Precision, Rank, Sign, TypeRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base {
    Num,
    Rat,
    Deci,
    Int,
    /// Arbitrary precision. Spelled `infnum` or `∞num`.
    InfNum,
    Str,
    /// Non-numerical array.
    Nna,
    Bool,
    /// The type of a function that hands nothing back.
    None,
}

impl Base {
    pub fn from_name(name: &str) -> Option<Base> {
        Some(match name {
            "num" => Base::Num,
            "rat" => Base::Rat,
            "deci" => Base::Deci,
            "int" => Base::Int,
            "infnum" | "∞num" => Base::InfNum,
            "str" => Base::Str,
            "nna" => Base::Nna,
            "bool" => Base::Bool,
            "none" => Base::None,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Base::Num => "num",
            Base::Rat => "rat",
            Base::Deci => "deci",
            Base::Int => "int",
            Base::InfNum => "infnum",
            Base::Str => "str",
            Base::Nna => "nna",
            Base::Bool => "bool",
            Base::None => "none",
        }
    }

    pub fn is_numeric(self) -> bool {
        matches!(self, Base::Num | Base::Rat | Base::Deci | Base::Int | Base::InfNum)
    }

    /// Position on the numeric ladder. Lower is narrower, so it fits anywhere wider.
    ///
    /// `infnum` sits alongside `rat`: it is exact and unbounded, so it holds any
    /// integer or rational, but not an irrational.
    fn ladder(self) -> Option<u8> {
        Some(match self {
            Base::Int => 0,
            Base::Deci => 1,
            Base::Rat => 2,
            Base::InfNum => 2,
            Base::Num => 3,
            _ => return None,
        })
    }

    /// Whether a value of `self` may be used where `target` is expected.
    pub fn fits_in(self, target: Base) -> bool {
        if self == target {
            return true;
        }
        match (self.ladder(), target.ladder()) {
            (Some(a), Some(b)) => a <= b,
            _ => false,
        }
    }

    /// The narrowest type holding both. Used for the result of mixed arithmetic.
    pub fn join(self, other: Base) -> Option<Base> {
        if self == other {
            return Some(self);
        }
        let (a, b) = (self.ladder()?, other.ladder()?);
        Some(if a >= b { self } else { other })
    }
}

/// A shape, one entry per dimension. `Dim::Unknown` is `?`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shape(pub Vec<Dim>);

impl Shape {
    pub fn rank(&self) -> usize {
        self.0.len()
    }

    /// Total element count, when every dimension is known.
    pub fn total(&self) -> Option<u64> {
        self.0.iter().try_fold(1u64, |acc, d| match d {
            Dim::Known(n) => Some(acc * n),
            Dim::Unknown => None,
        })
    }

    /// Two shapes agree when no *known* dimension contradicts the other. `?` matches
    /// anything, which is what makes partial shapes useful.
    pub fn agrees_with(&self, other: &Shape) -> bool {
        if self.rank() != other.rank() {
            return false;
        }
        self.0.iter().zip(&other.0).all(|(a, b)| match (a, b) {
            (Dim::Known(x), Dim::Known(y)) => x == y,
            _ => true,
        })
    }

    pub fn render(&self) -> String {
        let inner: Vec<String> = self
            .0
            .iter()
            .map(|d| match d {
                Dim::Known(n) => n.to_string(),
                Dim::Unknown => "?".to_string(),
            })
            .collect();
        format!("[{}]", inner.join(", "))
    }

    pub fn rank_name(&self) -> &'static str {
        match self.rank() {
            1 => "vector",
            2 => "matrix",
            _ => "tensor",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Type {
    pub base: Base,
    pub sign: Option<Sign>,
    /// `None` for a scalar.
    pub shape: Option<Shape>,
    pub precision: Option<Precision>,
}

impl Type {
    pub fn scalar(base: Base) -> Type {
        Type { base, sign: None, shape: None, precision: None }
    }

    pub fn is_array(&self) -> bool {
        self.shape.is_some()
    }

    /// The element type of an array, or the type itself for a scalar.
    pub fn element(&self) -> Type {
        Type { base: self.base, sign: self.sign, shape: None, precision: self.precision.clone() }
    }

    /// Whether a value of this type may be used where `target` is expected.
    ///
    /// Narrower goes in fine, because it is a promise. The reverse does not: a
    /// function demanding `+num` will not take a plain `num`, which might be negative.
    pub fn fits_in(&self, target: &Type) -> bool {
        if !self.base.fits_in(target.base) {
            return false;
        }
        if !sign_fits(self.sign, target.sign) {
            return false;
        }
        match (&self.shape, &target.shape) {
            (None, None) => true,
            (Some(a), Some(b)) => a.agrees_with(b),
            _ => false,
        }
    }

    /// True when this is strictly narrower, so passing it widens. The Informer
    /// reports every widening.
    pub fn widens_to(&self, target: &Type) -> bool {
        self.fits_in(target) && (self.base != target.base || self.sign != target.sign)
    }

    pub fn render(&self) -> String {
        let sign = match self.sign {
            Some(Sign::Positive) => "+",
            Some(Sign::Negative) => "-",
            None => "",
        };
        match &self.shape {
            Some(shape) => format!(
                "{}:{sign}{} {}",
                shape.rank_name(),
                self.base.name(),
                shape.render()
            ),
            None => format!("{sign}{}", self.base.name()),
        }
    }
}

/// A narrower sign satisfies a wider one, never the reverse.
fn sign_fits(from: Option<Sign>, to: Option<Sign>) -> bool {
    match (from, to) {
        (_, None) => true,
        (Some(a), Some(b)) => a == b,
        (None, Some(_)) => false,
    }
}

/// Build a checker type from a syntactic one. `None` when the base name is unknown —
/// the parser has already reported that.
///
/// A rank name makes the type an array even when no shape is written: `vector:num`
/// with no `[…]` is a vector of unknown length, which is what lets a literal or a
/// comprehension determine the shape.
pub fn from_type_ref(ty: &TypeRef, shape: Option<&Vec<Dim>>, precision: Option<&Precision>) -> Option<Type> {
    let base = Base::from_name(&ty.base)?;
    let shape = match (shape, ty.rank) {
        (Some(dims), _) => Some(Shape(dims.clone())),
        (None, Some(rank)) => {
            let n = rank.dimensions().unwrap_or(3);
            Some(Shape(vec![Dim::Unknown; n]))
        }
        // `nna` is "non-numerical-array": it is an array by definition, so it carries a
        // shape even when no rank name is written.
        (None, None) if base == Base::Nna => Some(Shape(vec![Dim::Unknown])),
        (None, None) => None,
    };
    Some(Type {
        base,
        sign: ty.sign,
        shape,
        precision: precision.cloned(),
    })
}

/// The rank name and the shape cross-check each other.
pub fn rank_matches(rank: Rank, shape: &Shape) -> bool {
    match rank.dimensions() {
        Some(n) => shape.rank() == n,
        None => shape.rank() >= 3,
    }
}

// ── the sign algebra ────────────────────────────────────────────────────────

/// The sign of a sum. Two positives stay positive; mixed signs are unknowable.
pub fn sign_add(a: Option<Sign>, b: Option<Sign>) -> Option<Sign> {
    match (a, b) {
        (Some(Sign::Positive), Some(Sign::Positive)) => Some(Sign::Positive),
        (Some(Sign::Negative), Some(Sign::Negative)) => Some(Sign::Negative),
        _ => None,
    }
}

/// Subtraction **always** widens: `7 - 7` is 0 and `5 - 10` is negative, so even
/// `+int - +int` is only `int`.
pub fn sign_sub(_a: Option<Sign>, _b: Option<Sign>) -> Option<Sign> {
    None
}

/// Two negatives multiply positive.
pub fn sign_mul(a: Option<Sign>, b: Option<Sign>) -> Option<Sign> {
    match (a, b) {
        (Some(Sign::Positive), Some(Sign::Positive)) => Some(Sign::Positive),
        (Some(Sign::Negative), Some(Sign::Negative)) => Some(Sign::Positive),
        (Some(Sign::Positive), Some(Sign::Negative)) => Some(Sign::Negative),
        (Some(Sign::Negative), Some(Sign::Positive)) => Some(Sign::Negative),
        _ => None,
    }
}

/// `(-2)²` is positive and `(-2)³` is not, and the parity is not known from the sign
/// alone — so a negative base gives an unknown sign.
pub fn sign_pow(base: Option<Sign>) -> Option<Sign> {
    match base {
        Some(Sign::Positive) => Some(Sign::Positive),
        _ => None,
    }
}

pub fn sign_neg(a: Option<Sign>) -> Option<Sign> {
    match a {
        Some(Sign::Positive) => Some(Sign::Negative),
        Some(Sign::Negative) => Some(Sign::Positive),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(base: Base) -> Type {
        Type::scalar(base)
    }

    fn signed(base: Base, sign: Sign) -> Type {
        Type { base, sign: Some(sign), shape: None, precision: None }
    }

    #[test]
    fn narrower_numerics_fit_into_wider_ones() {
        assert!(t(Base::Int).fits_in(&t(Base::Deci)));
        assert!(t(Base::Deci).fits_in(&t(Base::Rat)));
        assert!(t(Base::Int).fits_in(&t(Base::Num)));
        assert!(t(Base::Rat).fits_in(&t(Base::Num)));
    }

    #[test]
    fn wider_numerics_do_not_fit_into_narrower_ones() {
        assert!(!t(Base::Num).fits_in(&t(Base::Int)));
        assert!(!t(Base::Rat).fits_in(&t(Base::Deci)));
    }

    #[test]
    fn a_promise_goes_in_but_never_comes_out() {
        // +int is an int with an extra promise, so it fits where int is wanted.
        assert!(signed(Base::Int, Sign::Positive).fits_in(&t(Base::Int)));
        // A plain num might be negative, so it cannot satisfy +num.
        assert!(!t(Base::Num).fits_in(&signed(Base::Num, Sign::Positive)));
    }

    #[test]
    fn text_and_numbers_do_not_mix() {
        assert!(!t(Base::Str).fits_in(&t(Base::Num)));
        assert!(!t(Base::Num).fits_in(&t(Base::Str)));
        assert!(!t(Base::Bool).fits_in(&t(Base::Num)), "bool is outside the numeric hierarchy");
    }

    #[test]
    fn unknown_dimensions_agree_with_anything() {
        let known = Shape(vec![Dim::Known(3), Dim::Known(4)]);
        let partial = Shape(vec![Dim::Unknown, Dim::Known(4)]);
        assert!(known.agrees_with(&partial));
        assert!(partial.agrees_with(&known));
    }

    #[test]
    fn a_known_dimension_mismatch_is_caught_even_beside_an_unknown() {
        let a = Shape(vec![Dim::Unknown, Dim::Known(3)]);
        let b = Shape(vec![Dim::Known(4), Dim::Known(2)]);
        assert!(!a.agrees_with(&b), "3 and 2 contradict each other");
    }

    #[test]
    fn different_ranks_never_agree() {
        assert!(!Shape(vec![Dim::Known(3)]).agrees_with(&Shape(vec![Dim::Known(3), Dim::Known(1)])));
    }

    #[test]
    fn subtraction_always_widens() {
        // 7 - 7 is 0, and 5 - 10 is negative, so +int - +int is only int.
        assert_eq!(sign_sub(Some(Sign::Positive), Some(Sign::Positive)), None);
    }

    #[test]
    fn two_negatives_multiply_positive() {
        assert_eq!(
            sign_mul(Some(Sign::Negative), Some(Sign::Negative)),
            Some(Sign::Positive)
        );
    }

    #[test]
    fn a_negative_base_gives_an_unknown_sign_under_a_power() {
        assert_eq!(sign_pow(Some(Sign::Negative)), None);
        assert_eq!(sign_pow(Some(Sign::Positive)), Some(Sign::Positive));
    }

    #[test]
    fn rank_names_cross_check_shapes() {
        assert!(rank_matches(Rank::Vector, &Shape(vec![Dim::Known(3)])));
        assert!(!rank_matches(Rank::Matrix, &Shape(vec![Dim::Known(3)])));
        assert!(rank_matches(Rank::Tensor, &Shape(vec![Dim::Known(2), Dim::Known(2), Dim::Known(2)])));
        assert!(!rank_matches(Rank::Tensor, &Shape(vec![Dim::Known(3), Dim::Known(4)])));
    }
}
