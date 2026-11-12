//! A column's, a domain's or a parameter's data type, from the numbers
//! `rdb$fields` keeps it as.

use serde::Serialize;

use crate::row::Row;

/// `rdb$field_type` codes
mod code {
    pub const SMALLINT: i64 = 7;
    pub const INTEGER: i64 = 8;
    pub const QUAD: i64 = 9;
    pub const FLOAT: i64 = 10;
    pub const D_FLOAT: i64 = 11;
    pub const DATE: i64 = 12;
    pub const TIME: i64 = 13;
    pub const CHAR: i64 = 14;
    pub const BIGINT: i64 = 16;
    pub const BOOLEAN: i64 = 23;
    pub const DECFLOAT16: i64 = 24;
    pub const DECFLOAT34: i64 = 25;
    pub const INT128: i64 = 26;
    pub const DOUBLE: i64 = 27;
    pub const TIME_TZ: i64 = 28;
    pub const TIMESTAMP_TZ: i64 = 29;
    pub const TIMESTAMP: i64 = 35;
    pub const VARCHAR: i64 = 37;
    pub const CSTRING: i64 = 40;
    pub const BLOB: i64 = 261;
}

/// The type columns of `rdb$fields f`, joined as [`type_joins`] has it, in
/// the order [`DataType::read`] expects
pub(crate) const TYPE_COLUMNS: &str = "f.rdb$field_type, f.rdb$field_sub_type, f.rdb$field_length, \
     f.rdb$field_precision, f.rdb$field_scale, f.rdb$character_length, f.rdb$segment_length, \
     trim(cs.rdb$character_set_name), cs.rdb$bytes_per_character, trim(co.rdb$collation_name), \
     trim(cs.rdb$default_collate_name), f.rdb$dimensions";

