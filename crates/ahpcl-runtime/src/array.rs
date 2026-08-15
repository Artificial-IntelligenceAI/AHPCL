//! Arrays for compiled code.
//!
//! An array is an opaque heap object. Generated code holds only a pointer and never
//! computes an element offset itself, so the compiler never has to agree with this
//! file about element sizes or alignment — the class of mismatch that made decimals
//! silently do nothing before they moved to a by-pointer ABI.
//!
//! Like text, arrays built at runtime are not freed during the run. See `AhpclStr`.

use crate::{fail_with, format_decimal, AhpclDecimal, AhpclRational, AhpclStr};

/// Which type the elements are. Kept in step with `Native` in the backend.
pub const KIND_INT: u32 = 0;
pub const KIND_BOOL: u32 = 1;
pub const KIND_DECI: u32 = 2;
pub const KIND_RAT: u32 = 3;
pub const KIND_STR: u32 = 4;
/// A `num` element: the cell keeps whichever exact kind was stored in it.
pub const KIND_NUM: u32 = 5;

#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Int(i128),
    Bool(bool),
    Deci(AhpclDecimal),
    Rat(AhpclRational),
    Str(String),
}

impl Cell {
    fn render(&self) -> String {
        match self {
            Cell::Int(v) => v.to_string(),
            Cell::Bool(b) => if *b { "true" } else { "false" }.to_string(),
            Cell::Deci(d) => format_decimal(*d),
            Cell::Rat(r) => {
                if r.den == 1 {
                    r.num.to_string()
                } else {
                    format!("{}/{}", r.num, r.den)
                }
            }
            Cell::Str(s) => s.clone(),
        }
    }
}

/// Values in row-major order, with the shape they are read through.
#[derive(Debug, Clone)]
pub struct Array {
    pub items: Vec<Cell>,
    pub shape: Vec<u64>,
    pub kind: u32,
}

impl Array {
    fn vector(items: Vec<Cell>, kind: u32) -> Array {
        let n = items.len() as u64;
        Array { items, shape: vec![n], kind }
    }

    fn hand_out(self) -> *mut Array {
        Box::into_raw(Box::new(self))
    }
}

fn zero(kind: u32) -> Cell {
    match kind {
        KIND_BOOL => Cell::Bool(false),
        KIND_DECI => Cell::Deci(AhpclDecimal { mantissa: 0, scale: 0, failed: 0 }),
        KIND_RAT => Cell::Rat(AhpclRational { num: 0, den: 1, failed: 0 }),
        KIND_STR => Cell::Str(String::new()),
        _ => Cell::Int(0),
    }
}

// ── construction and element access ─────────────────────────────────────────

#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_new(kind: u32, rank: u32, dims: *const u64) -> *mut Array {
    let shape: Vec<u64> = if dims.is_null() || rank == 0 {
        vec![0]
    } else {
        std::slice::from_raw_parts(dims, rank as usize).to_vec()
    };
    let total: u64 = shape.iter().product();
    Array { items: vec![zero(kind); total as usize], shape, kind }.hand_out()
}

/// An array with no elements yet, for a loop that collects its handbacks. The shape is
/// filled in as elements arrive, since the count is not known before the loop runs.
#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_empty(kind: u32) -> *mut Array {
    Array { items: Vec::new(), shape: vec![0], kind }.hand_out()
}

