//! The names an SQL editor completes: every relation with its columns, what
//! else a statement can name, and the foreign keys a join is written from.
//! Read whole, in a few queries, so that completing costs the editor no
//! round trip.

use fb_core::{Connection, Error, Result};
use serde::Serialize;

use crate::diagram::{references, DiagramReference};
use crate::ident::quote;
use crate::row::rows;
use crate::types::{type_joins, DataType, TYPE_COLUMNS};
use crate::{objects, ObjectKind};

/// A name, and how SQL writes it in this database: in double quotes when it
/// would not read back bare
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Word {
    pub name: String,
    pub sql: String,
}

impl Word {
    fn new(name: String, dialect: u32) -> Self {
        Word { sql: quote(&name, dialect), name }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    Table,
    View,
    /// The catalog's own: `RDB$`, `MON$`, `SEC$`
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Relation {
    #[serde(flatten)]
    pub word: Word,
    pub kind: RelationKind,
    /// In the order the relation declares them
    pub columns: Vec<Column>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Column {
    #[serde(flatten)]
    pub word: Word,
    /// As it would be declared: `NUMERIC(18,4)`, `VARCHAR(60)`
    #[serde(rename = "type")]
    pub data_type: String,
}

/// Something that is not a relation but is named in statements
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Named {
    #[serde(flatten)]
    pub word: Word,
    pub kind: ObjectKind,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Catalog {
    /// By name
    pub relations: Vec<Relation>,
    pub others: Vec<Named>,
    /// Every foreign key: what a join between two relations is on
    pub references: Vec<DiagramReference>,
}

/// The kinds besides relations that statements name
const OTHERS: [ObjectKind; 6] = [
    ObjectKind::Procedure,
    ObjectKind::Function,
    ObjectKind::Package,
    ObjectKind::Generator,
    ObjectKind::Exception,
    ObjectKind::Domain,
];

pub fn catalog(conn: &Connection) -> Result<Catalog> {
    let dialect = conn.info().dialect;
    let sql = format!(
        "select trim(rf.rdb$relation_name), trim(rf.rdb$field_name), \
                coalesce(r.rdb$system_flag, 0), \
                case when r.rdb$view_blr is null then 0 else 1 end, \
                {TYPE_COLUMNS} \
         from rdb$relation_fields rf \
         join rdb$relations r on r.rdb$relation_name = rf.rdb$relation_name \
         join rdb$fields f on f.rdb$field_name = rf.rdb$field_source \
         {} \
         order by 1, rf.rdb$field_position",
        type_joins("coalesce(rf.rdb$collation_id, f.rdb$collation_id)")
    );

    let mut relations: Vec<Relation> = Vec::new();
    for row in rows(conn, &sql, vec![])? {
        let (Some(relation), Some(column), Some(system), Some(view)) = (row.text(0), row.text(1), row.int(2), row.int(3))
        else {
            return Err(Error::Client { message: "unexpected column row in the completion catalog".into() });
        };
        if relations.last().is_none_or(|r| r.word.name != relation) {
            let kind = match (system, view) {
                (0, 0) => RelationKind::Table,
                (0, _) => RelationKind::View,
                _ => RelationKind::System,
            };
            relations.push(Relation { word: Word::new(relation, dialect), kind, columns: Vec::new() });
        }
        let data_type = DataType::read(&row, 4).name(dialect);
        relations.last_mut().expect("just pushed").columns.push(Column { word: Word::new(column, dialect), data_type });
    }

    let mut others = Vec::new();
    for kind in OTHERS {
        // Empty for a kind this server does not have
        for object in objects(conn, kind)? {
            others.push(Named { word: Word::new(object.name, dialect), kind });
        }
    }
    Ok(Catalog { relations, others, references: references(conn)? })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_is_quoted_only_where_it_must_be() {
        assert_eq!(Word::new("PRODUTO".into(), 3).sql, "PRODUTO");
        assert_eq!(Word::new("Produto".into(), 3).sql, "\"Produto\"");
        assert_eq!(Word::new("VALUE".into(), 3).sql, "\"VALUE\"");
        assert_eq!(Word::new("VALUE".into(), 1).sql, "VALUE");
    }
}