/// The joins [`TYPE_COLUMNS`] needs. `collation` is the collation's id: a
/// column or a parameter can override its domain's.
pub(crate) fn type_joins(collation: &str) -> String {
    format!(
        "left join rdb$character_sets cs on cs.rdb$character_set_id = f.rdb$character_set_id \
         left join rdb$collations co on co.rdb$character_set_id = f.rdb$character_set_id \
              and co.rdb$collation_id = {collation}"
    )
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DataType {
    /// `rdb$field_type`
    pub code: i64,
    pub sub_type: i64,
    /// In bytes
    pub length: i64,
    pub precision: i64,
    /// Negative: the digits after the decimal point
    pub scale: i64,
    /// Of a CHAR or VARCHAR, in characters
    pub char_length: i64,
    pub segment_length: i64,
    /// Of a text type
    pub charset: Option<String>,
    /// When it is not the character set's own
    pub collation: Option<String>,
    /// An array's bounds, one pair per dimension
    pub dimensions: Vec<(i64, i64)>,
    /// How many dimensions the catalog says there are; `dimensions` is
    /// filled in by whoever reads the bounds
    #[serde(skip)]
    pub(crate) array: i64,
}

impl DataType {
    /// From a row's [`TYPE_COLUMNS`], which start at column `at`
    pub(crate) fn read(row: &Row, at: usize) -> Self {
        let int = |i: usize| row.int(at + i).unwrap_or(0);
        let length = int(2);
        let charset = row.text(at + 7);
        let collation = row.text(at + 9).filter(|c| Some(c) != row.text(at + 10).as_ref());
        let mut t = DataType {
            code: int(0),
            sub_type: int(1),
            length,
            precision: int(3),
            scale: int(4),
            char_length: match int(5) {
                // Old databases leave it null
                0 => length / int(8).max(1),
                n => n,
            },
            segment_length: int(6),
            charset: None,
            collation: None,
            dimensions: Vec::new(),
            array: int(11),
        };
        if t.is_text() {
            t.charset = charset;
            t.collation = collation;
        }
        t
    }

    /// Whether it has a character set
    pub fn is_text(&self) -> bool {
        matches!(self.code, code::CHAR | code::VARCHAR | code::CSTRING)
            || (self.code == code::BLOB && self.sub_type == 1)
    }

    /// As it is declared, without the character set: `NUMERIC(18,4)`,
    /// `VARCHAR(60)`, `BLOB SUB_TYPE TEXT`. `dialect` is the database's: in
    /// dialect 1 a DATE holds the time too.
    pub fn name(&self, dialect: u32) -> String {
        let scaled = |base: &str, digits: i64| -> String {
            let precision = if self.precision > 0 { self.precision } else { digits };
            let decimal = if self.sub_type == 2 { "DECIMAL" } else { "NUMERIC" };
            match self.scale {
                0 if self.sub_type == 0 => base.to_string(),
                0 => format!("{decimal}({precision})"),
                s => format!("{decimal}({precision},{})", -s),
            }
        };
        let mut name = match self.code {
            code::SMALLINT => scaled("SMALLINT", 4),
            code::INTEGER => scaled("INTEGER", 9),
            code::BIGINT => scaled("BIGINT", 18),
            code::INT128 => scaled("INT128", 38),
            code::FLOAT => "FLOAT".into(),
            // Dialect 1 keeps a NUMERIC wider than 9 digits as a double
            code::DOUBLE if self.scale < 0 => scaled("DOUBLE PRECISION", 15),
            code::DOUBLE | code::D_FLOAT => "DOUBLE PRECISION".into(),
            code::DECFLOAT16 => "DECFLOAT(16)".into(),
            code::DECFLOAT34 => "DECFLOAT(34)".into(),
            code::DATE => "DATE".into(),
            code::TIME => "TIME".into(),
            code::TIMESTAMP if dialect == 1 => "DATE".into(),
            code::TIMESTAMP => "TIMESTAMP".into(),
            code::TIME_TZ => "TIME WITH TIME ZONE".into(),
            code::TIMESTAMP_TZ => "TIMESTAMP WITH TIME ZONE".into(),
            code::BOOLEAN => "BOOLEAN".into(),
            code::CHAR => format!("CHAR({})", self.char_length),
            code::VARCHAR => format!("VARCHAR({})", self.char_length),
            code::CSTRING => format!("CSTRING({})", self.char_length),
            code::BLOB if self.sub_type == 1 => "BLOB SUB_TYPE TEXT".into(),
            code::BLOB => format!("BLOB SUB_TYPE {}", self.sub_type),
            code::QUAD => "QUAD".into(),
            other => format!("TYPE {other}"),
        };
        if !self.dimensions.is_empty() {
            let bounds: Vec<String> = self
                .dimensions
                .iter()
                .map(|(lower, upper)| if *lower == 1 { upper.to_string() } else { format!("{lower}:{upper}") })
                .collect();
            name.push_str(&format!(" [{}]", bounds.join(", ")));
        }
        name
    }

    /// As DDL declares it: the name, and the character set when it is not
    /// the database's default. The collation goes elsewhere in a
    /// declaration, after the default and NOT NULL.
    pub fn declaration(&self, dialect: u32, default_charset: &str) -> String {
        let mut name = self.name(dialect);
        if let Some(charset) = self.charset.as_deref().filter(|c| *c != default_charset) {
            // An array's bounds come after the character set
            let at = name.find(" [").unwrap_or(name.len());
            name.insert_str(at, &format!(" CHARACTER SET {charset}"));
        }
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(code: i64) -> DataType {
        DataType { code, ..Default::default() }
    }

    #[test]
    fn names_follow_the_declaration() {
        assert_eq!(t(8).name(3), "INTEGER");
        assert_eq!(DataType { sub_type: 1, precision: 18, scale: -4, ..t(16) }.name(3), "NUMERIC(18,4)");
        assert_eq!(DataType { sub_type: 2, precision: 9, scale: -2, ..t(8) }.name(3), "DECIMAL(9,2)");
        assert_eq!(DataType { sub_type: 1, precision: 18, ..t(16) }.name(3), "NUMERIC(18)");
        // Before precisions were kept, the type's own width stands in
        assert_eq!(DataType { scale: -2, ..t(7) }.name(3), "NUMERIC(4,2)");
        assert_eq!(DataType { sub_type: 1, precision: 38, scale: -10, ..t(26) }.name(3), "NUMERIC(38,10)");
        assert_eq!(DataType { scale: -2, ..t(27) }.name(1), "NUMERIC(15,2)");
        assert_eq!(t(27).name(3), "DOUBLE PRECISION");
        assert_eq!(t(35).name(3), "TIMESTAMP");
        assert_eq!(t(35).name(1), "DATE");
        assert_eq!(DataType { char_length: 60, ..t(37) }.name(3), "VARCHAR(60)");
        assert_eq!(DataType { sub_type: 1, ..t(261) }.name(3), "BLOB SUB_TYPE TEXT");
        assert_eq!(t(261).name(3), "BLOB SUB_TYPE 0");
        assert_eq!(DataType { dimensions: vec![(1, 4), (0, 2)], ..t(8) }.name(3), "INTEGER [4, 0:2]");
    }

    #[test]
    fn the_character_set_shows_when_it_is_not_the_default() {
        let text = DataType { char_length: 10, charset: Some("WIN1252".into()), ..t(14) };
        assert_eq!(text.declaration(3, "UTF8"), "CHAR(10) CHARACTER SET WIN1252");
        assert_eq!(text.declaration(3, "WIN1252"), "CHAR(10)");
        let array = DataType { dimensions: vec![(1, 3)], ..text };
        assert_eq!(array.declaration(3, "NONE"), "CHAR(10) CHARACTER SET WIN1252 [3]");
    }
}
