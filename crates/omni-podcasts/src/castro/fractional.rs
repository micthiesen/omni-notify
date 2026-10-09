//! rocicorp `fractional-indexing` 4.0.0 `generateKeyBetween` with the default
//! alphabets (base-62 digits, A-Z/a-z integer heads), which Castro queue
//! positions use. Golden vectors from node live in
//! `tests/golden/fractional_indexing.json`.

const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const INT_DIGITS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const ZERO: u8 = b'0';

/// The library's thrown message (`invalid order key: a00`, `a0 >= a0`, ...).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct FractionalIndexError(pub String);

type Result<T> = std::result::Result<T, FractionalIndexError>;

fn err<T>(message: impl Into<String>) -> Result<T> {
    Err(FractionalIndexError(message.into()))
}

/// `Uint8Array` digit lookup: unknown characters read as 0, as in JS.
fn index_in(alphabet: &[u8], c: u8) -> usize {
    alphabet.iter().position(|d| *d == c).unwrap_or(0)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn midpoint(a: &[u8], b: Option<&[u8]>) -> Result<Vec<u8>> {
    if let Some(b) = b
        && a >= b
    {
        return err(format!("{} >= {}", text(a), text(b)));
    }
    if a.last() == Some(&ZERO) || b.is_some_and(|b| b.last() == Some(&ZERO)) {
        return err("trailing zero");
    }
    if let Some(b) = b.filter(|b| !b.is_empty()) {
        let mut n = 0;
        while Some(a.get(n).copied().unwrap_or(ZERO)) == b.get(n).copied() {
            n += 1;
        }
        if n > 0 {
            let mut out = b[..n].to_vec();
            out.extend(midpoint(a.get(n..).unwrap_or(&[]), Some(&b[n..]))?);
            return Ok(out);
        }
    }
    let digit_a = a.first().map_or(0, |c| index_in(DIGITS, *c));
    let digit_b = match b {
        Some(b) => match b.first() {
            Some(c) => index_in(DIGITS, *c),
            // `b === ""` cannot reach here: `a >= ""` already failed above.
            None => return err(format!("{} >= ", text(a))),
        },
        None => DIGITS.len(),
    };
    if digit_b > digit_a + 1 {
        // Math.round(0.5 * (a + b)) on non-negative integers.
        let mid = (digit_a + digit_b).div_ceil(2);
        return Ok(vec![DIGITS[mid]]);
    }
    if let Some(b) = b
        && b.len() > 1
    {
        return Ok(b[..1].to_vec());
    }
    let mut out = vec![DIGITS[digit_a]];
    out.extend(midpoint(a.get(1..).unwrap_or(&[]), None)?);
    Ok(out)
}

fn integer_length(head: Option<u8>) -> Result<usize> {
    let Some(head) = head else {
        return err("invalid order key head: ");
    };
    let i = index_in(INT_DIGITS, head);
    if INT_DIGITS[i] != head {
        return err(format!("invalid order key head: {}", char::from(head)));
    }
    let half = INT_DIGITS.len() / 2;
    Ok(if i < half { half - i + 1 } else { i - half + 2 })
}

fn validate_integer(int: &[u8]) -> Result<()> {
    if int.len() != integer_length(int.first().copied())? {
        return err(format!("invalid integer part of order key: {}", text(int)));
    }
    Ok(())
}

fn integer_part(key: &[u8]) -> Result<&[u8]> {
    let length = integer_length(key.first().copied())?;
    if length > key.len() {
        return err(format!("invalid order key: {}", text(key)));
    }
    Ok(&key[..length])
}

fn is_smallest_integer(key: &[u8]) -> bool {
    key.len() == INT_DIGITS.len() / 2 + 1
        && key[0] == INT_DIGITS[0]
        && key[1..].iter().all(|c| *c == ZERO)
}

fn validate_order_key(key: &[u8]) -> Result<()> {
    if is_smallest_integer(key) {
        return err(format!("invalid order key: {}", text(key)));
    }
    let int = integer_part(key)?;
    if key[int.len()..].last() == Some(&ZERO) {
        return err(format!("invalid order key: {}", text(key)));
    }
    Ok(())
}

fn resize(head: u8, old_head: u8, mut trailing: Vec<u8>, fill: u8) -> Result<Vec<u8>> {
    let new_len = integer_length(Some(head))?;
    let old_len = integer_length(Some(old_head))?;
    let mut out = vec![head];
    if new_len > old_len {
        trailing.push(fill);
    } else if new_len < old_len && !trailing.is_empty() {
        trailing.remove(0);
    }
    out.extend(trailing);
    Ok(out)
}

fn increment_integer(x: &[u8]) -> Result<Option<Vec<u8>>> {
    validate_integer(x)?;
    let head = x[0];
    let mut trailing = Vec::new();
    for i in (1..x.len()).rev() {
        let d = index_in(DIGITS, x[i]) + 1;
        if d == DIGITS.len() {
            trailing.insert(0, ZERO);
        } else {
            let mut out = vec![head];
            out.extend_from_slice(&x[1..i]);
            out.push(DIGITS[d]);
            out.extend(trailing);
            return Ok(Some(out));
        }
    }
    let head_index = index_in(INT_DIGITS, head);
    if head_index == INT_DIGITS.len() - 1 {
        return Ok(None);
    }
    resize(INT_DIGITS[head_index + 1], head, trailing, ZERO).map(Some)
}

fn decrement_integer(x: &[u8]) -> Result<Option<Vec<u8>>> {
    validate_integer(x)?;
    let head = x[0];
    let last = DIGITS[DIGITS.len() - 1];
    let mut trailing = Vec::new();
    for i in (1..x.len()).rev() {
        let d = index_in(DIGITS, x[i]);
        if d == 0 {
            trailing.insert(0, last);
        } else {
            let mut out = vec![head];
            out.extend_from_slice(&x[1..i]);
            out.push(DIGITS[d - 1]);
            out.extend(trailing);
            return Ok(Some(out));
        }
    }
    let head_index = index_in(INT_DIGITS, head);
    if head_index == 0 {
        return Ok(None);
    }
    resize(INT_DIGITS[head_index - 1], head, trailing, last).map(Some)
}

/// A key sorting strictly between `a` and `b` (`None` = open end). Keys are
/// ASCII; anything else is rejected.
pub fn generate_key_between(a: Option<&str>, b: Option<&str>) -> Result<String> {
    for key in [a, b].into_iter().flatten() {
        if !key.is_ascii() {
            return err(format!("invalid order key: {key}"));
        }
        validate_order_key(key.as_bytes())?;
    }
    let (a, b) = match (a.map(str::as_bytes), b.map(str::as_bytes)) {
        (Some(a), Some(b)) if a > b => (Some(b), Some(a)),
        other => other,
    };
    let key = match (a, b) {
        (None, None) => vec![INT_DIGITS[INT_DIGITS.len() / 2], ZERO],
        (None, Some(b)) => {
            let ib = integer_part(b)?;
            let fb = &b[ib.len()..];
            if is_smallest_integer(ib) {
                let mut out = ib.to_vec();
                out.extend(midpoint(&[], Some(fb))?);
                out
            } else if ib < b {
                ib.to_vec()
            } else {
                match decrement_integer(ib)? {
                    Some(key) => key,
                    None => return err("cannot decrement any more"),
                }
            }
        }
        (Some(a), None) => {
            let ia = integer_part(a)?;
            let fa = &a[ia.len()..];
            match increment_integer(ia)? {
                Some(key) => key,
                None => {
                    let mut out = ia.to_vec();
                    out.extend(midpoint(fa, None)?);
                    out
                }
            }
        }
        (Some(a), Some(b)) => {
            let ia = integer_part(a)?;
            let fa = &a[ia.len()..];
            let ib = integer_part(b)?;
            let fb = &b[ib.len()..];
            if ia == ib {
                let mut out = ia.to_vec();
                out.extend(midpoint(fa, Some(fb))?);
                out
            } else {
                let Some(i) = increment_integer(ia)? else {
                    return err("cannot increment any more");
                };
                if i.as_slice() < b {
                    i
                } else {
                    let mut out = ia.to_vec();
                    out.extend(midpoint(fa, None)?);
                    out
                }
            }
        }
    };
    Ok(text(&key))
}
