//! Runtime values, and exact arithmetic.
//!
//! The headline promise is that `0.1 + 0.2` is exactly `0.3` and `1/3` is exactly one
//! third. Both hold here, because decimals are stored as a scaled integer and
//! rationals as a reduced fraction — never as binary floating point.
//!
//! **v1 limitation, deliberate and documented:** every exact type is backed by `i128`,
//! so `infnum` is bounded in practice at around 1.7 × 10^38. A real arbitrary-precision
//! backend is a v1-stable concern; the arithmetic below is already exact within that
//! range, which is what the language's guarantees actually rest on.

use std::fmt;

/// An exact decimal: `mantissa / 10^scale`. `0.1` is `Decimal { mantissa: 1, scale: 1 }`,
/// which is why `0.1 + 0.2` lands exactly on `0.3`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decimal {
    pub mantissa: i128,
    pub scale: u32,
}

impl Decimal {
    pub fn new(mantissa: i128, scale: u32) -> Decimal {
        Decimal { mantissa, scale }
    }

    pub fn from_int(v: i128) -> Decimal {
        Decimal { mantissa: v, scale: 0 }
    }

    /// Parse `"0.1"`, `"-42"`, `".5"`. Returns `None` on anything else.
    pub fn parse(text: &str) -> Option<Decimal> {
        let text = text.trim();
        let (negative, digits) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text.strip_prefix('+').unwrap_or(text)),
        };
        if digits.is_empty() {
            return None;
        }

        let (whole, frac) = match digits.split_once('.') {
            Some((w, f)) => (w, f),
            None => (digits, ""),
        };
        if !whole.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        if whole.is_empty() && frac.is_empty() {
            return None;
        }

        let joined = format!("{whole}{frac}");
        let mantissa: i128 = joined.parse().ok()?;
        Some(Decimal {
            mantissa: if negative { -mantissa } else { mantissa },
            scale: frac.len() as u32,
        })
    }

    /// Raise both to a common scale so they can be added exactly.
    fn align(a: Decimal, b: Decimal) -> Option<(i128, i128, u32)> {
        let scale = a.scale.max(b.scale);
        let am = a.mantissa.checked_mul(pow10(scale - a.scale)?)?;
        let bm = b.mantissa.checked_mul(pow10(scale - b.scale)?)?;
        Some((am, bm, scale))
    }

    pub fn add(self, other: Decimal) -> Option<Decimal> {
        let (a, b, scale) = Decimal::align(self, other)?;
        Some(Decimal { mantissa: a.checked_add(b)?, scale })
    }

    pub fn sub(self, other: Decimal) -> Option<Decimal> {
        let (a, b, scale) = Decimal::align(self, other)?;
        Some(Decimal { mantissa: a.checked_sub(b)?, scale })
    }

    pub fn mul(self, other: Decimal) -> Option<Decimal> {
        Some(Decimal {
            mantissa: self.mantissa.checked_mul(other.mantissa)?,
            scale: self.scale + other.scale,
        })
    }

    pub fn compare(self, other: Decimal) -> Option<std::cmp::Ordering> {
        let (a, b, _) = Decimal::align(self, other)?;
        Some(a.cmp(&b))
    }

    pub fn is_zero(self) -> bool {
        self.mantissa == 0
    }

    /// Drop trailing zeros, so `0.30` prints as `0.3`.
    pub fn normalised(mut self) -> Decimal {
        while self.scale > 0 && self.mantissa % 10 == 0 {
            self.mantissa /= 10;
            self.scale -= 1;
        }
        self
    }

    pub fn to_f64(self) -> f64 {
        self.mantissa as f64 / 10f64.powi(self.scale as i32)
    }

    /// Round a float to `digits` decimal places. Used for irrational results, which
    /// the Informer reports as rounded.
    pub fn from_f64(v: f64, digits: u32) -> Decimal {
        let factor = 10f64.powi(digits as i32);
        Decimal { mantissa: (v * factor).round() as i128, scale: digits }
    }
}

fn pow10(n: u32) -> Option<i128> {
    10i128.checked_pow(n)
}

impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let d = self.normalised();
        if d.scale == 0 {
            return write!(f, "{}", d.mantissa);
        }
        let negative = d.mantissa < 0;
        let digits = d.mantissa.unsigned_abs().to_string();
        let scale = d.scale as usize;
        let padded = if digits.len() <= scale {
            format!("{}{}", "0".repeat(scale - digits.len() + 1), digits)
        } else {
            digits
        };
        let split = padded.len() - scale;
        write!(
            f,
            "{}{}.{}",
            if negative { "-" } else { "" },
            &padded[..split],
            &padded[split..]
        )
    }
}

/// An exact rational, always reduced and with a positive denominator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rational {
    pub num: i128,
    pub den: i128,
}

impl Rational {
    pub fn new(num: i128, den: i128) -> Option<Rational> {
        if den == 0 {
            return None;
        }
        let sign = if (num < 0) != (den < 0) { -1 } else { 1 };
        let (n, d) = (num.unsigned_abs(), den.unsigned_abs());
        let g = gcd(n, d).max(1);
        Some(Rational {
            num: sign * (n / g) as i128,
            den: (d / g) as i128,
        })
    }

    pub fn from_int(v: i128) -> Rational {
        Rational { num: v, den: 1 }
    }

    pub fn from_decimal(d: Decimal) -> Option<Rational> {
        Rational::new(d.mantissa, pow10(d.scale)?)
    }

