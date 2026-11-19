//! What a column of a result adds up to, for the grid's summary row: how
//! many values it has, how many different ones, its least and greatest, and
//! for numbers their sum and average. Worked out here, over every row, from
//! the rows as the grid holds them: none of them crosses the bridge.

use std::cmp::Ordering;
use std::collections::HashSet;

use fb_core::ColumnKind;
use serde::Serialize;

use crate::results::{compare, Rows};

/// A column's summary. What does not apply to its kind is `None`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnSummary {
    /// Values that are not null
    pub count: usize,
    /// Different values among them; not for blobs, whose cells are markers
    pub distinct: Option<usize>,
    /// As the grid sorts the column: numbers by value, dates in order, text
    /// ignoring case
    pub min: Option<String>,
    pub max: Option<String>,
    /// Numbers only, exact for integers and NUMERIC/DECIMAL
    pub sum: Option<String>,
    pub avg: Option<String>,
    /// The sum is past what can be added exactly (beyond 38 digits)
    pub overflow: bool,
}

/// Digits an exact average has past the column's own
const AVG_DIGITS: u32 = 4;

/// The summary of column `col`
pub fn summarize(rows: &Rows, col: usize) -> ColumnSummary {
    let kind = rows.kinds()[col];
    let blob = matches!(kind, ColumnKind::Blob | ColumnKind::TextBlob);
    let mut count = 0;
    let mut distinct = HashSet::new();
    let (mut min, mut max): (Option<&str>, Option<&str>) = (None, None);
    let mut exact = matches!(kind, ColumnKind::Integer | ColumnKind::Decimal).then(Exact::default);
    let mut float = (kind == ColumnKind::Float).then_some(0f64);
    for value in rows.column(col).flatten() {
        count += 1;
        if blob {
            continue;
        }
        distinct.insert(value);
        if min.is_none_or(|m| compare(kind, Some(value), Some(m)) == Ordering::Less) {
            min = Some(value);
        }
        if max.is_none_or(|m| compare(kind, Some(value), Some(m)) == Ordering::Greater) {
            max = Some(value);
        }
        if let Some(sum) = &mut exact {
            sum.add(value);
        }
        if let Some(sum) = &mut float {
            *sum += value.parse::<f64>().unwrap_or(f64::NAN);
        }
    }
    let (sum, avg, overflow) = match (exact, float) {
        (Some(e), _) if e.overflow || e.unreadable => (None, None, e.overflow),
        (Some(e), _) => (Some(e.text()), (count > 0).then(|| e.average(count)).flatten(), false),
        (_, Some(f)) => (Some(f.to_string()), (count > 0).then(|| (f / count as f64).to_string()), false),
        _ => (None, None, false),
    };
    ColumnSummary {
        count,
        distinct: (!blob).then_some(distinct.len()),
        min: min.map(str::to_owned),
        max: max.map(str::to_owned),
        sum,
        avg,
        overflow,
    }
}

/// An exact sum: `mantissa` / 10^`scale`, the scale the widest value had
#[derive(Default)]
struct Exact {
    mantissa: i128,
    scale: u32,
    overflow: bool,
    /// A value was not a plain number (`display` writes them plain)
    unreadable: bool,
}

impl Exact {
    fn add(&mut self, text: &str) {
        if self.overflow || self.unreadable {
            return;
        }
        let Some((value, scale)) = parse(text) else {
            self.unreadable = true;
            return;
        };
        let to = self.scale.max(scale);
        let sum = rescale(self.mantissa, self.scale, to)
            .zip(rescale(value, scale, to))
            .and_then(|(a, b)| a.checked_add(b));
        match sum {
            Some(sum) => {
                self.mantissa = sum;
                self.scale = to;
            }
            None => self.overflow = true,
        }
    }

    fn text(&self) -> String {
        format_scaled(self.mantissa, self.scale)
    }

    /// The sum over `count`, rounded half away from zero to `AVG_DIGITS`
    /// more digits, without the zeros that end it past the column's own
    fn average(&self, count: usize) -> Option<String> {
        let scale = self.scale + AVG_DIGITS;
        let numerator = rescale(self.mantissa, self.scale, scale)?;
        let count = count as i128;
        let (quotient, rest) = (numerator / count, numerator % count);
        let rounded = if rest.abs() * 2 >= count { quotient + numerator.signum() } else { quotient };
        let mut text = format_scaled(rounded, scale);
        if text.contains('.') {
            let keep = text.find('.').unwrap() + 1 + self.scale as usize;
            while text.len() > keep && text.ends_with('0') {
                text.pop();
            }
            if text.ends_with('.') {
                text.pop();
            }
        }
        Some(text)
    }
}

