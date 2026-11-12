//! The small kinds: domains, generators, exceptions and roles.

use fb_core::{Connection, Result};
use serde::Serialize;

use crate::ident::quote;
use crate::row::{inline, rows, whole};
use crate::table::{fill_dimensions, not_found};
use crate::types::{type_joins, DataType, TYPE_COLUMNS};
use crate::Generation;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Domain {
    pub name: String,
    pub data_type: DataType,
    /// As written: `DEFAULT 0`
    pub default: Option<String>,
    /// As written: `CHECK (VALUE > 0)`
    pub check: Option<String>,
    pub not_null: bool,
    pub description: Option<String>,
}

pub fn domain(conn: &Connection, name: &str) -> Result<Domain> {
    domains(conn, Some(name))?.pop().ok_or_else(|| not_found("domain", name))
}

/// One domain, or every domain of the user's, by name
pub fn domains(conn: &Connection, name: Option<&str>) -> Result<Vec<Domain>> {
    let (filter, params) = match name {
        Some(name) => ("f.rdb$field_name = ?", vec![name.into()]),
        // Columns declared with a plain type get domains named RDB$<n>
        None => ("coalesce(f.rdb$system_flag, 0) = 0 and f.rdb$field_name not starting with 'RDB$'", vec![]),
    };
    let sql = format!(
        "select trim(f.rdb$field_name), f.rdb$null_flag, {default}, {check}, {description}, {TYPE_COLUMNS} \
         from rdb$fields f {type_joins} where {filter} order by 1",
        default = inline("f.rdb$default_source"),
        check = inline("f.rdb$validation_source"),
        description = inline("f.rdb$description"),
        type_joins = type_joins("f.rdb$collation_id"),
    );
    rows(conn, &sql, params)?
        .into_iter()
        .map(|row| {
            let name = row.text(0).unwrap_or_default();
            let of_domain = |column: &str| format!("select f.{column} from rdb$fields f where f.rdb$field_name = ?");
            let mut data_type = DataType::read(&row, 5);
            fill_dimensions(conn, &name, &mut data_type)?;
            Ok(Domain {
                not_null: row.flag(1),
                default: whole(conn, row.lob(2), &of_domain("rdb$default_source"), &[&name])?,
                check: whole(conn, row.lob(3), &of_domain("rdb$validation_source"), &[&name])?,
                description: whole(conn, row.lob(4), &of_domain("rdb$description"), &[&name])?,
                data_type,
                name,
            })
        })
        .collect()
}

/// A generator (a sequence)
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Generator {
    pub name: String,
    /// What it holds now; `None` when it was not asked for
    pub value: Option<i64>,
    /// Firebird 3 and later
    pub initial: Option<i64>,
    pub increment: Option<i64>,
    pub description: Option<String>,
}

/// A generator with its current value, which reading does not change
pub fn generator(conn: &Connection, name: &str) -> Result<Generator> {
    let mut generator = generators(conn, Some(name))?.pop().ok_or_else(|| not_found("generator", name))?;
    let value = format!("select gen_id({}, 0) from rdb$database", quote(name, conn.info().dialect));
    generator.value = rows(conn, &value, vec![])?.first().and_then(|r| r.int(0));
    Ok(generator)
}

/// One generator, or every generator of the user's, by name, without their
/// values
pub fn generators(conn: &Connection, name: Option<&str>) -> Result<Vec<Generator>> {
    let fb3 = Generation::of(conn.info()) == Generation::Fb3;
    let (filter, params) = match name {
        Some(name) => ("g.rdb$generator_name = ?", vec![name.into()]),
        None => ("coalesce(g.rdb$system_flag, 0) = 0", vec![]),
    };
    let shape = if fb3 {
        "g.rdb$initial_value, g.rdb$generator_increment"
    } else {
        "cast(null as bigint), cast(null as integer)"
    };
    let sql = format!(
        "select trim(g.rdb$generator_name), {shape}, {description} from rdb$generators g where {filter} order by 1",
        description = inline("g.rdb$description"),
    );
    rows(conn, &sql, params)?
        .into_iter()
        .map(|row| {
            let name = row.text(0).unwrap_or_default();
            let description = whole(
                conn,
                row.lob(3),
                "select rdb$description from rdb$generators where rdb$generator_name = ?",
                &[&name],
            )?;
            Ok(Generator { name, value: None, initial: row.int(1), increment: row.int(2), description })
        })
        .collect()
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Exception {
    pub name: String,
    pub number: i64,
    pub message: Option<String>,
    pub description: Option<String>,
}

pub fn exception(conn: &Connection, name: &str) -> Result<Exception> {
    exceptions(conn, Some(name))?.pop().ok_or_else(|| not_found("exception", name))
}

/// One exception, or every exception of the user's, by name
pub fn exceptions(conn: &Connection, name: Option<&str>) -> Result<Vec<Exception>> {
    let (filter, params) = match name {
        Some(name) => ("e.rdb$exception_name = ?", vec![name.into()]),
        None => ("coalesce(e.rdb$system_flag, 0) = 0", vec![]),
    };
    let sql = format!(
        "select trim(e.rdb$exception_name), e.rdb$exception_number, e.rdb$message, {description} \
         from rdb$exceptions e where {filter} order by 1",
        description = inline("e.rdb$description"),
    );
    rows(conn, &sql, params)?
        .into_iter()
        .map(|row| {
            let name = row.text(0).unwrap_or_default();
            let description = whole(
                conn,
                row.lob(3),
                "select rdb$description from rdb$exceptions where rdb$exception_name = ?",
                &[&name],
            )?;
            Ok(Exception { name, number: row.int(1).unwrap_or(0), message: row.text(2), description })
        })
        .collect()
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Role {
    pub name: String,
    pub owner: Option<String>,
    pub description: Option<String>,
}

pub fn role(conn: &Connection, name: &str) -> Result<Role> {
    let sql = format!(
        "select trim(o.rdb$owner_name), {description} from rdb$roles o where o.rdb$role_name = ?",
        description = inline("o.rdb$description"),
    );
    let row = rows(conn, &sql, vec![name.into()])?.pop().ok_or_else(|| not_found("role", name))?;
    let description =
        whole(conn, row.lob(1), "select rdb$description from rdb$roles where rdb$role_name = ?", &[name])?;
    Ok(Role { name: name.to_string(), owner: row.text(0), description })
}