unsafe fn push(a: *mut Array, c: Cell) {
    let a = &mut *a;
    a.items.push(c);
    a.shape = vec![a.items.len() as u64];
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_push_int(a: *mut Array, v: i128) {
    push(a, Cell::Int(v));
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_push_bool(a: *mut Array, v: i8) {
    push(a, Cell::Bool(v != 0));
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_push_deci(a: *mut Array, v: *const AhpclDecimal) {
    push(a, Cell::Deci(*v));
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_push_rat(a: *mut Array, v: *const AhpclRational) {
    push(a, Cell::Rat(*v));
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_push_str(a: *mut Array, v: *const AhpclStr) {
    let text = (*v).as_str().to_string();
    push(a, Cell::Str(text));
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_push_num(a: *mut Array, v: *const Cell) {
    let cell = (*v).clone();
    push(a, cell);
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_len(a: *const Array) -> i128 {
    (*a).items.len() as i128
}

/// Which element type an array ended up holding, so a caller knows how to read it.
#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_kind(a: *const Array) -> u32 {
    (*a).kind
}

/// The shape, itself as an array, so `:shape;` has something to hand back.
#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_shape(a: *const Array) -> *mut Array {
    let items = (&*a).shape.iter().map(|d| Cell::Int(*d as i128)).collect();
    Array::vector(items, KIND_INT).hand_out()
}

unsafe fn bounds(a: &Array, index: i128) -> usize {
    let len = a.items.len();
    if index < 1 || index as usize > len {
        fail_with(
            "AHPCL-RUN-0003",
            &format!("index {index} is out of range for an array of length {len}"),
        );
    }
    index as usize - 1
}

macro_rules! accessors {
    ($set:ident, $get:ident, $variant:ident, $ct:ty, $to_cell:expr, $from_cell:expr) => {
        #[no_mangle]
        pub unsafe extern "C" fn $set(a: *mut Array, index: i128, v: $ct) {
            let a = &mut *a;
            let i = bounds(a, index);
            a.items[i] = $to_cell(v);
        }

        #[no_mangle]
        pub unsafe extern "C" fn $get(a: *const Array, index: i128) -> $ct {
            let a = &*a;
            let i = bounds(a, index);
            $from_cell(&a.items[i])
        }
    };
}

accessors!(
    ahpcl_array_set_int,
    ahpcl_array_get_int,
    Int,
    i128,
    |v: i128| Cell::Int(v),
    |c: &Cell| match c {
        Cell::Int(v) => *v,
        Cell::Bool(b) => *b as i128,
        _ => 0,
    }
);

accessors!(
    ahpcl_array_set_bool,
    ahpcl_array_get_bool,
    Bool,
    i8,
    |v: i8| Cell::Bool(v != 0),
    |c: &Cell| match c {
        Cell::Bool(b) => *b as i8,
        Cell::Int(v) => (*v != 0) as i8,
        _ => 0,
    }
);

#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_set_deci(a: *mut Array, index: i128, v: *const AhpclDecimal) {
    let a = &mut *a;
    let i = bounds(a, index);
    a.items[i] = Cell::Deci(*v);
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_get_deci(out: *mut AhpclDecimal, a: *const Array, index: i128) {
    let a = &*a;
    let i = bounds(a, index);
    *out = match &a.items[i] {
        Cell::Deci(d) => *d,
        Cell::Int(v) => AhpclDecimal { mantissa: *v, scale: 0, failed: 0 },
        _ => AhpclDecimal { mantissa: 0, scale: 0, failed: 1 },
    };
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_set_rat(a: *mut Array, index: i128, v: *const AhpclRational) {
    let a = &mut *a;
    let i = bounds(a, index);
    a.items[i] = Cell::Rat(*v);
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_get_rat(out: *mut AhpclRational, a: *const Array, index: i128) {
    let a = &*a;
    let i = bounds(a, index);
    *out = match &a.items[i] {
        Cell::Rat(r) => *r,
        Cell::Int(v) => AhpclRational { num: *v, den: 1, failed: 0 },
        _ => AhpclRational { num: 0, den: 1, failed: 1 },
    };
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_set_str(a: *mut Array, index: i128, v: *const AhpclStr) {
    let text = (*v).as_str().to_string();
    let a = &mut *a;
    let i = bounds(a, index);
    a.items[i] = Cell::Str(text);
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_get_str(out: *mut AhpclStr, a: *const Array, index: i128) {
    let a = &*a;
    let i = bounds(a, index);
    let text = a.items[i].render();
    *out = AhpclStr::owned(text);
}

/// A `num` element is stored and read as the tagged cell it already is.
#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_set_num(a: *mut Array, index: i128, v: *const Cell) {
    let cell = (*v).clone();
    let a = &mut *a;
    let i = bounds(a, index);
    a.items[i] = cell;
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_get_num(a: *const Array, index: i128) -> *mut Cell {
    let a = &*a;
    let i = bounds(a, index);
    hand_out_cell(a.items[i].clone())
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_print_array(a: *const Array) {
    let rendered: Vec<String> = (&*a).items.iter().map(Cell::render).collect();
    crate::emit(&format!("{{{}}}", rendered.join(", ")));
}

// ── selection ───────────────────────────────────────────────────────────────

/// Pick out elements by 1-based index, handing back an array of the same element type.
#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_select(
    a: *const Array,
    indices: *const i128,
    count: u64,
) -> *mut Array {
    let a = &*a;
    let picks = std::slice::from_raw_parts(indices, count as usize);
    let items: Vec<Cell> = picks.iter().map(|&i| a.items[bounds(a, i)].clone()).collect();
    Array::vector(items, a.kind).hand_out()
}

/// `from to to by step`, all 1-based and inclusive.
#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_range(
    a: *const Array,
    from: i128,
    to: i128,
    step: i128,
) -> *mut Array {
    if step == 0 {
        fail_with("AHPCL-RUN-0001", "a selector step of 0 would never advance");
    }
    let a = &*a;
    let mut items = Vec::new();
    let mut i = from;
    while (step > 0 && i <= to) || (step < 0 && i >= to) {
        items.push(a.items[bounds(a, i)].clone());
        i += step;
    }
    Array::vector(items, a.kind).hand_out()
}

/// How generated code describes a run of selectors: parallel arrays, one entry per
/// selector, rather than an array of a shared struct.
///
/// A struct would be tidier to read and is exactly what broke: LLVM aligns `i128` to 8
/// inside a struct where Rust's `repr(C)` aligns it to 16, so every field after the
/// first two sat at a different offset in the compiler's view than in this one. The
/// selector that ignores its fields (`:all;`) kept working, which made it look like a
/// selector bug rather than a layout one. Parallel arrays of a single primitive have no
/// layout left to disagree about.
///
/// `kinds[i]` is 0 for `:all;`, 1 for an index list, 2 for a range.
/// `bounds` holds three values per selector — from, to, by — used only by ranges.
/// `indices[i]` and `counts[i]` describe an index list.

/// The 0-based positions a selector picks along a dimension of `extent`, and whether
/// the dimension collapses (a single index gives a plain value, not a 1-long slice).
unsafe fn positions(
    kind: u32,
    from: i128,
    to: i128,
    by: i128,
    indices: *const i128,
    count: u64,
    extent: u64,
    dim: usize,
) -> (Vec<usize>, bool) {
    let check = |i: i128| -> usize {
        if i < 1 || i as u128 > extent as u128 {
            fail_with(
                "AHPCL-RUN-0003",
                &format!(
                    "index {i} is out of range for dimension {} of length {extent}",
                    dim + 1
                ),
            );
        }
        i as usize - 1
    };
    match kind {
        1 => {
            let list = std::slice::from_raw_parts(indices, count as usize);
            (list.iter().map(|&i| check(i)).collect(), count == 1)
        }
        2 => {
            if by == 0 {
                fail_with("AHPCL-RUN-0001", "a selector step of 0 would never advance");
            }
            let mut out = Vec::new();
            let mut i = from;
            while (by > 0 && i <= to) || (by < 0 && i >= to) {
                out.push(check(i));
                i += by;
            }
            (out, false)
        }
        _ => ((0..extent as usize).collect(), false),
    }
}

/// Apply a run of selectors, one per dimension.
///
/// When every selected dimension collapses the result is a single value; it is handed
/// back as an array with an empty shape, which `ahpcl_array_is_scalar` reports.
#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_select_run(
    a: *const Array,
    kinds: *const u32,
    bounds: *const i128,
    indices: *const *const i128,
    counts: *const u64,
    n: i128,
) -> *mut Array {
    let a = &*a;
    let n = n as usize;
    if n == 0 {
        return a.clone().hand_out();
    }
    if n > a.shape.len() {
        fail_with(
            "AHPCL-RUN-0003",
            &format!(
                "{n} selectors were given, but this array has {} dimension{}",
                a.shape.len(),
                if a.shape.len() == 1 { "" } else { "s" }
            ),
        );
    }

    let kinds = std::slice::from_raw_parts(kinds, n);
    let bounds = std::slice::from_raw_parts(bounds, n * 3);
    let indices = std::slice::from_raw_parts(indices, n);
    let counts = std::slice::from_raw_parts(counts, n);
    let mut picks: Vec<Vec<usize>> = Vec::new();
    let mut collapse: Vec<bool> = Vec::new();
    for dim in 0..n {
        let (chosen, single) = positions(
            kinds[dim],
            bounds[dim * 3],
            bounds[dim * 3 + 1],
            bounds[dim * 3 + 2],
            indices[dim],
            counts[dim],
            a.shape[dim],
            dim,
        );
        picks.push(chosen);
        collapse.push(single);
    }
    // Dimensions with no selector are kept whole.
    for dim in n..a.shape.len() {
        picks.push((0..a.shape[dim] as usize).collect());
        collapse.push(false);
    }

    let strides: Vec<usize> = (0..a.shape.len())
        .map(|d| a.shape[d + 1..].iter().product::<u64>().max(1) as usize)
        .collect();

    let mut items = Vec::new();
    let mut counter = vec![0usize; picks.len()];
    if picks.iter().any(Vec::is_empty) {
        return Array { items, shape: vec![0], kind: a.kind }.hand_out();
    }
    loop {
        let offset: usize = counter
            .iter()
            .enumerate()
            .map(|(d, &c)| picks[d][c] * strides[d])
            .sum();
        items.push(a.items[offset].clone());

        let mut d = picks.len();
        loop {
            if d == 0 {
                let shape: Vec<u64> = picks
                    .iter()
                    .zip(&collapse)
                    .filter(|(_, c)| !**c)
                    .map(|(p, _)| p.len() as u64)
                    .collect();
                return Array { items, shape, kind: a.kind }.hand_out();
            }
            d -= 1;
            counter[d] += 1;
            if counter[d] < picks[d].len() {
                break;
            }
            counter[d] = 0;
        }
    }
}

/// The 1-based flat position addressed by a run of single indices, one per dimension.
///
/// Writing to `('m'):2;:1;` needs the same row-major arithmetic reading uses; without it
/// the second selector would index the flat buffer and hit the wrong cell.
#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_offset(
    a: *const Array,
    indices: *const i128,
    n: i128,
) -> i128 {
    let a = &*a;
    let n = n as usize;
    if n > a.shape.len() {
        fail_with(
            "AHPCL-RUN-0003",
            &format!(
                "{n} selectors were given, but this array has {} dimension{}",
                a.shape.len(),
                if a.shape.len() == 1 { "" } else { "s" }
            ),
        );
    }
    let picks = std::slice::from_raw_parts(indices, n);
    let strides: Vec<usize> = (0..a.shape.len())
        .map(|d| a.shape[d + 1..].iter().product::<u64>().max(1) as usize)
        .collect();

    let mut offset = 0usize;
    for (dim, &i) in picks.iter().enumerate() {
        let extent = a.shape[dim];
        if i < 1 || i as u128 > extent as u128 {
            fail_with(
                "AHPCL-RUN-0003",
                &format!(
                    "index {i} is out of range for dimension {} of length {extent}",
                    dim + 1
                ),
            );
        }
        offset += (i as usize - 1) * strides[dim];
    }
    offset as i128 + 1
}

/// Whether a selection collapsed to a single value rather than an array.
#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_is_scalar(a: *const Array) -> i32 {
    (*a).shape.is_empty() as i32
}

// ── arithmetic ──────────────────────────────────────────────────────────────

pub const OP_ADD: u32 = 0;
pub const OP_SUB: u32 = 1;
pub const OP_MUL: u32 = 2;
pub const OP_DIV: u32 = 3;

/// Element arithmetic, promoting the narrower side: int → deci → rat.
fn arith(op: u32, a: &Cell, b: &Cell) -> Cell {
    use Cell::*;
    let rank = |c: &Cell| match c {
        Rat(_) => 2,
        Deci(_) => 1,
        _ => 0,
    };
    match rank(a).max(rank(b)) {
        2 => {
            let (x, y) = (to_rat(a), to_rat(b));
            Rat(crate::rat_apply(op, x, y))
        }
        1 => {
            let (x, y) = (to_deci(a), to_deci(b));
            Deci(crate::deci_apply(op, x, y))
        }
        _ => {
            let (Int(x), Int(y)) = (to_int(a), to_int(b)) else { unreachable!() };
            let r = match op {
                OP_ADD => x.checked_add(y),
                OP_SUB => x.checked_sub(y),
                OP_MUL => x.checked_mul(y),
                OP_POW => {
                    if y < 0 || y > u32::MAX as i128 {
                        fail_with("AHPCL-PREC-0004", "this exponent is out of range");
                    }
                    x.checked_pow(y as u32)
                }
                OP_INTDIV | OP_MOD => {
                    if y == 0 {
                        fail_with("AHPCL-RUN-0002", "division by zero");
                    }
                    // Euclidean, matching the interpreter and `ahpcl_int_div`.
                    Some(if op == OP_INTDIV { x.div_euclid(y) } else { x.rem_euclid(y) })
                }
                _ => {
                    // Integer division that does not divide exactly becomes a decimal,
                    // matching how the interpreter keeps the true value.
                    let (dx, dy) = (to_deci(a), to_deci(b));
                    return Deci(crate::deci_apply(OP_DIV, dx, dy));
                }
            };
            match r {
                Some(v) => Int(v),
                None => fail_with("AHPCL-PREC-0004", "this array arithmetic overflowed"),
            }
        }
    }
}

fn to_int(c: &Cell) -> Cell {
    match c {
        Cell::Bool(b) => Cell::Int(*b as i128),
        Cell::Int(v) => Cell::Int(*v),
        _ => Cell::Int(0),
    }
}

fn to_deci(c: &Cell) -> AhpclDecimal {
    match c {
        Cell::Deci(d) => *d,
        Cell::Int(v) => AhpclDecimal { mantissa: *v, scale: 0, failed: 0 },
        Cell::Bool(b) => AhpclDecimal { mantissa: *b as i128, scale: 0, failed: 0 },
        _ => AhpclDecimal { mantissa: 0, scale: 0, failed: 1 },
    }
}

fn to_rat(c: &Cell) -> AhpclRational {
    match c {
        Cell::Rat(r) => *r,
        Cell::Int(v) => AhpclRational { num: *v, den: 1, failed: 0 },
        Cell::Bool(b) => AhpclRational { num: *b as i128, den: 1, failed: 0 },
        // A decimal is a rational: mantissa over a power of ten.
        Cell::Deci(d) => match 10i128.checked_pow(d.scale) {
            Some(den) => crate::rat_reduce(d.mantissa, den),
            None => AhpclRational { num: 0, den: 1, failed: 1 },
        },
        _ => AhpclRational { num: 0, den: 1, failed: 1 },
    }
}

fn shapes_agree(x: &Array, y: &Array) -> bool {
    x.items.len() == y.items.len()
}

/// Elementwise, with a one-element side broadcast across the other.
#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_elementwise(
    op: u32,
    a: *const Array,
    b: *const Array,
) -> *mut Array {
    let (x, y) = (&*a, &*b);
    let items: Vec<Cell> = if x.items.len() == 1 {
        y.items.iter().map(|q| arith(op, &x.items[0], q)).collect()
    } else if y.items.len() == 1 {
        x.items.iter().map(|p| arith(op, p, &y.items[0])).collect()
    } else if shapes_agree(x, y) {
        x.items.iter().zip(&y.items).map(|(p, q)| arith(op, p, q)).collect()
    } else {
        fail_with(
            "AHPCL-RUN-0002",
            &format!(
                "elementwise operations need matching shapes, but these hold {} and {} elements",
                x.items.len(),
                y.items.len()
            ),
        )
    };
    let shape = if x.items.len() == 1 { y.shape.clone() } else { x.shape.clone() };
    let kind = result_kind(&items);
    Array { items, shape, kind }.hand_out()
}

fn result_kind(items: &[Cell]) -> u32 {
    items
        .iter()
        .map(|c| match c {
            Cell::Rat(_) => KIND_RAT,
            Cell::Deci(_) => KIND_DECI,
            Cell::Str(_) => KIND_STR,
            Cell::Bool(_) => KIND_BOOL,
            Cell::Int(_) => KIND_INT,
        })
        .max()
        .unwrap_or(KIND_INT)
}

/// Compare elementwise, handing back an array of bools. A one-element side broadcasts,
/// so `('a'):all; > 2` compares every element against the same number.
///
/// Ordering codes match `OP_CMP_*`.
pub const OP_CMP_EQ: u32 = 10;
pub const OP_CMP_NE: u32 = 11;
pub const OP_CMP_LT: u32 = 12;
pub const OP_CMP_GT: u32 = 13;
pub const OP_CMP_LE: u32 = 14;
pub const OP_CMP_GE: u32 = 15;

fn cell_order(a: &Cell, b: &Cell) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    if let (Cell::Str(x), Cell::Str(y)) = (a, b) {
        return x.cmp(y);
    }
    // Compare as rationals, which every numeric cell converts to exactly.
    let (x, y) = (to_rat(a), to_rat(b));
    let (l, r) = (
        x.num.saturating_mul(y.den),
        y.num.saturating_mul(x.den),
    );
    l.cmp(&r).then(Ordering::Equal)
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_compare(
    op: u32,
    a: *const Array,
    b: *const Array,
) -> *mut Array {
    use std::cmp::Ordering;
    let (x, y) = (&*a, &*b);
    let decide = |p: &Cell, q: &Cell| {
        let o = cell_order(p, q);
        let yes = match op {
            OP_CMP_EQ => o == Ordering::Equal,
            OP_CMP_NE => o != Ordering::Equal,
            OP_CMP_LT => o == Ordering::Less,
            OP_CMP_GT => o == Ordering::Greater,
            OP_CMP_LE => o != Ordering::Greater,
            _ => o != Ordering::Less,
        };
        Cell::Bool(yes)
    };
    let items: Vec<Cell> = if x.items.len() == 1 {
        y.items.iter().map(|q| decide(&x.items[0], q)).collect()
    } else if y.items.len() == 1 {
        x.items.iter().map(|p| decide(p, &y.items[0])).collect()
    } else if shapes_agree(x, y) {
        x.items.iter().zip(&y.items).map(|(p, q)| decide(p, q)).collect()
    } else {
        fail_with(
            "AHPCL-RUN-0001",
            &format!(
                "comparing elementwise needs matching shapes, but these hold {} and {} elements",
                x.items.len(),
                y.items.len()
            ),
        )
    };
    let shape = if x.items.len() == 1 { y.shape.clone() } else { x.shape.clone() };
    Array { items, shape, kind: KIND_BOOL }.hand_out()
}

/// `⊙` — elementwise multiplication, which requires shapes to match exactly.
#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_hadamard(a: *const Array, b: *const Array) -> *mut Array {
    if !shapes_agree(&*a, &*b) {
        fail_with(
            "AHPCL-RUN-0002",
            &format!(
                "elementwise operations need matching shapes, but these are {:?} and {:?}",
                (*a).shape,
                (*b).shape
            ),
        );
    }
    ahpcl_array_elementwise(OP_MUL, a, b)
}

/// `·` — dot product for two vectors, matrix multiplication otherwise.
///
/// The vector case hands back a single value, so it writes through the out-array and
/// reports which kind landed there.
#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_dot(a: *const Array, b: *const Array) -> *mut Array {
    let (x, y) = (&*a, &*b);
    if x.shape.len() == 1 && y.shape.len() == 1 {
        if !shapes_agree(x, y) {
            fail_with(
                "AHPCL-RUN-0002",
                "a dot product needs two vectors of the same length",
            );
        }
        let mut total = Cell::Int(0);
        for (p, q) in x.items.iter().zip(&y.items) {
            total = arith(OP_ADD, &total, &arith(OP_MUL, p, q));
        }
        let kind = result_kind(std::slice::from_ref(&total));
        return Array { items: vec![total], shape: vec![1], kind }.hand_out();
    }
    matmul(x, y)
}

fn matmul(x: &Array, y: &Array) -> *mut Array {
    let (m, k) = (x.shape[0] as usize, *x.shape.get(1).unwrap_or(&1) as usize);
    let (k2, n) = (y.shape[0] as usize, *y.shape.get(1).unwrap_or(&1) as usize);
    if k != k2 {
        fail_with(
            "AHPCL-RUN-0002",
            &format!("matrix multiplication requires inner dimensions to agree: {k} ≠ {k2}"),
        );
    }
    let mut items = Vec::with_capacity(m * n);
    for row in 0..m {
        for col in 0..n {
            let mut total = Cell::Int(0);
            for i in 0..k {
                let p = &x.items[row * k + i];
                let q = &y.items[i * n + col];
                total = arith(OP_ADD, &total, &arith(OP_MUL, p, q));
            }
            items.push(total);
        }
    }
    let kind = result_kind(&items);
    Array { items, shape: vec![m as u64, n as u64], kind }.hand_out()
}

/// `×` — cross product, defined only for two 3-element vectors.
#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_cross(a: *const Array, b: *const Array) -> *mut Array {
    let (u, v) = (&(&*a).items, &(&*b).items);
    if u.len() != 3 || v.len() != 3 {
        fail_with(
            "AHPCL-RUN-0002",
            "cross product is defined for two 3-element vectors",
        );
    }
    let mut out = Vec::new();
    for (i, j) in [(1, 2), (2, 0), (0, 1)] {
        let l = arith(OP_MUL, &u[i], &v[j]);
        let r = arith(OP_MUL, &u[j], &v[i]);
        out.push(arith(OP_SUB, &l, &r));
    }
    let kind = result_kind(&out);
    Array::vector(out, kind).hand_out()
}

/// `⊗` — tensor product: every pairing, with the shapes concatenated.
#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_tensor(a: *const Array, b: *const Array) -> *mut Array {
    let (x, y) = (&*a, &*b);
    let mut items = Vec::with_capacity(x.items.len() * y.items.len());
    for p in &x.items {
        for q in &y.items {
            items.push(arith(OP_MUL, p, q));
        }
    }
    let shape: Vec<u64> = x.shape.iter().chain(&y.shape).copied().collect();
    let kind = result_kind(&items);
    Array { items, shape, kind }.hand_out()
}