/// `-12.50` → (-1250, 2)
fn parse(text: &str) -> Option<(i128, u32)> {
    let (negative, digits) = text.strip_prefix('-').map_or((false, text), |d| (true, d));
    let (int, frac) = digits.split_once('.').unwrap_or((digits, ""));
    if int.is_empty() && frac.is_empty() || !int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut mantissa: i128 = 0;
    for b in int.bytes().chain(frac.bytes()) {
        mantissa = mantissa.checked_mul(10)?.checked_add(i128::from(b - b'0'))?;
    }
    Some((if negative { -mantissa } else { mantissa }, frac.len() as u32))
}

fn rescale(mantissa: i128, from: u32, to: u32) -> Option<i128> {
    10i128.checked_pow(to - from).and_then(|f| mantissa.checked_mul(f))
}

fn format_scaled(mantissa: i128, scale: u32) -> String {
    let digits = mantissa.unsigned_abs().to_string();
    let sign = if mantissa < 0 { "-" } else { "" };
    if scale == 0 {
        return format!("{sign}{digits}");
    }
    let scale = scale as usize;
    let padded = format!("{digits:0>width$}", width = scale + 1);
    let (int, frac) = padded.split_at(padded.len() - scale);
    format!("{sign}{int}.{frac}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use fb_core::{Decimal, Value};

    fn rows(kind: ColumnKind, values: Vec<Value>) -> Rows {
        let mut rows = Rows::new(vec![kind]);
        for v in values {
            rows.push(vec![v]);
        }
        rows
    }

    fn decimal(text: &str) -> Value {
        Value::Decimal(text.parse::<Decimal>().unwrap())
    }

    #[test]
    fn integers_add_up_exactly() {
        let s = summarize(&rows(ColumnKind::Integer, vec![Value::Integer(7), Value::Null, Value::Integer(23), Value::Integer(7)]), 0);
        assert_eq!(
            s,
            ColumnSummary {
                count: 3,
                distinct: Some(2),
                min: Some("7".into()),
                max: Some("23".into()),
                sum: Some("37".into()),
                avg: Some("12.3333".into()),
                overflow: false,
            }
        );
    }

    #[test]
    fn decimals_keep_their_scale_and_negatives_their_sign() {
        let s = summarize(&rows(ColumnKind::Decimal, vec![decimal("10.25"), decimal("-0.50"), decimal("2")]), 0);
        assert_eq!(s.sum.as_deref(), Some("11.75"));
        // 11.75 / 3 = 3.9166666…: four digits past the column's two, rounded
        assert_eq!(s.avg.as_deref(), Some("3.916667"));
        assert_eq!((s.min.as_deref(), s.max.as_deref()), (Some("-0.50"), Some("10.25")));
        let half = summarize(&rows(ColumnKind::Decimal, vec![decimal("1.00"), decimal("2.00")]), 0);
        // Zeros past the column's own scale go
        assert_eq!(half.avg.as_deref(), Some("1.50"));
        let negative = summarize(&rows(ColumnKind::Decimal, vec![decimal("-1"), decimal("-2")]), 0);
        assert_eq!((negative.sum.as_deref(), negative.avg.as_deref()), (Some("-3"), Some("-1.5")));
    }

    #[test]
    fn a_sum_past_38_digits_says_so() {
        let big = decimal("99999999999999999999999999999999999999");
        let s = summarize(&rows(ColumnKind::Decimal, vec![big.clone(), big]), 0);
        assert_eq!((s.sum, s.avg, s.overflow), (None, None, true));
        assert_eq!(s.count, 2);
    }

    #[test]
    fn text_has_no_sum_but_its_least_and_greatest() {
        let text = |s: &str| Value::Text(s.into());
        let s = summarize(&rows(ColumnKind::Text, vec![text("NAO"), text("sim"), text("NAO"), Value::Null]), 0);
        assert_eq!((s.count, s.distinct), (3, Some(2)));
        assert_eq!((s.min.as_deref(), s.max.as_deref()), (Some("NAO"), Some("sim")));
        assert_eq!((s.sum, s.avg), (None, None));
    }

    #[test]
    fn floats_add_up_as_floats() {
        let s = summarize(&rows(ColumnKind::Float, vec![Value::Float(0.5), Value::Float(1.0)]), 0);
        assert_eq!((s.sum.as_deref(), s.avg.as_deref()), (Some("1.5"), Some("0.75")));
    }

    #[test]
    fn blobs_are_counted_only() {
        let blob = Value::BlobRef { id: 1, text: true };
        let s = summarize(&rows(ColumnKind::TextBlob, vec![blob, Value::Null]), 0);
        assert_eq!((s.count, s.distinct, s.min), (1, None, None));
    }

    #[test]
    fn an_empty_column_has_nothing_to_average() {
        let s = summarize(&rows(ColumnKind::Integer, vec![Value::Null]), 0);
        assert_eq!((s.count, s.sum.as_deref(), s.avg), (0, Some("0"), None));
    }
}