    pub fn add(self, o: Rational) -> Option<Rational> {
        Rational::new(
            self.num.checked_mul(o.den)?.checked_add(o.num.checked_mul(self.den)?)?,
            self.den.checked_mul(o.den)?,
        )
    }

    pub fn sub(self, o: Rational) -> Option<Rational> {
        Rational::new(
            self.num.checked_mul(o.den)?.checked_sub(o.num.checked_mul(self.den)?)?,
            self.den.checked_mul(o.den)?,
        )
    }

    pub fn mul(self, o: Rational) -> Option<Rational> {
        Rational::new(self.num.checked_mul(o.num)?, self.den.checked_mul(o.den)?)
    }

    pub fn div(self, o: Rational) -> Option<Rational> {
        if o.num == 0 {
            return None;
        }
        Rational::new(self.num.checked_mul(o.den)?, self.den.checked_mul(o.num)?)
    }

    pub fn compare(self, o: Rational) -> Option<std::cmp::Ordering> {
        let a = self.num.checked_mul(o.den)?;
        let b = o.num.checked_mul(self.den)?;
        Some(a.cmp(&b))
    }

    pub fn to_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }
}

fn gcd(a: u128, b: u128) -> u128 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

impl fmt::Display for Rational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{}/{}", self.num, self.den)
        }
    }
}

/// An array: values in row-major order, with its shape.
#[derive(Debug, Clone, PartialEq)]
pub struct Array {
    pub items: Vec<Value>,
    pub shape: Vec<usize>,
}

impl Array {
    pub fn vector(items: Vec<Value>) -> Array {
        let n = items.len();
        Array { items, shape: vec![n] }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i128),
    Deci(Decimal),
    Rat(Rational),
    Bool(bool),
    Str(String),
    Array(Array),
    /// The value of something that hands nothing back.
    Nothing,
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "int",
            Value::Deci(_) => "deci",
            Value::Rat(_) => "rat",
            Value::Bool(_) => "bool",
            Value::Str(_) => "str",
            Value::Array(_) => "array",
            Value::Nothing => "none",
        }
    }

    pub fn is_numeric(&self) -> bool {
        matches!(self, Value::Int(_) | Value::Deci(_) | Value::Rat(_))
    }

    pub fn to_f64(&self) -> Option<f64> {
        match self {
            Value::Int(v) => Some(*v as f64),
            Value::Deci(d) => Some(d.to_f64()),
            Value::Rat(r) => Some(r.to_f64()),
            _ => None,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(v) => write!(f, "{v}"),
            Value::Deci(d) => write!(f, "{d}"),
            Value::Rat(r) => write!(f, "{r}"),
            Value::Bool(b) => write!(f, "{}", if *b { "true" } else { "false" }),
            Value::Str(s) => write!(f, "{s}"),
            Value::Nothing => write!(f, ""),
            Value::Array(a) => {
                write!(f, "{{")?;
                for (i, item) in a.items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{item}")?;
                }
                write!(f, "}}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_are_exact_where_binary_floats_are_not() {
        // The headline case. In binary floating point this is 0.30000000000000004.
        let a = Decimal::parse("0.1").unwrap();
        let b = Decimal::parse("0.2").unwrap();
        let sum = a.add(b).unwrap();
        assert_eq!(sum.normalised(), Decimal::parse("0.3").unwrap());
        assert_eq!(sum.to_string(), "0.3");
    }

    #[test]
    fn a_third_is_exact_as_a_rational() {
        let third = Rational::new(1, 3).unwrap();
        let sum = third.add(third).unwrap().add(third).unwrap();
        assert_eq!(sum, Rational::from_int(1), "three thirds are exactly one");
    }

    #[test]
    fn rationals_reduce_to_lowest_terms() {
        assert_eq!(Rational::new(2, 4).unwrap(), Rational::new(1, 2).unwrap());
        assert_eq!(Rational::new(-6, -8).unwrap().to_string(), "3/4");
        assert_eq!(Rational::new(6, -8).unwrap().to_string(), "-3/4");
    }

    #[test]
    fn division_by_zero_is_refused_rather_than_producing_infinity() {
        assert!(Rational::new(1, 0).is_none());
        assert!(Rational::from_int(1).div(Rational::from_int(0)).is_none());
    }

    #[test]
    fn decimals_parse_and_print_round_trip() {
        for text in ["0.1", "-42", "3.14159", "0.5", "1000"] {
            let d = Decimal::parse(text).unwrap();
            assert_eq!(d.to_string(), text, "round trip of {text}");
        }
        // A leading dot is legal inside math { }.
        assert_eq!(Decimal::parse(".5").unwrap().to_string(), "0.5");
    }

    #[test]
    fn decimal_multiplication_keeps_every_digit() {
        let a = Decimal::parse("0.1").unwrap();
        let b = Decimal::parse("0.02").unwrap();
        assert_eq!(a.mul(b).unwrap().to_string(), "0.002");
    }

    #[test]
    fn comparison_works_across_scales() {
        use std::cmp::Ordering;
        let a = Decimal::parse("0.10").unwrap();
        let b = Decimal::parse("0.1").unwrap();
        assert_eq!(a.compare(b), Some(Ordering::Equal));
        assert_eq!(
            Decimal::parse("0.9").unwrap().compare(Decimal::parse("0.10").unwrap()),
            Some(Ordering::Greater)
        );
    }

    #[test]
    fn overflow_is_reported_rather_than_wrapping() {
        let big = Decimal::new(i128::MAX, 0);
        assert!(big.add(Decimal::from_int(1)).is_none(), "overflow must not wrap");
    }
}
