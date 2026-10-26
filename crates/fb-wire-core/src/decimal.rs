//! Exact NUMERIC / DECIMAL values.
//!
//! Firebird stores a scaled integer: SMALLINT, INTEGER or BIGINT up to 18
//! digits, INT128 up to 38 (Firebird 4). Upstream turned these into `f64`,
//! which cannot hold most decimal fractions; this keeps the integer and the
//! scale as the server sent them.

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// `mantissa / 10^scale`, exactly
pub struct Decimal {
    mantissa: i128,
    scale: u8,
}

/// Firebird's widest NUMERIC is 38 digits
pub const MAX_SCALE: u8 = 38;

impl Decimal {
    /// `mantissa / 10^scale`. Panics if `scale` is over 38.
    pub fn new(mantissa: i128, scale: u8) -> Self {
        assert!(scale <= MAX_SCALE, "decimal scale {scale} is over {MAX_SCALE}");
        Self { mantissa, scale }
    }

    /// From a value in Firebird's terms: `data * 10^sqlscale`, `sqlscale <= 0`
    pub fn from_scaled(data: i128, sqlscale: i16) -> Option<Self> {
        let scale = u8::try_from(sqlscale.checked_neg()?).ok()?;
        (scale <= MAX_SCALE).then_some(Self { mantissa: data, scale })
    }

    pub fn mantissa(&self) -> i128 {
        self.mantissa
    }

    /// Digits after the decimal point
    pub fn scale(&self) -> u8 {
        self.scale
    }

    /// The value, if it is a whole number that fits
    pub fn to_i64(&self) -> Option<i64> {
        let unit = 10i128.pow(self.scale as u32);
        if self.mantissa % unit != 0 {
            return None;
        }
        i64::try_from(self.mantissa / unit).ok()
    }

    /// The nearest `f64`. Lossy: only for callers that ask for a float.
    pub fn to_f64(&self) -> f64 {
        // Parsing the exact text rounds once; dividing would round twice.
        self.to_string().parse().unwrap_or(f64::NAN)
    }
}

impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let digits = self.mantissa.unsigned_abs().to_string();
        let sign = if self.mantissa < 0 { "-" } else { "" };
        let scale = self.scale as usize;
        if scale == 0 {
            return write!(f, "{sign}{digits}");
        }
        let digits = format!("{digits:0>width$}", width = scale + 1);
        let (int, frac) = digits.split_at(digits.len() - scale);
        write!(f, "{sign}{int}.{frac}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseDecimalError(String);

impl fmt::Display for ParseDecimalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid decimal {:?}", self.0)
    }
}

impl std::error::Error for ParseDecimalError {}

impl FromStr for Decimal {
    type Err = ParseDecimalError;

    /// `[-+]digits[.digits]`, as Firebird prints a NUMERIC
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseDecimalError(s.to_string());
        let (negative, body) = match s.as_bytes().first() {
            Some(b'-') => (true, &s[1..]),
            Some(b'+') => (false, &s[1..]),
            _ => (false, s),
        };
        let (int, frac) = body.split_once('.').unwrap_or((body, ""));
        if int.is_empty() && frac.is_empty()
            || !int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit())
        {
            return Err(err());
        }
        let scale = u8::try_from(frac.len()).ok().filter(|&s| s <= MAX_SCALE).ok_or_else(err)?;
        let mut mantissa: i128 = 0;
        for b in int.bytes().chain(frac.bytes()) {
            mantissa = mantissa
                .checked_mul(10)
                .and_then(|m| m.checked_add((b - b'0') as i128))
                .ok_or_else(err)?;
        }
        Ok(Self { mantissa: if negative { -mantissa } else { mantissa }, scale })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prints_exactly() {
        assert_eq!(Decimal::new(12345, 2).to_string(), "123.45");
        assert_eq!(Decimal::new(-5, 3).to_string(), "-0.005");
        assert_eq!(Decimal::new(0, 2).to_string(), "0.00");
        assert_eq!(Decimal::new(7, 0).to_string(), "7");
        // 0.1 + 0.2 in NUMERIC(18,2) is 0.30, not 0.30000000000000004
        assert_eq!(Decimal::new(10 + 20, 2).to_string(), "0.30");
        assert_eq!(Decimal::new(i128::MIN, 38).mantissa(), i128::MIN);
        assert_eq!(
            Decimal::new(i128::MIN, 0).to_string(),
            "-170141183460469231731687303715884105728"
        );
    }

    #[test]
    fn parses_what_it_prints() {
        for s in ["123.45", "-0.005", "0.00", "7", "-99999999999999999999999999999999999.999"] {
            assert_eq!(s.parse::<Decimal>().unwrap().to_string(), s);
        }
        assert_eq!("+.5".parse::<Decimal>().unwrap(), Decimal::new(5, 1));
        for bad in ["", "-", ".", "1e5", "1.2.3", "12a", "1 "] {
            assert!(bad.parse::<Decimal>().is_err(), "{bad}");
        }
    }

    #[test]
    fn converts() {
        assert_eq!(Decimal::from_scaled(12345, -2), Some(Decimal::new(12345, 2)));
        assert_eq!(Decimal::from_scaled(1, 2), None);
        assert_eq!(Decimal::new(500, 2).to_i64(), Some(5));
        assert_eq!(Decimal::new(501, 2).to_i64(), None);
        assert_eq!(Decimal::new(12345, 2).to_f64(), 123.45);
    }
}
