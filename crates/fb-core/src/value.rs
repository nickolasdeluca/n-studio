use chrono::NaiveDateTime;
use rsfbclient_core::{Decimal, SqlType};
use serde::{Serialize, Serializer};

/// A column or parameter value
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Value {
    Null,
    Integer(i64),
    /// NUMERIC / DECIMAL / INT128, exact. Serialized as its text, never a
    /// JSON number, which a JavaScript reader would turn into a float.
    Decimal(#[serde(serialize_with = "as_text")] Decimal),
    Float(f64),
    /// Text, and the types the server formats as text: DECFLOAT and the
    /// time-zone types
    Text(String),
    Binary(Vec<u8>),
    Boolean(bool),
    /// DATE and TIME arrive as timestamps
    Timestamp(NaiveDateTime),
    /// A blob not read yet, as an editor's cursor returns them: read it with
    /// `Connection::read_blob` while its transaction lasts
    BlobRef {
        id: u64,
        text: bool,
    },
}

fn as_text<S: Serializer>(d: &Decimal, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(d)
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// The text of a `Text` value
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }

    /// An integer, including a whole-number decimal that fits
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Integer(i) => Some(*i),
            Value::Decimal(d) => d.to_i64(),
            _ => None,
        }
    }
}

impl From<SqlType> for Value {
    fn from(v: SqlType) -> Self {
        match v {
            SqlType::Null => Value::Null,
            SqlType::Integer(i) => Value::Integer(i),
            SqlType::Decimal(d) => Value::Decimal(d),
            SqlType::Floating(f) => Value::Float(f),
            SqlType::Text(s) => Value::Text(s),
            SqlType::Binary(b) => Value::Binary(b),
            SqlType::Boolean(b) => Value::Boolean(b),
            SqlType::Timestamp(t) => Value::Timestamp(t),
        }
    }
}

/// A value as a parameter. A blob reference is not one: it stands for data
/// the server has not sent.
impl TryFrom<Value> for SqlType {
    type Error = crate::Error;

    fn try_from(v: Value) -> Result<Self, Self::Error> {
        Ok(match v {
            Value::Null => SqlType::Null,
            Value::Integer(i) => SqlType::Integer(i),
            Value::Decimal(d) => SqlType::Decimal(d),
            Value::Float(f) => SqlType::Floating(f),
            Value::Text(s) => SqlType::Text(s),
            Value::Binary(b) => SqlType::Binary(b),
            Value::Boolean(b) => SqlType::Boolean(b),
            Value::Timestamp(t) => SqlType::Timestamp(t),
            Value::BlobRef { .. } => {
                return Err(crate::Error::Client { message: "a blob reference cannot be a parameter".into() })
            }
        })
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Text(s.to_string())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Text(s)
    }
}

impl From<i64> for Value {
    fn from(i: i64) -> Self {
        Value::Integer(i)
    }
}

impl From<Decimal> for Value {
    fn from(d: Decimal) -> Self {
        Value::Decimal(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_tagged_with_decimals_as_text() {
        let json = |v: Value| serde_json::to_string(&v).unwrap();
        assert_eq!(json(Value::Decimal(Decimal::new(-12345, 2))), r#"{"type":"decimal","value":"-123.45"}"#);
        assert_eq!(json(Value::Null), r#"{"type":"null"}"#);
        assert_eq!(json(Value::Integer(7)), r#"{"type":"integer","value":7}"#);
    }
}
