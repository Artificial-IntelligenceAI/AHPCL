//! The AHPCL runtime.
//!
//! Native code cannot hold an exact decimal or rational in a machine register — there
//! is no such LLVM type. So generated code calls into these functions, which do the
//! arithmetic exactly and hand back the same representation the interpreter uses.
//!
//! Everything here is `extern "C"` and `#[no_mangle]`, because LLVM-generated code
//! calls it by symbol name.
//!
//! A decimal is a `(mantissa, scale)` pair meaning `mantissa / 10^scale`.
//!
//! **Decimals are passed by pointer, never by value.** LLVM IR does not perform platform
//! ABI lowering — a frontend that writes `{ i128, i32, i32 }` in a signature gets
//! register passing, while the AArch64 C ABI requires a 24-byte struct to be passed
//! indirectly. The two disagree silently and the call goes nowhere. Pointers sidestep
//! the question on every platform.

#![allow(clippy::missing_safety_doc)]

use std::ffi::c_char;
use std::io::Write;

/// Write and flush immediately.
///
/// The entry point of a compiled program is LLVM-generated C `main`, not Rust's, so
/// Rust's flush-on-exit never runs. Without this, output is buffered and lost.
pub(crate) fn emit(text: &str) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
}

pub mod array;

/// Stop the program through the Error Handler, from inside the runtime.
pub(crate) fn fail_with(code: &str, message: &str) -> ! {
    let code = std::ffi::CString::new(code).unwrap();
    let msg = std::ffi::CString::new(message).unwrap();
    unsafe { ahpcl_fail(code.as_ptr(), msg.as_ptr()) }
}

/// How many digits a division keeps when nothing says otherwise. The backend uses the
/// same number, so native and interpreted division agree digit for digit.
pub(crate) const DEFAULT_DIVISION_DIGITS: u32 = 15;

/// Apply one of the four arithmetic operations to two decimals.
pub(crate) fn deci_apply(op: u32, a: AhpclDecimal, b: AhpclDecimal) -> AhpclDecimal {
    let out = match op {
        array::OP_ADD => Some(raw_add(a, b)),
        array::OP_SUB => Some(raw_sub(a, b)),
        array::OP_MUL => Some(raw_mul(a, b)),
        array::OP_POW => {
            let e = deci_whole(b);
            match e {
                Some(n) if n >= 0 && n <= u32::MAX as i128 => deci_pow(a, n as u32),
                // A fractional exponent has no exact decimal answer; the interpreter
                // falls back to floating point here, and so does this.
                _ => decimal_from_f64(deci_to_f64(a).powf(deci_to_f64(b)), DEFAULT_DIVISION_DIGITS),
            }
        }
        array::OP_INTDIV | array::OP_MOD => {
            if b.mantissa == 0 {
                fail_with("AHPCL-RUN-0002", "division by zero");
            }
            let (x, y, _) = match align(a, b) {
                Some(v) => v,
                None => fail_with("AHPCL-PREC-0004", "this decimal arithmetic overflowed"),
            };
            Some(if op == array::OP_INTDIV {
                AhpclDecimal::ok(x.div_euclid(y), 0)
            } else {
                // The remainder keeps the scale the operands were aligned to.
                let scale = a.scale.max(b.scale);
                AhpclDecimal::ok(x.rem_euclid(y), scale)
            })
        }
        _ => Some(deci_div(a, b, DEFAULT_DIVISION_DIGITS)),
    };
    match out {
        Some(d) if d.failed == 0 => d,
        _ => fail_with("AHPCL-PREC-0004", "this decimal arithmetic overflowed or divided by zero"),
    }
}

/// The whole-number value of a decimal, if it is one.
fn deci_whole(d: AhpclDecimal) -> Option<i128> {
    let p = pow10(d.scale)?;
    (d.mantissa % p == 0).then(|| d.mantissa / p)
}

/// Exact integer exponentiation, rather than a floating-point round trip.
fn deci_pow(a: AhpclDecimal, e: u32) -> Option<AhpclDecimal> {
    let mut out = AhpclDecimal::ok(1, 0);
    for _ in 0..e {
        out = AhpclDecimal::ok(
            out.mantissa.checked_mul(a.mantissa)?,
            out.scale.checked_add(a.scale)?,
        );
    }
    Some(out)
}

pub(crate) fn deci_as_f64(d: AhpclDecimal) -> f64 {
    deci_to_f64(d)
}