/// Sum every element, for Rule A: a bare array reference in arithmetic reduces to the
/// total of its elements rather than staying an array.
#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_sum(a: *const Array) -> *mut Cell {
    let a = &*a;
    let mut total = Cell::Int(0);
    for item in &a.items {
        total = arith(OP_ADD, &total, item);
    }
    hand_out_cell(total)
}

// ── num: the polymorphic top of the numeric hierarchy ───────────────────────
//
// A `num` holds whichever exact kind flowed into it, so it is a tagged value rather
// than a fixed layout — the same `Cell` an array element is. Operations promote the
// narrower side exactly as the interpreter does, so the two agree digit for digit.

pub const OP_POW: u32 = 4;
pub const OP_INTDIV: u32 = 5;
pub const OP_MOD: u32 = 6;

fn hand_out_cell(c: Cell) -> *mut Cell {
    Box::into_raw(Box::new(c))
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_num_from_int(v: i128) -> *mut Cell {
    hand_out_cell(Cell::Int(v))
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_num_from_deci(v: *const AhpclDecimal) -> *mut Cell {
    hand_out_cell(Cell::Deci(*v))
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_num_from_rat(v: *const AhpclRational) -> *mut Cell {
    hand_out_cell(Cell::Rat(*v))
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_num_from_bool(v: i8) -> *mut Cell {
    hand_out_cell(Cell::Bool(v != 0))
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_num_binary(op: u32, a: *const Cell, b: *const Cell) -> *mut Cell {
    hand_out_cell(arith(op, &*a, &*b))
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_num_to_deci(out: *mut AhpclDecimal, a: *const Cell) {
    *out = to_deci(&*a);
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_num_to_rat(out: *mut AhpclRational, a: *const Cell) {
    *out = to_rat(&*a);
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_num_to_int(a: *const Cell) -> i128 {
    match &*a {
        Cell::Int(v) => *v,
        Cell::Bool(b) => *b as i128,
        Cell::Deci(d) => match 10i128.checked_pow(d.scale) {
            Some(p) => d.mantissa / p,
            None => 0,
        },
        Cell::Rat(r) => r.num / r.den,
        Cell::Str(_) => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_num_cmp(a: *const Cell, b: *const Cell) -> i32 {
    let diff = arith(OP_SUB, &*a, &*b);
    let sign = match diff {
        Cell::Int(v) => v.signum(),
        Cell::Deci(d) => d.mantissa.signum(),
        Cell::Rat(r) => r.num.signum(),
        Cell::Bool(b) => b as i128,
        Cell::Str(_) => 0,
    };
    sign as i32
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_print_num(a: *const Cell) {
    crate::emit(&(*a).render());
}

/// The unary operators, on whichever kind the value holds.
pub const UN_NEG: u32 = 0;
pub const UN_ABS: u32 = 1;
pub const UN_SQRT: u32 = 2;
pub const UN_FLOOR: u32 = 3;
pub const UN_CEIL: u32 = 4;
pub const UN_SIN: u32 = 5;
pub const UN_COS: u32 = 6;
pub const UN_TAN: u32 = 7;
pub const UN_LOG: u32 = 8;
pub const UN_LN: u32 = 9;

pub(crate) fn unary(op: u32, c: &Cell, digits: u32) -> Cell {
    match op {
        UN_NEG => match c {
            Cell::Int(v) => Cell::Int(-v),
            Cell::Deci(d) => Cell::Deci(AhpclDecimal { mantissa: -d.mantissa, ..*d }),
            Cell::Rat(r) => Cell::Rat(AhpclRational { num: -r.num, ..*r }),
            other => other.clone(),
        },
        UN_ABS => match c {
            Cell::Int(v) => Cell::Int(v.abs()),
            Cell::Deci(d) => Cell::Deci(AhpclDecimal { mantissa: d.mantissa.abs(), ..*d }),
            Cell::Rat(r) => Cell::Rat(AhpclRational { num: r.num.abs(), ..*r }),
            other => other.clone(),
        },
        UN_SQRT => {
            let d = to_deci(c);
            Cell::Deci(crate::deci_sqrt(d, digits))
        }
        // Transcendental functions have no exact decimal answer, so these go through
        // f64 — the same route the interpreter takes, and for the same reason.
        UN_SIN | UN_COS | UN_TAN | UN_LOG | UN_LN => {
            let f = crate::deci_as_f64(to_deci(c));
            let out = match op {
                UN_SIN => f.sin(),
                UN_COS => f.cos(),
                UN_TAN => f.tan(),
                UN_LOG => f.log10(),
                _ => f.ln(),
            };
            match crate::decimal_from_f64_public(out, digits) {
                Some(d) => Cell::Deci(d),
                None => fail_with("AHPCL-RUN-0001", "this result is not a finite number"),
            }
        }
        UN_FLOOR | UN_CEIL => {
            let d = to_deci(c);
            let p = match 10i128.checked_pow(d.scale) {
                Some(p) => p,
                None => fail_with("AHPCL-PREC-0004", "this value is too large to round"),
            };
            let mut whole = d.mantissa / p;
            let remainder = d.mantissa % p;
            if op == UN_FLOOR && remainder < 0 {
                whole -= 1;
            }
            if op == UN_CEIL && remainder > 0 {
                whole += 1;
            }
            Cell::Int(whole)
        }
        _ => c.clone(),
    }
}

/// Apply a unary operator to every element, for `-('a'):all;` and friends.
#[no_mangle]
pub unsafe extern "C" fn ahpcl_array_unary(op: u32, a: *const Array, digits: u32) -> *mut Array {
    let a = &*a;
    let items: Vec<Cell> = a.items.iter().map(|c| unary(op, c, digits)).collect();
    let kind = result_kind(&items);
    Array { items, shape: a.shape.clone(), kind }.hand_out()
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_num_unary(op: u32, a: *const Cell, digits: u32) -> *mut Cell {
    hand_out_cell(unary(op, &*a, digits))
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_deci_unary(
    out: *mut AhpclDecimal,
    op: u32,
    a: *const AhpclDecimal,
    digits: u32,
) {
    *out = to_deci(&unary(op, &Cell::Deci(*a), digits));
}

#[no_mangle]
pub unsafe extern "C" fn ahpcl_rat_unary(
    out: *mut AhpclRational,
    op: u32,
    a: *const AhpclRational,
    digits: u32,
) {
    *out = to_rat(&unary(op, &Cell::Rat(*a), digits));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ints(vs: &[i128]) -> Array {
        Array::vector(vs.iter().map(|v| Cell::Int(*v)).collect(), KIND_INT)
    }

    #[test]
    fn the_dot_product_matches_the_interpreter() {
        unsafe {
            let a = ints(&[1, 2, 3]);
            let b = ints(&[4, 5, 6]);
            let out = &*ahpcl_array_dot(&a, &b);
            assert_eq!(out.items, vec![Cell::Int(32)]);
        }
    }

    #[test]
    fn the_cross_product_is_perpendicular_to_both() {
        unsafe {
            let x = ints(&[1, 0, 0]);
            let y = ints(&[0, 1, 0]);
            let out = &*ahpcl_array_cross(&x, &y);
            assert_eq!(out.items, vec![Cell::Int(0), Cell::Int(0), Cell::Int(1)]);
        }
    }

    #[test]
    fn the_tensor_product_concatenates_shapes() {
        unsafe {
            let a = ints(&[1, 2]);
            let b = ints(&[3, 4, 5]);
            let out = &*ahpcl_array_tensor(&a, &b);
            assert_eq!(out.shape, vec![2, 3]);
            assert_eq!(out.items.len(), 6);
            assert_eq!(out.items[0], Cell::Int(3));
            assert_eq!(out.items[5], Cell::Int(10));
        }
    }

    #[test]
    fn matrix_multiplication_uses_the_inner_dimension() {
        unsafe {
            let a = Array { items: ints(&[1, 2, 3, 4]).items, shape: vec![2, 2], kind: KIND_INT };
            let b = Array { items: ints(&[5, 6, 7, 8]).items, shape: vec![2, 2], kind: KIND_INT };
            let out = &*ahpcl_array_dot(&a, &b);
            assert_eq!(
                out.items,
                vec![Cell::Int(19), Cell::Int(22), Cell::Int(43), Cell::Int(50)]
            );
        }
    }

    #[test]
    fn a_single_value_broadcasts_across_the_other_side() {
        unsafe {
            let a = ints(&[1, 2, 3]);
            let b = ints(&[10]);
            let out = &*ahpcl_array_elementwise(OP_MUL, &a, &b);
            assert_eq!(out.items, vec![Cell::Int(10), Cell::Int(20), Cell::Int(30)]);
        }
    }

    #[test]
    fn a_collected_array_grows_and_keeps_its_shape_in_step() {
        unsafe {
            let a = ahpcl_array_empty(KIND_INT);
            for v in [12, 30, 16] {
                ahpcl_array_push_int(a, v);
            }
            assert_eq!((*a).items, vec![Cell::Int(12), Cell::Int(30), Cell::Int(16)]);
            assert_eq!((*a).shape, vec![3]);
        }
    }

    #[test]
    fn a_bare_array_reference_sums_its_elements() {
        // Rule A, the same reduction the interpreter performs.
        unsafe {
            let a = ints(&[2, 4, 4, 4, 5, 9]);
            assert_eq!(*ahpcl_array_sum(&a), Cell::Int(28));
        }
    }

    /// A run of single-index selectors, in the parallel-array form the compiler builds.
    unsafe fn select_indices(a: &Array, ix: &[i128]) -> *mut Array {
        let kinds: Vec<u32> = ix.iter().map(|_| 1).collect();
        let bounds = vec![0i128; ix.len() * 3];
        let buffers: Vec<*const i128> =
            ix.iter().map(|&i| Box::leak(Box::new([i])).as_ptr()).collect();
        let counts = vec![1u64; ix.len()];
        ahpcl_array_select_run(
            a,
            kinds.as_ptr(),
            bounds.as_ptr(),
            buffers.as_ptr(),
            counts.as_ptr(),
            ix.len() as i128,
        )
    }

    #[test]
    fn a_selector_addresses_a_dimension_not_the_flat_buffer() {
        // `('m'):2;` on a 3x4 matrix is the second *row*, not the second element.
        unsafe {
            let m = Array {
                items: ints(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]).items,
                shape: vec![3, 4],
                kind: KIND_INT,
            };
            let row = &*select_indices(&m, &[2]);
            assert_eq!(row.shape, vec![4]);
            assert_eq!(
                row.items,
                vec![Cell::Int(5), Cell::Int(6), Cell::Int(7), Cell::Int(8)]
            );

            // Two selectors address two dimensions, and both collapse to one value.
            let one = &*select_indices(&m, &[2, 3]);
            assert!(one.shape.is_empty(), "collapsed to a scalar");
            assert_eq!(one.items, vec![Cell::Int(7)]);
        }
    }

    #[test]
    fn selectors_are_one_based() {
        unsafe {
            let a = ints(&[10, 20, 30, 40, 50]);
            let picked = &*ahpcl_array_select(&a, [1i128, 3].as_ptr(), 2);
            assert_eq!(picked.items, vec![Cell::Int(10), Cell::Int(30)]);
            let ranged = &*ahpcl_array_range(&a, 1, 5, 2);
            assert_eq!(ranged.items, vec![Cell::Int(10), Cell::Int(30), Cell::Int(50)]);
        }
    }

    #[test]
    fn division_that_does_not_divide_exactly_keeps_the_true_value() {
        unsafe {
            let a = ints(&[1]);
            let b = ints(&[3]);
            let out = &*ahpcl_array_elementwise(OP_DIV, &a, &b);
            assert!(matches!(out.items[0], Cell::Deci(_)), "{:?}", out.items[0]);
        }
    }
}
