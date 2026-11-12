//! A table's rows changed from a grid: the statements that make the
//! changes. A row is told from the others by its key, a column's new value
//! is written as the literal its type takes, and what the server makes of
//! a value it cannot convert is the server's to say.

use fb_core::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::ddl::{Context, Statement};
use crate::ident::{literal, quote};
use crate::table::{Column, ConstraintKind, Table};
use crate::types::Written;

/// How a table's rows can be changed
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RowEditing {
    /// The columns that tell a row from every other
    pub key: Vec<String>,
    /// The columns a value can be typed into
    pub editable: Vec<String>,
}

/// How the table's rows can be changed; `None` when nothing tells one row
/// from another: without a primary key, or a unique one that has no nulls,
/// a change could reach rows it was not meant for
pub fn editing(table: &Table) -> Option<RowEditing> {
    let column = |name: &String| table.columns.iter().find(|c| &c.name == name);
    let identifies = |columns: &[String], nulls: bool| {
        !columns.is_empty()
            && columns.iter().all(|name| {
                column(name).is_some_and(|c| {
                    c.data_type.written().is_some() && !c.data_type.is_approximate() && (nulls || c.required())
                })
            })
    };
    let key = table
        .constraints
        .iter()
        .find(|c| c.kind == ConstraintKind::PrimaryKey && identifies(&c.columns, true))
        .or_else(|| table.constraints.iter().find(|c| c.kind == ConstraintKind::Unique && identifies(&c.columns, false)))?;
    let editable =
        table.columns.iter().filter(|c| c.computed.is_none() && c.data_type.written().is_some()).map(|c| c.name.clone());
    Some(RowEditing { key: key.columns.clone(), editable: editable.collect() })
}

/// A column given a value; `None` is NULL
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Assign {
    pub column: String,
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RowUpdate {
    /// The row's key as it was read, in the key's column order
    pub key: Vec<Option<String>>,
    pub set: Vec<Assign>,
}

/// What was done to a grid's rows and is not in the table yet
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct RowChanges {
    pub updates: Vec<RowUpdate>,
    /// The keys of the rows to delete
    pub deletes: Vec<Vec<Option<String>>>,
    /// New rows: the columns given a value; the rest take their defaults
    pub inserts: Vec<Vec<Assign>>,
}

fn invalid(message: String) -> Error {
    Error::Client { message }
}

/// Whether text reads as a number: `12`, `-12.50`, `1e-3`
fn is_number(text: &str) -> bool {
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
        Some((m, e)) => (m, Some(e)),
        None => (unsigned, None),
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let mantissa_ok = (digits(int) && (frac.is_empty() || digits(frac))) || (int.is_empty() && digits(frac));
    mantissa_ok && exponent.is_none_or(|e| digits(e.strip_prefix(['+', '-']).unwrap_or(e)))
}

/// A typed value as the literal the column takes
fn written(column: &Column, value: Option<&str>) -> String {
    let Some(value) = value else { return "NULL".into() };
    let trimmed = value.trim();
    // Only text can hold nothing and not be null
    if trimmed.is_empty() && !column.data_type.is_text() {
        return "NULL".into();
    }
    match column.data_type.written() {
        Some(Written::Number) => {
            // A decimal comma, as half the world types it
            let dotted = if trimmed.matches(',').count() == 1 && !trimmed.contains('.') {
                trimmed.replace(',', ".")
            } else {
                trimmed.to_string()
            };
            if is_number(&dotted) {
                dotted
            } else {
                literal(value)
            }
        }
        Some(Written::Boolean) if trimmed.eq_ignore_ascii_case("true") => "TRUE".into(),
        Some(Written::Boolean) if trimmed.eq_ignore_ascii_case("false") => "FALSE".into(),
        Some(Written::Quoted) if !column.data_type.is_text() => literal(trimmed),
        _ => literal(value),
    }
}

struct Writer<'a> {
    table: &'a Table,
    editing: RowEditing,
    ctx: &'a Context,
}