pub(crate) fn decimal_from_f64_public(v: f64, digits: u32) -> Option<AhpclDecimal> {
    decimal_from_f64(v, digits).map(|d| AhpclDecimal::ok(d.mantissa, d.scale))
}

/// AHPCL computes square roots to at most this many places, the same cap the
/// interpreter enforces.
pub(crate) const SQRT_MAX_DIGITS: u32 = 18;

fn deci_to_f64(d: AhpclDecimal) -> f64 {
    d.mantissa as f64 / 10f64.powi(d.scale as i32)
}

fn decimal_from_f64(v: f64, digits: u32) -> Option<AhpclDecimal> {
    if !v.is_finite() {
        return None;
    }
    let scaled = v * 10f64.powi(digits as i32);
    (scaled.abs() < i128::MAX as f64).then(|| AhpclDecimal::ok(scaled.round() as i128, digits))
}

/// Square root to `digits` places, by integer Newton's method — the same algorithm the
/// interpreter uses, so the two agree digit for digit rather than approximately.
pub(crate) fn deci_sqrt(d: AhpclDecimal, digits: u32) -> AhpclDecimal {
    let digits = digits.min(SQRT_MAX_DIGITS);
    if d.mantissa < 0 {
        fail_with("AHPCL-RUN-0001", "the square root of a negative number is not a real number");
    }
    // sqrt(m / 10^s) to n places = isqrt(m × 10^(2n − s)) / 10^n
    let shift = 2i64 * digits as i64 - d.scale as i64;
    let scaled = if shift >= 0 {
        match pow10(shift as u32).and_then(|p| d.mantissa.checked_mul(p)) {
            Some(v) => v,
            None => fail_with("AHPCL-PREC-0004", "this square root overflowed"),
        }
    } else {
        match pow10((-shift) as u32) {
            Some(p) => d.mantissa / p,
            None => fail_with("AHPCL-PREC-0004", "this square root overflowed"),
        }
    };
    AhpclDecimal::ok(integer_sqrt(scaled as u128) as i128, digits)
}

