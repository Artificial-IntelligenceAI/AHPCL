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
fn emit(text: &str) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
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
        AhpclDecimal { mantissa, scale, failed: 0 }
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
    let (a, b) = (*a, *b);
    *out = match align(a, b).and_then(|(x, y, s)| Some((x.checked_add(y)?, s))) {
        Some((m, s)) => AhpclDecimal::ok(m, s),
        None => AhpclDecimal::fail(),
    };
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_deci_sub(out: *mut AhpclDecimal, a: *const AhpclDecimal, b: *const AhpclDecimal) {
    let (a, b) = (*a, *b);
    *out = match align(a, b).and_then(|(x, y, s)| Some((x.checked_sub(y)?, s))) {
        Some((m, s)) => AhpclDecimal::ok(m, s),
        None => AhpclDecimal::fail(),
    };
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_deci_mul(out: *mut AhpclDecimal, a: *const AhpclDecimal, b: *const AhpclDecimal) {
    let (a, b) = (*a, *b);
    *out = match a.mantissa.checked_mul(b.mantissa) {
        Some(m) => AhpclDecimal::ok(m, a.scale + b.scale),
        None => AhpclDecimal::fail(),
    };
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_deci_div(
    out: *mut AhpclDecimal,
    a: *const AhpclDecimal,
    b: *const AhpclDecimal,
    digits: u32,
) {
    *out = deci_div(*a, *b, digits);
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
pub unsafe extern "C" fn ahpcl_deci_from_int(out: *mut AhpclDecimal, v: i64) {
    *out = AhpclDecimal::ok(v as i128, 0);
}

/// Print a decimal, with trailing zeros dropped so `0.30` shows as `0.3`.
#[no_mangle]
pub unsafe extern "C" fn ahpcl_print_deci(d: *const AhpclDecimal) {
    emit(&format_decimal(*d));
}

#[no_mangle]
pub extern "C" fn ahpcl_print_int(v: i64) {
    emit(&v.to_string());
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

    fn add(a: AhpclDecimal, b: AhpclDecimal) -> AhpclDecimal {
        let mut out = d(0, 0);
        unsafe { ahpcl_deci_add(&mut out, &a, &b) };
        out
    }

    fn mul(a: AhpclDecimal, b: AhpclDecimal) -> AhpclDecimal {
        let mut out = d(0, 0);
        unsafe { ahpcl_deci_mul(&mut out, &a, &b) };
        out
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
    fn printing_drops_trailing_zeros() {
        assert_eq!(format_decimal(d(300, 3)), "0.3");
        assert_eq!(format_decimal(d(-1500, 3)), "-1.5");
        assert_eq!(format_decimal(d(42, 0)), "42");
    }
}