impl Writer<'_> {
    fn column(&self, name: &str) -> Result<&Column> {
        self.table
            .columns
            .iter()
            .find(|c| c.name == name)
            .ok_or_else(|| invalid(format!("{} has no column {name}.", self.table.name)))
    }

    fn name(&self, name: &str) -> String {
        quote(name, self.ctx.dialect)
    }

    /// `A = 1, B = 'x'`, of columns that can be typed into
    fn assignments(&self, set: &[Assign]) -> Result<Vec<(String, String)>> {
        set.iter()
            .map(|a| {
                if !self.editing.editable.contains(&a.column) {
                    return Err(invalid(format!("The column {} cannot be given a value.", a.column)));
                }
                Ok((self.name(&a.column), written(self.column(&a.column)?, a.value.as_deref())))
            })
            .collect()
    }

    /// `WHERE ID = 1 AND ...`: the one row with the key
    fn row(&self, key: &[Option<String>]) -> Result<String> {
        if key.len() != self.editing.key.len() {
            return Err(invalid("A row's key does not match the table's.".into()));
        }
        let mut conditions = Vec::new();
        for (name, value) in self.editing.key.iter().zip(key) {
            let column = self.column(name)?;
            conditions.push(match value {
                None => format!("{} IS NULL", self.name(name)),
                Some(value) => format!("{} = {}", self.name(name), written(column, Some(value))),
            });
        }
        Ok(format!("WHERE {}", conditions.join(" AND ")))
    }
}