fn integer_sqrt(n: u128) -> u128 {
    if n < 2 {
        return n;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

pub(crate) fn rat_apply(op: u32, a: AhpclRational, b: AhpclRational) -> AhpclRational {
    let out = match op {
        array::OP_ADD => rat_add(a, b),
        array::OP_SUB => rat_sub(a, b),
        array::OP_MUL => rat_mul(a, b),
        array::OP_POW => {
            if b.den != 1 || b.num < 0 || b.num > u32::MAX as i128 {
                fail_with("AHPCL-RUN-0001", "a rational power needs a whole, non-negative exponent");
            }
            let mut out = AhpclRational { num: 1, den: 1, failed: 0 };
            for _ in 0..b.num {
                out = rat_mul(out, a);
            }
            out
        }
        array::OP_INTDIV | array::OP_MOD => {
            if b.num == 0 {
                fail_with("AHPCL-RUN-0002", "division by zero");
            }
            let q = rat_div(a, b);
            let whole = q.num.div_euclid(q.den);
            if op == array::OP_INTDIV {
                AhpclRational { num: whole, den: 1, failed: q.failed }
            } else {
                rat_sub(a, rat_mul(AhpclRational { num: whole, den: 1, failed: 0 }, b))
            }
        }
        _ => rat_div(a, b),
    };
    if out.failed != 0 {
        fail_with("AHPCL-PREC-0004", "this rational arithmetic overflowed or divided by zero");
    }
    out
}

pub(crate) fn rat_reduce(num: i128, den: i128) -> AhpclRational {
    AhpclRational::reduced(num, den)
}

/// An exact decimal, laid out for the C ABI.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AhpclDecimal {
    pub mantissa: i128,
    pub scale: u32,
    /// Non-zero when the operation overflowed. Checked by generated code, which then
    /// reports through the Error Handler and stops.
    pub failed: u32,
}

impl AhpclDecimal {
    fn ok(mantissa: i128, scale: u32) -> Self {
        AhpclDecimal { mantissa, scale, failed: 0 }.normalised()
    }

    /// Drop trailing zeros, so `0.30` is held as `0.3`.
    ///
    /// This is not only cosmetic. Multiplication adds scales, so without it a chain of
    /// operations compounds: 15 digits becomes 30, then 60, until the value overflows
    /// into nonsense. The interpreter normalises after every step, so this does too.
    fn normalised(mut self) -> Self {
        while self.scale > 0 && self.mantissa % 10 == 0 {
            self.mantissa /= 10;
            self.scale -= 1;
        }
        if self.mantissa == 0 {
            self.scale = 0;
        }
        self
    }

    fn fail() -> Self {
        AhpclDecimal { mantissa: 0, scale: 0, failed: 1 }
    }
}

fn pow10(n: u32) -> Option<i128> {
    10i128.checked_pow(n)
}

fn align(a: AhpclDecimal, b: AhpclDecimal) -> Option<(i128, i128, u32)> {
    let scale = a.scale.max(b.scale);
    let am = a.mantissa.checked_mul(pow10(scale - a.scale)?)?;
    let bm = b.mantissa.checked_mul(pow10(scale - b.scale)?)?;
    Some((am, bm, scale))
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_deci_add(out: *mut AhpclDecimal, a: *const AhpclDecimal, b: *const AhpclDecimal) {
    *out = checked_deci(raw_add(*a, *b));
}

/// The arithmetic itself, which reports overflow through the `failed` flag. The
/// exported entry points wrap these and stop the program instead, since generated code
/// has no way to read the flag.
fn raw_add(a: AhpclDecimal, b: AhpclDecimal) -> AhpclDecimal {
    match align(a, b).and_then(|(x, y, s)| Some((x.checked_add(y)?, s))) {
        Some((m, s)) => AhpclDecimal::ok(m, s),
        None => AhpclDecimal::fail(),
    }
}

fn raw_sub(a: AhpclDecimal, b: AhpclDecimal) -> AhpclDecimal {
    match align(a, b).and_then(|(x, y, s)| Some((x.checked_sub(y)?, s))) {
        Some((m, s)) => AhpclDecimal::ok(m, s),
        None => AhpclDecimal::fail(),
    }
}

fn raw_mul(a: AhpclDecimal, b: AhpclDecimal) -> AhpclDecimal {
    match a.mantissa.checked_mul(b.mantissa) {
        Some(m) => AhpclDecimal::ok(m, a.scale + b.scale),
        None => AhpclDecimal::fail(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_deci_sub(out: *mut AhpclDecimal, a: *const AhpclDecimal, b: *const AhpclDecimal) {
    *out = checked_deci(raw_sub(*a, *b));
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_deci_mul(out: *mut AhpclDecimal, a: *const AhpclDecimal, b: *const AhpclDecimal) {
    *out = checked_deci(raw_mul(*a, *b));
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_deci_div(
    out: *mut AhpclDecimal,
    a: *const AhpclDecimal,
    b: *const AhpclDecimal,
    digits: u32,
) {
    if (*b).mantissa == 0 {
        fail_with("AHPCL-RUN-0002", "division by zero");
    }
    *out = checked_deci(deci_div(*a, *b, digits));
}

/// A failed decimal means overflow or a zero divisor. Generated code has no way to
/// inspect the flag, so the runtime stops here rather than handing back a silent 0.
fn checked_deci(d: AhpclDecimal) -> AhpclDecimal {
    if d.failed != 0 {
        fail_with("AHPCL-PREC-0004", "this decimal arithmetic overflowed");
    }
    d
}

fn checked_rat(r: AhpclRational) -> AhpclRational {
    if r.failed != 0 {
        fail_with("AHPCL-PREC-0004", "this rational arithmetic overflowed or divided by zero");
    }
    r
}

fn deci_div(a: AhpclDecimal, b: AhpclDecimal, digits: u32) -> AhpclDecimal {
    if b.mantissa == 0 {
        return AhpclDecimal::fail();
    }
    let Some(num) = a.mantissa.checked_mul(match pow10(b.scale) {
        Some(v) => v,
        None => return AhpclDecimal::fail(),
    }) else {
        return AhpclDecimal::fail();
    };
    let Some(den) = b.mantissa.checked_mul(match pow10(a.scale) {
        Some(v) => v,
        None => return AhpclDecimal::fail(),
    }) else {
        return AhpclDecimal::fail();
    };

    let negative = (num < 0) != (den < 0);
    let num_abs = num.unsigned_abs();
    let den_abs = den.unsigned_abs();

    let mut whole = num_abs / den_abs;
    let mut remainder = num_abs % den_abs;
    let mut scale = 0u32;
    while remainder != 0 && scale < digits {
        whole = match whole.checked_mul(10) {
            Some(v) => v,
            None => return AhpclDecimal::fail(),
        };
        remainder = match remainder.checked_mul(10) {
            Some(v) => v,
            None => return AhpclDecimal::fail(),
        };
        whole += remainder / den_abs;
        remainder %= den_abs;
        scale += 1;
    }
    if remainder != 0 && remainder * 2 >= den_abs {
        whole += 1;
    }
    let m = whole as i128;
    AhpclDecimal::ok(if negative { -m } else { m }, scale)
}

// ── text ────────────────────────────────────────────────────────────────────

/// A string: pointer and byte length, always valid UTF-8.
///
/// Literals point into the binary's constant data. Anything built at runtime is
/// allocated and, for v1, never freed — the process exit reclaims it. That is a real
/// limitation, not an oversight: a loop that reads a million lines holds a million
/// lines. Freeing needs ownership tracking, which v1 does not have.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AhpclStr {
    pub ptr: *const u8,
    pub len: u64,
}

impl AhpclStr {
    pub(crate) unsafe fn as_str(&self) -> &str {
        if self.ptr.is_null() {
            return "";
        }
        std::str::from_utf8_unchecked(std::slice::from_raw_parts(self.ptr, self.len as usize))
    }

    pub(crate) fn owned(text: String) -> AhpclStr {
        let boxed = text.into_boxed_str();
        let len = boxed.len() as u64;
        let ptr = Box::into_raw(boxed) as *const u8;
        AhpclStr { ptr, len }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_print_text(s: *const AhpclStr) {
    emit((*s).as_str());
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_str_cmp(a: *const AhpclStr, b: *const AhpclStr) -> i32 {
    match (*a).as_str().cmp((*b).as_str()) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// Read a whole file as text — `read["path"]`, the same thing the interpreter does.
/// A missing or unreadable file stops the program through the Error Handler.
#[no_mangle]
pub unsafe extern "C" fn ahpcl_read_file(out: *mut AhpclStr, path: *const AhpclStr) {
    let path = (*path).as_str();
    match std::fs::read_to_string(path) {
        Ok(text) => *out = AhpclStr::owned(text),
        Err(e) => fail_with("AHPCL-RUN-0001", &format!("{path} could not be read — {e}")),
    }
}

/// The options `parse` accepts, as a bitmask so they cross the ABI as one word.
pub const PARSE_TRIM: u64 = 1;
pub const PARSE_SCIENTIFIC: u64 = 2;
pub const PARSE_HEX: u64 = 4;
pub const PARSE_UNICODE_DIGITS: u64 = 8;
/// Read `n/d` as a fraction. Opt-in, matching the interpreter.
pub const PARSE_FRACTION: u64 = 16;

/// Normalise text under the parse options, or `None` if the options reject it.
unsafe fn parse_prepare(
    text: *const AhpclStr,
    flags: u64,
    group: *const AhpclStr,
    decimal: *const AhpclStr,
) -> Option<String> {
    let mut s = (*text).as_str().to_string();
    if flags & PARSE_TRIM != 0 {
        s = s.trim().to_string();
    }
    if flags & PARSE_UNICODE_DIGITS != 0 {
        // `char::to_digit` only recognises ASCII and a few others; Thai and
        // Arabic-Indic digits need the general numeric value, which is what the
        // interpreter uses.
        s = s.chars().map(unicode_digit).collect();
    }
    if !group.is_null() {
        let g = (*group).as_str();
        if !g.is_empty() {
            s = s.replace(g, "");
        }
    }
    if !decimal.is_null() {
        let d = (*decimal).as_str();
        if !d.is_empty() && d != "." {
            s = s.replace(d, ".");
        }
    }
    if flags & PARSE_HEX != 0 {
        let body = s.trim_start_matches("0x").trim_start_matches("0X");
        return i128::from_str_radix(body, 16).ok().map(|v| v.to_string());
    }
    if flags & PARSE_SCIENTIFIC != 0 {
        if let Some((mantissa, exponent)) = s.split_once(['e', 'E']) {
            let exp: i32 = exponent.parse().ok()?;
            let (m, scale) = split_decimal(mantissa)?;
            let scale = scale as i32 - exp;
            return Some(if scale <= 0 {
                format!("{}", m.checked_mul(10i128.checked_pow((-scale) as u32)?)?)
            } else {
                render_scaled(m, scale as u32)
            });
        }
    }
    Some(s)
}

/// Map any Unicode decimal digit onto its ASCII form, leaving everything else alone.
///
/// `char::to_digit` only understands ASCII, so the decimal blocks are listed. This table
/// is kept identical to `unicode_digit_to_ascii` in the interpreter — a shorter list
/// here would mean `parse["๔๒" unicode-digits]` worked interpreted and failed compiled.
fn unicode_digit(c: char) -> char {
    const BLOCKS: &[u32] = &[
        0x0660, // Arabic-Indic
        0x06F0, // Extended Arabic-Indic
        0x0966, // Devanagari
        0x09E6, // Bengali
        0x0A66, // Gurmukhi
        0x0AE6, // Gujarati
        0x0B66, // Oriya
        0x0BE6, // Tamil
        0x0C66, // Telugu
        0x0CE6, // Kannada
        0x0D66, // Malayalam
        0x0E50, // Thai
        0x0ED0, // Lao
        0x0F20, // Tibetan
        0x1040, // Myanmar
        0x17E0, // Khmer
        0xFF10, // Fullwidth
    ];
    let code = c as u32;
    for &base in BLOCKS {
        if code >= base && code < base + 10 {
            return char::from_digit(code - base, 10).unwrap_or(c);
        }
    }
    c
}

/// Split `12.34` into mantissa 1234 and scale 2.
fn split_decimal(text: &str) -> Option<(i128, u32)> {
    match text.split_once('.') {
        None => Some((text.parse().ok()?, 0)),
        Some((whole, frac)) => {
            let joined = format!("{whole}{frac}");
            let joined = if joined.starts_with('+') { &joined[1..] } else { &joined[..] };
            Some((joined.parse().ok()?, frac.len() as u32))
        }
    }
}

fn render_scaled(mantissa: i128, scale: u32) -> String {
    format_decimal(AhpclDecimal::ok(mantissa, scale))
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_parse_int(
    text: *const AhpclStr,
    flags: u64,
    group: *const AhpclStr,
    decimal: *const AhpclStr,
) -> i128 {
    let prepared = parse_prepare(text, flags, group, decimal);
    match prepared.as_deref().and_then(|s| s.parse::<i128>().ok()) {
        Some(v) => v,
        None => parse_failure(text),
    }
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_parse_deci(
    out: *mut AhpclDecimal,
    text: *const AhpclStr,
    flags: u64,
    group: *const AhpclStr,
    decimal: *const AhpclStr,
) {
    let prepared = parse_prepare(text, flags, group, decimal);
    match prepared.as_deref().and_then(split_decimal) {
        Some((m, scale)) => *out = AhpclDecimal::ok(m, scale),
        None => parse_failure(text),
    }
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_parse_rat(
    out: *mut AhpclRational,
    text: *const AhpclStr,
    flags: u64,
    group: *const AhpclStr,
    decimal: *const AhpclStr,
) {
    let prepared = parse_prepare(text, flags, group, decimal);
    // Decimal text, plus `n/d` when `fraction` was asked for.
    let parsed = prepared.as_deref().and_then(|s| {
        if flags & PARSE_FRACTION != 0 {
            if let Some((n, d)) = s.split_once('/') {
                let n: i128 = n.trim().parse().ok()?;
                let d: i128 = d.trim().parse().ok()?;
                if d == 0 {
                    return None;
                }
                return Some(AhpclRational::reduced(n, d));
            }
        }
        let (m, scale) = split_decimal(s)?;
        Some(AhpclRational::reduced(m, 10i128.checked_pow(scale)?))
    });
    match parsed {
        Some(v) => *out = v,
        None => parse_failure(text),
    }
}

unsafe fn parse_failure(text: *const AhpclStr) -> ! {
    let message = format!("'{}' is not a number", (*text).as_str());
    let code = std::ffi::CString::new("AHPCL-RUN-0004").unwrap();
    let msg = std::ffi::CString::new(message).unwrap();
    ahpcl_fail(code.as_ptr(), msg.as_ptr())
}

// ── exact rationals ─────────────────────────────────────────────────────────

/// An exact rational, always reduced, with a positive denominator.
///
/// Fixed size like a decimal, so it needs no heap — only the same by-pointer ABI.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AhpclRational {
    pub num: i128,
    pub den: i128,
    pub failed: u64,
}

impl AhpclRational {
    fn fail() -> Self {
        AhpclRational { num: 0, den: 1, failed: 1 }
    }

    fn reduced(num: i128, den: i128) -> Self {
        if den == 0 {
            return Self::fail();
        }
        let sign = if (num < 0) != (den < 0) { -1 } else { 1 };
        let (n, d) = (num.unsigned_abs(), den.unsigned_abs());
        let g = gcd(n, d).max(1);
        AhpclRational { num: sign * (n / g) as i128, den: (d / g) as i128, failed: 0 }
    }
}

fn gcd(a: u128, b: u128) -> u128 {
    if b == 0 { a } else { gcd(b, a % b) }
}

macro_rules! try_rat {
    ($e:expr) => {
        match $e {
            Some(v) => v,
            None => return AhpclRational::fail(),
        }
    };
}

fn rat_add(a: AhpclRational, b: AhpclRational) -> AhpclRational {
    AhpclRational::reduced(
        try_rat!(try_rat!(a.num.checked_mul(b.den)).checked_add(try_rat!(b.num.checked_mul(a.den)))),
        try_rat!(a.den.checked_mul(b.den)),
    )
}

fn rat_sub(a: AhpclRational, b: AhpclRational) -> AhpclRational {
    AhpclRational::reduced(
        try_rat!(try_rat!(a.num.checked_mul(b.den)).checked_sub(try_rat!(b.num.checked_mul(a.den)))),
        try_rat!(a.den.checked_mul(b.den)),
    )
}

fn rat_mul(a: AhpclRational, b: AhpclRational) -> AhpclRational {
    AhpclRational::reduced(
        try_rat!(a.num.checked_mul(b.num)),
        try_rat!(a.den.checked_mul(b.den)),
    )
}

fn rat_div(a: AhpclRational, b: AhpclRational) -> AhpclRational {
    if b.num == 0 {
        return AhpclRational::fail();
    }
    AhpclRational::reduced(
        try_rat!(a.num.checked_mul(b.den)),
        try_rat!(a.den.checked_mul(b.num)),
    )
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_rat_add(out: *mut AhpclRational, a: *const AhpclRational, b: *const AhpclRational) {
    *out = checked_rat(rat_add(*a, *b));
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_rat_sub(out: *mut AhpclRational, a: *const AhpclRational, b: *const AhpclRational) {
    *out = checked_rat(rat_sub(*a, *b));
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_rat_mul(out: *mut AhpclRational, a: *const AhpclRational, b: *const AhpclRational) {
    *out = checked_rat(rat_mul(*a, *b));
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_rat_div(out: *mut AhpclRational, a: *const AhpclRational, b: *const AhpclRational) {
    if (*b).num == 0 {
        fail_with("AHPCL-RUN-0002", "division by zero");
    }
    *out = checked_rat(rat_div(*a, *b));
}

/// The general arithmetic entry points, tagged by operation. These cover the cases the
/// dedicated `ahpcl_deci_add`-style functions do not: powers, `//` and `mod`.
#[no_mangle]
pub unsafe extern "C" fn ahpcl_deci_binary(
    out: *mut AhpclDecimal,
    op: u32,
    a: *const AhpclDecimal,
    b: *const AhpclDecimal,
) {
    *out = deci_apply(op, *a, *b);
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_rat_binary(
    out: *mut AhpclRational,
    op: u32,
    a: *const AhpclRational,
    b: *const AhpclRational,
) {
    *out = rat_apply(op, *a, *b);
}

/// `//` between decimals gives a whole number, so it hands back an int rather than a
/// decimal — matching `Decimal::int_div` in the interpreter.
#[no_mangle]
pub unsafe extern "C" fn ahpcl_deci_int_div(a: *const AhpclDecimal, b: *const AhpclDecimal) -> i128 {
    let q = deci_apply(array::OP_INTDIV, *a, *b);
    q.mantissa
}

/// π, e and τ to `digits` places, truncated then rounded — the same table and rounding
/// the interpreter uses, so a compiled program prints the same digits.
#[no_mangle]
pub unsafe extern "C" fn ahpcl_constant(out: *mut AhpclDecimal, which: u32, digits: u32) {
    let text = match which {
        1 => "2.718281828459045235360287471352662497",
        2 => "6.283185307179586476925286766559005768",
        _ => "3.141592653589793238462643383279502884",
    };
    // The table is written with 36 decimal places.
    let full = AhpclDecimal { mantissa: text.replace('.', "").parse().unwrap(), scale: 36, failed: 0 };
    if digits > full.scale {
        // A silent approximation would be worse than refusing: types.md is explicit
        // that asking for more places than are known is an error.
        fail_with(
            "AHPCL-PREC-0004",
            &format!(
                "AHPCL knows this constant to {} decimal places; {digits} were asked for",
                full.scale
            ),
        );
    }
    if digits == full.scale {
        *out = full;
        return;
    }
    let drop = full.scale - digits;
    let divisor = 10i128.pow(drop);
    let kept = full.mantissa / divisor;
    let remainder = (full.mantissa % divisor).abs();
    let rounded = if remainder * 2 >= divisor { kept + kept.signum().max(1) } else { kept };
    *out = AhpclDecimal { mantissa: rounded, scale: digits, failed: 0 };
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_rat_from_int(out: *mut AhpclRational, v: i128) {
    *out = AhpclRational { num: v as i128, den: 1, failed: 0 };
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_rat_cmp(a: *const AhpclRational, b: *const AhpclRational) -> i32 {
    let (a, b) = (*a, *b);
    let (x, y) = (a.num.saturating_mul(b.den), b.num.saturating_mul(a.den));
    match x.cmp(&y) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_print_rat(r: *const AhpclRational) {
    let r = *r;
    if r.den == 1 {
        emit(&r.num.to_string());
    } else {
        emit(&format!("{}/{}", r.num, r.den));
    }
}

/// -1, 0 or 1.
#[no_mangle]
pub unsafe extern "C" fn ahpcl_deci_cmp(a: *const AhpclDecimal, b: *const AhpclDecimal) -> i32 {
    let (a, b) = (*a, *b);
    match align(a, b) {
        Some((x, y, _)) => match x.cmp(&y) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        },
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_deci_from_int(out: *mut AhpclDecimal, v: i128) {
    *out = AhpclDecimal::ok(v as i128, 0);
}

/// Print a decimal, with trailing zeros dropped so `0.30` shows as `0.3`.
#[no_mangle]
pub unsafe extern "C" fn ahpcl_print_deci(d: *const AhpclDecimal) {
    emit(&format_decimal(*d));
}

#[no_mangle]
pub extern "C" fn ahpcl_print_int(v: i128) {
    emit(&v.to_string());
}

/// Booleans print as `true`/`false`, matching the literals and the interpreter.
#[no_mangle]
pub extern "C" fn ahpcl_print_bool(v: i8) {
    emit(if v != 0 { "true" } else { "false" });
}

/// Euclidean division, matching the interpreter.
///
/// LLVM's `sdiv`/`srem` truncate toward zero, so `-7 // 3` would be -2 natively and -3
/// interpreted. Euclidean is the one the language specifies, and a remainder that is
/// never negative is the more useful of the two.
#[no_mangle]
pub extern "C" fn ahpcl_int_div(a: i128, b: i128) -> i128 {
    if b == 0 {
        unsafe { ahpcl_fail(c"AHPCL-RUN-0002".as_ptr(), c"division by zero.".as_ptr()) }
    }
    a.div_euclid(b)
}

#[no_mangle]
pub extern "C" fn ahpcl_int_mod(a: i128, b: i128) -> i128 {
    if b == 0 {
        unsafe { ahpcl_fail(c"AHPCL-RUN-0002".as_ptr(), c"remainder by zero.".as_ptr()) }
    }
    a.rem_euclid(b)
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_print_str(p: *const c_char) {
    if p.is_null() {
        return;
    }
    let bytes = std::ffi::CStr::from_ptr(p).to_bytes();
    emit(&String::from_utf8_lossy(bytes));
}

#[no_mangle]
pub extern "C" fn ahpcl_print_newline() {
    emit("\n");
}

/// Report a runtime failure in the Error Handler's voice, then stop.
///
/// Runtime failures stop the program — chosen deliberately as a starting point, with
/// failures-as-values remaining addable later.
#[no_mangle]
pub unsafe extern "C" fn ahpcl_fail(code: *const c_char, message: *const c_char) -> ! {
    let text = |p: *const c_char| {
        if p.is_null() {
            String::new()
        } else {
            String::from_utf8_lossy(std::ffi::CStr::from_ptr(p).to_bytes()).into_owned()
        }
    };
    eprintln!("AHPCL Error Handler:");
    eprintln!("Something went wrong while running.");
    eprintln!();
    eprintln!("rule conditions: {}", text(message));
    eprintln!("[{}]", text(code));
    eprintln!();
    eprintln!("1 error found.");
    std::process::exit(1);
}

pub fn format_decimal(d: AhpclDecimal) -> String {
    let mut mantissa = d.mantissa;
    let mut scale = d.scale;
    while scale > 0 && mantissa % 10 == 0 {
        mantissa /= 10;
        scale -= 1;
    }
    if scale == 0 {
        return mantissa.to_string();
    }
    let negative = mantissa < 0;
    let digits = mantissa.unsigned_abs().to_string();
    let s = scale as usize;
    let padded = if digits.len() <= s {
        format!("{}{}", "0".repeat(s - digits.len() + 1), digits)
    } else {
        digits
    };
    let split = padded.len() - s;
    format!(
        "{}{}.{}",
        if negative { "-" } else { "" },
        &padded[..split],
        &padded[split..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(m: i128, s: u32) -> AhpclDecimal {
        AhpclDecimal::ok(m, s)
    }

    // The raw forms, which report overflow through the flag. The exported entry points
    // stop the program instead, so they cannot be asserted on from inside a test.
    fn add(a: AhpclDecimal, b: AhpclDecimal) -> AhpclDecimal {
        raw_add(a, b)
    }

    fn mul(a: AhpclDecimal, b: AhpclDecimal) -> AhpclDecimal {
        raw_mul(a, b)
    }

    #[test]
    fn the_headline_case_holds_in_native_code_too() {
        // 0.1 + 0.2 is exactly 0.3, the same as in the interpreter.
        let sum = add(d(1, 1), d(2, 1));
        assert_eq!(sum.failed, 0);
        assert_eq!(format_decimal(sum), "0.3");
        assert_eq!(unsafe { ahpcl_deci_cmp(&sum, &d(3, 1)) }, 0);
    }

    #[test]
    fn division_uses_the_true_digits() {
        assert_eq!(format_decimal(deci_div(d(58, 0), d(3, 0), 15)), "19.333333333333333");
    }

    #[test]
    fn division_by_zero_is_flagged_rather_than_producing_infinity() {
        assert_eq!(deci_div(d(1, 0), d(0, 0), 15).failed, 1);
    }

    #[test]
    fn overflow_is_flagged_rather_than_wrapping() {
        assert_eq!(add(d(i128::MAX, 0), d(1, 0)).failed, 1);
        assert_eq!(mul(d(i128::MAX, 0), d(2, 0)).failed, 1);
    }

    #[test]
    fn integer_division_is_euclidean_in_native_code_too() {
        // LLVM's sdiv truncates: -7/3 would be -2. Euclidean gives -3, and a
        // remainder that is never negative.
        assert_eq!(ahpcl_int_div(-7, 3), -3);
        assert_eq!(ahpcl_int_mod(-7, 3), 2);
        assert_eq!(ahpcl_int_div(-7, -3), 3);
        assert_eq!(ahpcl_int_mod(-7, -3), 2);
        assert_eq!(ahpcl_int_div(7, 3), 2);
        assert_eq!(ahpcl_int_mod(7, 3), 1);
    }

    fn s(text: &'static str) -> AhpclStr {
        AhpclStr { ptr: text.as_ptr(), len: text.len() as u64 }
    }

    #[test]
    fn native_parsing_honours_its_options() {
        unsafe {
            let comma = s(",");
            assert_eq!(
                ahpcl_parse_int(&s("1,234,567"), 0, &comma, std::ptr::null()),
                1_234_567
            );
            assert_eq!(ahpcl_parse_int(&s("  42  "), PARSE_TRIM, std::ptr::null(), std::ptr::null()), 42);
            assert_eq!(ahpcl_parse_int(&s("0xff"), PARSE_HEX, std::ptr::null(), std::ptr::null()), 255);
        }
    }

    #[test]
    fn native_string_comparison_matches_the_interpreter() {
        unsafe {
            assert_eq!(ahpcl_str_cmp(&s("abc"), &s("abc")), 0);
            assert_eq!(ahpcl_str_cmp(&s("abc"), &s("abd")), -1);
            assert_eq!(ahpcl_str_cmp(&s("😂"), &s("a")), 1);
        }
    }

    #[test]
    fn rationals_stay_exact_in_native_code() {
        let third = AhpclRational::reduced(1, 3);
        let sum = rat_add(rat_add(third, third), third);
        assert_eq!(sum, AhpclRational { num: 1, den: 1, failed: 0 });
        assert_eq!(rat_mul(third, third), AhpclRational::reduced(1, 9));
    }

    #[test]
    fn rationals_reduce_and_refuse_zero_denominators() {
        assert_eq!(AhpclRational::reduced(2, 4), AhpclRational::reduced(1, 2));
        assert_eq!(AhpclRational::reduced(1, 0).failed, 1);
        assert_eq!(rat_div(AhpclRational::reduced(1, 2), AhpclRational::reduced(0, 1)).failed, 1);
    }

    #[test]
    fn printing_drops_trailing_zeros() {
        assert_eq!(format_decimal(d(300, 3)), "0.3");
        assert_eq!(format_decimal(d(-1500, 3)), "-1.5");
        assert_eq!(format_decimal(d(42, 0)), "42");
    }
}