/// The statements that make the changes: rows deleted first, which frees
/// the values the changed and the new rows may want
pub fn statements(table: &Table, changes: &RowChanges, ctx: &Context) -> Result<Vec<Statement>> {
    let editing = editing(table).ok_or_else(|| {
        invalid(format!("{} has no key that tells its rows apart: they cannot be changed here.", table.name))
    })?;
    let w = Writer { table, editing, ctx };
    let name = w.name(&table.name);
    let mut out = Vec::new();
    for key in &changes.deletes {
        out.push(Statement::plain(format!("DELETE FROM {name} {}", w.row(key)?)));
    }
    for update in &changes.updates {
        let set: Vec<String> = w.assignments(&update.set)?.into_iter().map(|(c, v)| format!("{c} = {v}")).collect();
        if !set.is_empty() {
            out.push(Statement::plain(format!("UPDATE {name} SET {} {}", set.join(", "), w.row(&update.key)?)));
        }
    }
    for insert in &changes.inserts {
        let (columns, values): (Vec<String>, Vec<String>) = w.assignments(insert)?.into_iter().unzip();
        out.push(Statement::plain(if columns.is_empty() {
            format!("INSERT INTO {name} DEFAULT VALUES")
        } else {
            format!("INSERT INTO {name} ({}) VALUES ({})", columns.join(", "), values.join(", "))
        }));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::Constraint;
    use crate::types::DataType;

    fn ctx() -> Context {
        Context { dialect: 3, charset: "UTF8".into(), major: 4 }
    }

    fn column(name: &str, code: i64) -> Column {
        Column { name: name.into(), data_type: DataType { code, ..DataType::default() }, ..Column::default() }
    }

    fn key(kind: ConstraintKind, columns: &[&str]) -> Constraint {
        Constraint {
            name: "K".into(),
            kind,
            columns: columns.iter().map(|c| c.to_string()).collect(),
            index: None,
            index_descending: false,
            references: None,
            check: None,
        }
    }

    /// ID INTEGER, NOME VARCHAR, VALOR DOUBLE, DIA DATE, ATIVO BOOLEAN, FOTO BLOB, TOTAL computed
    fn table() -> Table {
        let mut total = column("TOTAL", 8);
        total.computed = Some("(VALOR * 2)".into());
        let mut nome = column("NOME", 37);
        nome.data_type.charset = Some("UTF8".into());
        Table {
            name: "PEDIDO".into(),
            columns: vec![column("ID", 8), nome, column("VALOR", 27), column("DIA", 12), column("ATIVO", 23), column("FOTO", 261), total],
            constraints: vec![key(ConstraintKind::PrimaryKey, &["ID"])],
            ..Table::default()
        }
    }

    fn set(column: &str, value: Option<&str>) -> Assign {
        Assign { column: column.into(), value: value.map(String::from) }
    }

    fn sql(table: &Table, changes: &RowChanges) -> Vec<String> {
        statements(table, changes, &ctx()).unwrap().into_iter().map(|s| s.sql).collect()
    }

    #[test]
    fn a_table_is_edited_by_its_key_or_not_at_all() {
        let editing = editing(&table()).unwrap();
        assert_eq!(editing.key, ["ID"]);
        // Neither the blob nor the computed column
        assert_eq!(editing.editable, ["ID", "NOME", "VALOR", "DIA", "ATIVO"]);

        let mut keyless = table();
        keyless.constraints.clear();
        assert_eq!(super::editing(&keyless), None);
        assert!(statements(&keyless, &RowChanges::default(), &ctx()).is_err());

        // A unique key tells rows apart only if it has no nulls
        keyless.constraints = vec![key(ConstraintKind::Unique, &["NOME"])];
        assert_eq!(super::editing(&keyless), None);
        keyless.columns[1].not_null = true;
        assert_eq!(super::editing(&keyless).unwrap().key, ["NOME"]);
        // An approximate number is not something to look a row up by
        keyless.constraints = vec![key(ConstraintKind::PrimaryKey, &["VALOR"])];
        assert_eq!(super::editing(&keyless), None);
    }

    #[test]
    fn values_are_written_as_their_columns_take_them() {
        let changes = RowChanges {
            updates: vec![RowUpdate {
                key: vec![Some("7".into())],
                set: vec![
                    set("NOME", Some("O'Brien ")),
                    set("VALOR", Some(" 12,5 ")),
                    set("DIA", Some(" 2024-02-29 ")),
                    set("ATIVO", Some("true")),
                ],
            }],
            deletes: vec![vec![Some("9".into())]],
            inserts: vec![vec![set("NOME", Some("")), set("VALOR", Some("")), set("DIA", None)], vec![]],
        };
        assert_eq!(
            sql(&table(), &changes),
            [
                "DELETE FROM PEDIDO WHERE ID = 9",
                "UPDATE PEDIDO SET NOME = 'O''Brien ', VALOR = 12.5, DIA = '2024-02-29', ATIVO = TRUE WHERE ID = 7",
                "INSERT INTO PEDIDO (NOME, VALOR, DIA) VALUES ('', NULL, NULL)",
                "INSERT INTO PEDIDO DEFAULT VALUES",
            ]
        );
    }

    #[test]
    fn what_is_not_a_number_is_left_for_the_server_to_refuse() {
        for number in ["12", "-12.50", "+.5", "1e-3", "3."] {
            assert!(is_number(number), "{number}");
        }
        for other in ["", "-", "1.2.3", "1,000.5", "abc", "1e", "0x10"] {
            assert!(!is_number(other), "{other}");
        }
        let update = |value: &str| RowChanges {
            updates: vec![RowUpdate { key: vec![Some("1".into())], set: vec![set("VALOR", Some(value))] }],
            ..RowChanges::default()
        };
        assert_eq!(sql(&table(), &update("1.000,50")), ["UPDATE PEDIDO SET VALOR = '1.000,50' WHERE ID = 1"]);
        assert_eq!(sql(&table(), &update("1; DROP TABLE PEDIDO")), ["UPDATE PEDIDO SET VALOR = '1; DROP TABLE PEDIDO' WHERE ID = 1"]);
    }

    #[test]
    fn only_what_can_be_typed_is_changed_and_only_by_a_whole_key() {
        let bad = |changes: RowChanges| statements(&table(), &changes, &ctx()).is_err();
        let update = |key: Vec<Option<String>>, column: &str| RowChanges {
            updates: vec![RowUpdate { key, set: vec![set(column, Some("1"))] }],
            ..RowChanges::default()
        };
        assert!(bad(update(vec![Some("1".into())], "TOTAL")));
        assert!(bad(update(vec![Some("1".into())], "FOTO")));
        assert!(bad(update(vec![Some("1".into())], "NADA")));
        assert!(bad(update(vec![], "NOME")));
        assert!(bad(RowChanges { deletes: vec![vec![]], ..RowChanges::default() }));
    }
}
