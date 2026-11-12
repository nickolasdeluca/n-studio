//! Tables and views: their columns, constraints, indices and triggers.
//!
//! Each reader takes one relation's name or `None` for every user relation,
//! so that extracting a whole database's metadata costs the same handful of
//! queries as looking at one table.

use fb_core::{Connection, Error, Result, Value};
use serde::Serialize;

use crate::row::{inline, rows, whole};
use crate::types::{type_joins, DataType, TYPE_COLUMNS};
use crate::Generation;

/// A `where` condition for one relation, or for every user relation. The
/// query joins `rdb$relations r`.
pub(crate) fn scope(column: &str, name: Option<&str>) -> (String, Vec<Value>) {
    match name {
        Some(name) => (format!("{column} = ?"), vec![name.into()]),
        None => ("coalesce(r.rdb$system_flag, 0) = 0".into(), vec![]),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub name: String,
    pub data_type: DataType,
    /// The domain it is declared with, when that is a domain of the user's
    pub domain: Option<String>,
    /// Declared NOT NULL itself
    pub not_null: bool,
    /// Its domain is NOT NULL
    pub domain_not_null: bool,
    /// As written: `DEFAULT 0`
    pub default: Option<String>,
    pub domain_default: Option<String>,
    /// A computed column's expression, in its parentheses
    pub computed: Option<String>,
    pub identity: Option<Identity>,
    pub description: Option<String>,
}

impl Column {
    /// Whether it refuses nulls, by its own declaration or its domain's
    pub fn required(&self) -> bool {
        self.not_null || self.domain_not_null
    }

    /// The default that applies: its own, or else its domain's
    pub fn effective_default(&self) -> Option<&str> {
        self.default.as_deref().or(self.domain_default.as_deref())
    }
}

/// A column that numbers itself (Firebird 3 and later)
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    /// GENERATED ALWAYS, rather than BY DEFAULT
    pub always: bool,
    pub start: Option<i64>,
    pub increment: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintKind {
    PrimaryKey,
    Unique,
    ForeignKey,
    Check,
}

impl ConstraintKind {
    pub fn label(self) -> &'static str {
        match self {
            ConstraintKind::PrimaryKey => "PRIMARY KEY",
            ConstraintKind::Unique => "UNIQUE",
            ConstraintKind::ForeignKey => "FOREIGN KEY",
            ConstraintKind::Check => "CHECK",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "PRIMARY KEY" => ConstraintKind::PrimaryKey,
            "UNIQUE" => ConstraintKind::Unique,
            "FOREIGN KEY" => ConstraintKind::ForeignKey,
            "CHECK" => ConstraintKind::Check,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Constraint {
    pub name: String,
    pub kind: ConstraintKind,
    pub columns: Vec<String>,
    /// The index that enforces a key
    pub index: Option<String>,
    pub index_descending: bool,
    /// A foreign key's target
    pub references: Option<Reference>,
    /// A CHECK's text, as written: `CHECK (VALOR > 0)`
    pub check: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reference {
    pub table: String,
    pub columns: Vec<String>,
    /// `CASCADE`, `SET NULL`, `SET DEFAULT`; `None` for the default,
    /// RESTRICT
    pub on_update: Option<String>,
    pub on_delete: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Index {
    pub name: String,
    pub table: String,
    pub unique: bool,
    pub descending: bool,
    pub inactive: bool,
    pub columns: Vec<String>,
    /// An expression index's expression, in its parentheses
    pub expression: Option<String>,
    /// A partial index's condition, as written: `WHERE …` (Firebird 5)
    pub condition: Option<String>,
    /// Selectivity as of the last time it was computed: 1 / distinct keys
    pub statistics: Option<f64>,
    /// The constraint it enforces, if it is one's
    pub constraint: Option<ConstraintKind>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Trigger {
    pub name: String,
    /// `None` for a database or DDL trigger
    pub table: Option<String>,
    /// `rdb$trigger_type`
    pub type_code: i64,
    pub position: i64,
    pub inactive: bool,
    /// From `AS` on; only when asked for
    pub source: Option<String>,
    pub description: Option<String>,
}

impl Trigger {
    /// When it fires, as its declaration says: `BEFORE INSERT OR UPDATE`,
    /// `ON CONNECT`, `AFTER CREATE TABLE`
    pub fn event(&self) -> String {
        trigger_event(self.type_code)
    }
}

/// `rdb$trigger_type` → the declaration's event clause
fn trigger_event(code: i64) -> String {
    const KIND: i64 = 0x6000;
    match code & KIND {
        0x2000 => {
            let event = match code & 0xff {
                0 => "CONNECT",
                1 => "DISCONNECT",
                2 => "TRANSACTION START",
                3 => "TRANSACTION COMMIT",
                4 => "TRANSACTION ROLLBACK",
                _ => "?",
            };
            format!("ON {event}")
        }
        0x4000 => {
            let when = if code & 1 == 0 { "BEFORE" } else { "AFTER" };
            let events: Vec<&str> =
                DDL_EVENTS.iter().filter(|(bit, _)| code & (1 << bit) != 0).map(|(_, e)| *e).collect();
            if events.len() == DDL_EVENTS.len() {
                format!("{when} ANY DDL STATEMENT")
            } else {
                format!("{when} {}", events.join(" OR "))
            }
        }
        _ => {
            // Up to three two-bit actions, above a bit for before/after
            let t = code + 1;
            let when = if t & 1 == 1 { "AFTER" } else { "BEFORE" };
            let actions: Vec<&str> = [1, 3, 5]
                .iter()
                .filter_map(|shift| match (t >> shift) & 3 {
                    1 => Some("INSERT"),
                    2 => Some("UPDATE"),
                    3 => Some("DELETE"),
                    _ => None,
                })
                .collect();
            format!("{when} {}", actions.join(" OR "))
        }
    }
}

/// A DDL trigger's events, by their bit in `rdb$trigger_type`. Bits 13 to
/// 15 are skipped: they would collide with the bits that tell the kinds of
/// trigger apart.
const DDL_EVENTS: &[(u32, &str)] = &[
    (1, "CREATE TABLE"),
    (2, "ALTER TABLE"),
    (3, "DROP TABLE"),
    (4, "CREATE PROCEDURE"),
    (5, "ALTER PROCEDURE"),
    (6, "DROP PROCEDURE"),
    (7, "CREATE FUNCTION"),
    (8, "ALTER FUNCTION"),
    (9, "DROP FUNCTION"),
    (10, "CREATE TRIGGER"),
    (11, "ALTER TRIGGER"),
    (12, "DROP TRIGGER"),
    (16, "CREATE EXCEPTION"),
    (17, "ALTER EXCEPTION"),
    (18, "DROP EXCEPTION"),
    (19, "CREATE VIEW"),
    (20, "ALTER VIEW"),
    (21, "DROP VIEW"),
    (22, "CREATE DOMAIN"),
    (23, "ALTER DOMAIN"),
    (24, "DROP DOMAIN"),
    (25, "CREATE ROLE"),
    (26, "ALTER ROLE"),
    (27, "DROP ROLE"),
    (28, "CREATE INDEX"),
    (29, "ALTER INDEX"),
    (30, "DROP INDEX"),
    (31, "CREATE SEQUENCE"),
    (32, "ALTER SEQUENCE"),
    (33, "DROP SEQUENCE"),
    (34, "CREATE USER"),
    (35, "ALTER USER"),
    (36, "DROP USER"),
    (37, "CREATE COLLATION"),
    (38, "DROP COLLATION"),
    (39, "ALTER CHARACTER SET"),
    (40, "CREATE PACKAGE"),
    (41, "ALTER PACKAGE"),
    (42, "DROP PACKAGE"),
    (43, "CREATE PACKAGE BODY"),
    (44, "DROP PACKAGE BODY"),
    (45, "CREATE MAPPING"),
    (46, "ALTER MAPPING"),
    (47, "DROP MAPPING"),
];

/// How a table keeps its rows
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Storage {
    #[default]
    Persistent,
    /// In a file outside the database
    External(String),
    /// A global temporary table whose rows last the connection
    TemporaryPreserve,
    /// A global temporary table whose rows last the transaction
    TemporaryDelete,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Table {
    pub name: String,
    pub storage: Storage,
    pub columns: Vec<Column>,
    pub constraints: Vec<Constraint>,
    pub indices: Vec<Index>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub name: String,
    pub columns: Vec<Column>,
    /// The query, from after `AS`
    pub source: Option<String>,
    pub description: Option<String>,
}

/// What `rdb$relations` says of a table or a view
struct Relation {
    name: String,
    storage: Storage,
    view_source: Option<String>,
    description: Option<String>,
}

fn relations(conn: &Connection, name: Option<&str>, views: bool) -> Result<Vec<Relation>> {
    let (filter, params) = scope("r.rdb$relation_name", name);
    // A view's query is read whole: it is one blob per view
    let sql = format!(
        "select trim(r.rdb$relation_name), trim(r.rdb$external_file), r.rdb$relation_type, r.rdb$view_source, {description} \
         from rdb$relations r where {filter} and r.rdb$view_blr is {null} order by 1",
        description = inline("r.rdb$description"),
        null = if views { "not null" } else { "null" },
    );
    rows(conn, &sql, params)?
        .into_iter()
        .map(|row| {
            let name = row.text(0).ok_or_else(|| unexpected("relation"))?;
            let description = whole(
                conn,
                row.lob(4),
                "select rdb$description from rdb$relations where rdb$relation_name = ?",
                &[&name],
            )?;
            let storage = match (row.text(1), row.int(2)) {
                (Some(file), _) => Storage::External(file),
                (None, Some(4)) => Storage::TemporaryPreserve,
                (None, Some(5)) => Storage::TemporaryDelete,
                _ => Storage::Persistent,
            };
            Ok(Relation { name, storage, view_source: row.text(3), description })
        })
        .collect()
}

/// One table, with everything declared on it but its triggers
pub fn table(conn: &Connection, name: &str) -> Result<Table> {
    tables(conn, Some(name))?.pop().ok_or_else(|| not_found("table", name))
}

/// One table, or every user table, by name
pub fn tables(conn: &Connection, name: Option<&str>) -> Result<Vec<Table>> {
    let mut tables: Vec<Table> = relations(conn, name, false)?
        .into_iter()
        .map(|r| Table { name: r.name, storage: r.storage, description: r.description, ..Default::default() })
        .collect();
    if tables.is_empty() {
        return Ok(tables);
    }
    // Each list comes back by table name, as the tables are
    let at = |tables: &[Table], table: &str| tables.binary_search_by(|t| t.name.as_str().cmp(table)).ok();
    for (table, column) in columns(conn, name)? {
        if let Some(i) = at(&tables, &table) {
            tables[i].columns.push(column);
        }
    }
    for (table, constraint) in constraints(conn, name)? {
        if let Some(i) = at(&tables, &table) {
            tables[i].constraints.push(constraint);
        }
    }
    for index in indices(conn, name)? {
        if let Some(i) = at(&tables, &index.table) {
            tables[i].indices.push(index);
        }
    }
    for table in &mut tables {
        table.constraints.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
    }
    Ok(tables)
}

pub fn view(conn: &Connection, name: &str) -> Result<View> {
    views(conn, Some(name))?.pop().ok_or_else(|| not_found("view", name))
}

/// One view, or every user view, by name
pub fn views(conn: &Connection, name: Option<&str>) -> Result<Vec<View>> {
    let mut views: Vec<View> = relations(conn, name, true)?
        .into_iter()
        .map(|r| View { name: r.name, source: r.view_source, description: r.description, columns: Vec::new() })
        .collect();
    if views.is_empty() {
        return Ok(views);
    }
    for (relation, column) in columns(conn, name)? {
        if let Ok(i) = views.binary_search_by(|v| v.name.as_str().cmp(&relation)) {
            views[i].columns.push(column);
        }
    }
    Ok(views)
}

/// The columns of one relation, or of every user relation, each with its
/// relation's name, in the order they are declared
pub fn columns(conn: &Connection, relation: Option<&str>) -> Result<Vec<(String, Column)>> {
    let fb3 = Generation::of(conn.info()) == Generation::Fb3;
    let (identity, identity_join) = if fb3 {
        (
            "rf.rdb$identity_type, g.rdb$initial_value, g.rdb$generator_increment",
            "left join rdb$generators g on g.rdb$generator_name = rf.rdb$generator_name",
        )
    } else {
        ("cast(null as smallint), cast(null as bigint), cast(null as integer)", "")
    };
    let (filter, params) = scope("rf.rdb$relation_name", relation);
    let sql = format!(
        "select trim(rf.rdb$relation_name), trim(rf.rdb$field_name), trim(rf.rdb$field_source), \
                rf.rdb$null_flag, f.rdb$null_flag, {default}, {domain_default}, {computed}, {description}, \
                {identity}, {TYPE_COLUMNS} \
         from rdb$relation_fields rf \
         join rdb$relations r on r.rdb$relation_name = rf.rdb$relation_name \
         join rdb$fields f on f.rdb$field_name = rf.rdb$field_source \
         {type_joins} {identity_join} \
         where {filter} \
         order by rf.rdb$relation_name, rf.rdb$field_position",
        default = inline("rf.rdb$default_source"),
        domain_default = inline("f.rdb$default_source"),
        computed = inline("f.rdb$computed_source"),
        description = inline("rf.rdb$description"),
        type_joins = type_joins("coalesce(rf.rdb$collation_id, f.rdb$collation_id)"),
    );
    rows(conn, &sql, params)?
        .into_iter()
        .map(|row| {
            let (Some(table), Some(name), Some(source)) = (row.text(0), row.text(1), row.text(2)) else {
                return Err(unexpected("column"));
            };
            let of_column = |column: &str| {
                format!(
                    "select rf.{column} from rdb$relation_fields rf \
                     where rf.rdb$relation_name = ? and rf.rdb$field_name = ?"
                )
            };
            let of_domain = |column: &str| format!("select f.{column} from rdb$fields f where f.rdb$field_name = ?");
            let mut data_type = DataType::read(&row, 12);
            fill_dimensions(conn, &source, &mut data_type)?;
            let column = Column {
                default: whole(conn, row.lob(5), &of_column("rdb$default_source"), &[&table, &name])?,
                domain_default: whole(conn, row.lob(6), &of_domain("rdb$default_source"), &[&source])?,
                computed: whole(conn, row.lob(7), &of_domain("rdb$computed_source"), &[&source])?,
                description: whole(conn, row.lob(8), &of_column("rdb$description"), &[&table, &name])?,
                identity: row.int(9).map(|kind| Identity {
                    always: kind == 0,
                    start: row.int(10),
                    increment: row.int(11),
                }),
                not_null: row.flag(3),
                domain_not_null: row.flag(4),
                // A column declared with a plain type gets a domain of its
                // own, named RDB$<n>
                domain: (!source.starts_with("RDB$")).then_some(source),
                data_type,
                name,
            };
            Ok((table, column))
        })
        .collect()
}

/// Reads an array type's bounds, which have a table of their own
pub(crate) fn fill_dimensions(conn: &Connection, domain: &str, data_type: &mut DataType) -> Result<()> {
    if data_type.array == 0 {
        return Ok(());
    }
    let bounds = rows(
        conn,
        "select rdb$lower_bound, rdb$upper_bound from rdb$field_dimensions \
         where rdb$field_name = ? order by rdb$dimension",
        vec![domain.into()],
    )?;
    data_type.dimensions = bounds.iter().map(|r| (r.int(0).unwrap_or(1), r.int(1).unwrap_or(1))).collect();
    Ok(())
}

/// The constraints of one table, or of every user table, each with its
/// table's name. NOT NULL is left to the columns.
pub fn constraints(conn: &Connection, table: Option<&str>) -> Result<Vec<(String, Constraint)>> {
    let (filter, params) = scope("rc.rdb$relation_name", table);
    // One row per key column; a foreign key's row has the column it
    // references beside it
    let keys = format!(
        "select trim(rc.rdb$relation_name), trim(rc.rdb$constraint_name), trim(rc.rdb$constraint_type), \
                trim(rc.rdb$index_name), i.rdb$index_type, trim(s.rdb$field_name), \
                trim(uq.rdb$relation_name), trim(us.rdb$field_name), \
                trim(ref.rdb$update_rule), trim(ref.rdb$delete_rule) \
         from rdb$relation_constraints rc \
         join rdb$relations r on r.rdb$relation_name = rc.rdb$relation_name \
         left join rdb$indices i on i.rdb$index_name = rc.rdb$index_name \
         left join rdb$index_segments s on s.rdb$index_name = rc.rdb$index_name \
         left join rdb$ref_constraints ref on ref.rdb$constraint_name = rc.rdb$constraint_name \
         left join rdb$relation_constraints uq on uq.rdb$constraint_name = ref.rdb$const_name_uq \
         left join rdb$index_segments us on us.rdb$index_name = uq.rdb$index_name \
              and us.rdb$field_position = s.rdb$field_position \
         where {filter} and rc.rdb$constraint_type in ('PRIMARY KEY', 'UNIQUE', 'FOREIGN KEY') \
         order by rc.rdb$relation_name, rc.rdb$constraint_name, s.rdb$field_position"
    );
    let mut found: Vec<(String, Constraint)> = Vec::new();
    for row in rows(conn, &keys, params.clone())? {
        let (Some(table), Some(name), Some(kind)) = (row.text(0), row.text(1), row.text(2)) else {
            return Err(unexpected("constraint"));
        };
        let kind = ConstraintKind::parse(&kind).ok_or_else(|| unexpected("constraint type"))?;
        if found.last().is_none_or(|(t, c)| *t != table || c.name != name) {
            // RESTRICT is what a foreign key does when it says nothing
            let rule = |i: usize| row.text(i).filter(|r| r != "RESTRICT");
            let references = row.text(6).map(|table| Reference {
                table,
                columns: Vec::new(),
                on_update: rule(8),
                on_delete: rule(9),
            });
            let constraint = Constraint {
                name,
                kind,
                columns: Vec::new(),
                index: row.text(3),
                index_descending: row.int(4) == Some(1),
                references,
                check: None,
            };
            found.push((table, constraint));
        }
        let constraint = &mut found.last_mut().expect("pushed above").1;
        constraint.columns.extend(row.text(5));
        if let Some(references) = &mut constraint.references {
            references.columns.extend(row.text(7));
        }
    }

    // A CHECK is enforced by two triggers the engine writes, which share
    // its text; the one before insert is always there
    let checks = format!(
        "select trim(rc.rdb$relation_name), trim(rc.rdb$constraint_name), {source} \
         from rdb$relation_constraints rc \
         join rdb$relations r on r.rdb$relation_name = rc.rdb$relation_name \
         join rdb$check_constraints cc on cc.rdb$constraint_name = rc.rdb$constraint_name \
         join rdb$triggers t on t.rdb$trigger_name = cc.rdb$trigger_name and t.rdb$trigger_type = 1 \
         where {filter} and rc.rdb$constraint_type = 'CHECK' \
         order by rc.rdb$relation_name, rc.rdb$constraint_name",
        source = inline("t.rdb$trigger_source"),
    );
    for row in rows(conn, &checks, params)? {
        let (Some(table), Some(name)) = (row.text(0), row.text(1)) else {
            return Err(unexpected("check constraint"));
        };
        let check = whole(
            conn,
            row.lob(2),
            "select t.rdb$trigger_source from rdb$check_constraints cc \
             join rdb$triggers t on t.rdb$trigger_name = cc.rdb$trigger_name and t.rdb$trigger_type = 1 \
             where cc.rdb$constraint_name = ?",
            &[&name],
        )?;
        let constraint = Constraint {
            name,
            kind: ConstraintKind::Check,
            columns: Vec::new(),
            index: None,
            index_descending: false,
            references: None,
            check: check.map(|c| c.trim().to_string()),
        };
        found.push((table, constraint));
    }
    Ok(found)
}

/// The indices of one table, or of every user table, by table and name
pub fn indices(conn: &Connection, table: Option<&str>) -> Result<Vec<Index>> {
    let (filter, params) = scope("i.rdb$relation_name", table);
    indices_where(conn, &filter, params)
}

/// One index, by its own name
pub fn index(conn: &Connection, name: &str) -> Result<Index> {
    indices_where(conn, "i.rdb$index_name = ?", vec![name.into()])?.pop().ok_or_else(|| not_found("index", name))
}

fn indices_where(conn: &Connection, filter: &str, params: Vec<Value>) -> Result<Vec<Index>> {
    // Partial indices came with Firebird 5
    let condition =
        if conn.info().major >= 5 { inline("i.rdb$condition_source") } else { "cast(null as varchar(1))".into() };
    let sql = format!(
        "select trim(i.rdb$relation_name), trim(i.rdb$index_name), i.rdb$unique_flag, i.rdb$index_type, \
                i.rdb$index_inactive, i.rdb$statistics, {expression}, {condition}, {description}, \
                trim(rc.rdb$constraint_type), trim(s.rdb$field_name) \
         from rdb$indices i \
         join rdb$relations r on r.rdb$relation_name = i.rdb$relation_name \
         left join rdb$relation_constraints rc on rc.rdb$index_name = i.rdb$index_name \
         left join rdb$index_segments s on s.rdb$index_name = i.rdb$index_name \
         where {filter} \
         order by i.rdb$relation_name, i.rdb$index_name, s.rdb$field_position",
        expression = inline("i.rdb$expression_source"),
        description = inline("i.rdb$description"),
    );
    let mut found: Vec<Index> = Vec::new();
    for row in rows(conn, &sql, params)? {
        let (Some(table), Some(name)) = (row.text(0), row.text(1)) else {
            return Err(unexpected("index"));
        };
        if found.last().is_none_or(|i| i.name != name) {
            let of_index = |column: &str| format!("select i.{column} from rdb$indices i where i.rdb$index_name = ?");
            found.push(Index {
                unique: row.flag(2),
                descending: row.int(3) == Some(1),
                inactive: row.flag(4),
                statistics: row.float(5),
                expression: whole(conn, row.lob(6), &of_index("rdb$expression_source"), &[&name])?,
                condition: whole(conn, row.lob(7), &of_index("rdb$condition_source"), &[&name])?,
                description: whole(conn, row.lob(8), &of_index("rdb$description"), &[&name])?,
                constraint: row.text(9).and_then(|kind| ConstraintKind::parse(&kind)),
                columns: Vec::new(),
                table,
                name,
            });
        }
        found.last_mut().expect("pushed above").columns.extend(row.text(10));
    }
    Ok(found)
}

/// Which triggers to read
pub enum TriggerScope<'a> {
    /// One, by its name
    Named(&'a str),
    /// Those of a table or a view
    On(&'a str),
    /// Every trigger of the user's
    All,
}

/// Triggers in the order they fire, without the ones the engine writes to
/// enforce CHECK constraints. `source` reads their bodies too, each a blob
/// of its own.
pub fn triggers(conn: &Connection, scope: TriggerScope, source: bool) -> Result<Vec<Trigger>> {
    let (filter, params): (&str, Vec<Value>) = match scope {
        TriggerScope::Named(name) => ("t.rdb$trigger_name = ?", vec![name.into()]),
        TriggerScope::On(table) => ("t.rdb$relation_name = ?", vec![table.into()]),
        TriggerScope::All => ("1 = 1", vec![]),
    };
    let sql = format!(
        "select trim(t.rdb$trigger_name), trim(t.rdb$relation_name), t.rdb$trigger_type, t.rdb$trigger_sequence, \
                t.rdb$trigger_inactive, {description}, {source} \
         from rdb$triggers t \
         where {filter} and coalesce(t.rdb$system_flag, 0) = 0 \
           and not exists (select 1 from rdb$check_constraints c where c.rdb$trigger_name = t.rdb$trigger_name) \
         order by t.rdb$relation_name, t.rdb$trigger_type, t.rdb$trigger_sequence, t.rdb$trigger_name",
        description = inline("t.rdb$description"),
        source = if source { "t.rdb$trigger_source" } else { "cast(null as varchar(1))" },
    );
    rows(conn, &sql, params)?
        .into_iter()
        .map(|row| {
            let name = row.text(0).ok_or_else(|| unexpected("trigger"))?;
            let description = whole(
                conn,
                row.lob(5),
                "select rdb$description from rdb$triggers where rdb$trigger_name = ?",
                &[&name],
            )?;
            Ok(Trigger {
                table: row.text(1),
                type_code: row.int(2).unwrap_or(0),
                position: row.int(3).unwrap_or(0),
                inactive: row.flag(4),
                source: row.text(6),
                description,
                name,
            })
        })
        .collect()
}

pub fn trigger(conn: &Connection, name: &str) -> Result<Trigger> {
    triggers(conn, TriggerScope::Named(name), true)?.pop().ok_or_else(|| not_found("trigger", name))
}

pub(crate) fn unexpected(what: &str) -> Error {
    Error::Client { message: format!("unexpected {what} row in the catalog") }
}

pub(crate) fn not_found(what: &str, name: &str) -> Error {
    Error::Client { message: format!("the {what} {name} is no longer in the database") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_types_read_as_their_declaration() {
        assert_eq!(trigger_event(1), "BEFORE INSERT");
        assert_eq!(trigger_event(2), "AFTER INSERT");
        assert_eq!(trigger_event(3), "BEFORE UPDATE");
        assert_eq!(trigger_event(4), "AFTER UPDATE");
        assert_eq!(trigger_event(5), "BEFORE DELETE");
        assert_eq!(trigger_event(6), "AFTER DELETE");
        assert_eq!(trigger_event(17), "BEFORE INSERT OR UPDATE");
        assert_eq!(trigger_event(18), "AFTER INSERT OR UPDATE");
        assert_eq!(trigger_event(113), "BEFORE INSERT OR UPDATE OR DELETE");
        assert_eq!(trigger_event(114), "AFTER INSERT OR UPDATE OR DELETE");
        assert_eq!(trigger_event(8192), "ON CONNECT");
        assert_eq!(trigger_event(8196), "ON TRANSACTION ROLLBACK");
        assert_eq!(trigger_event(0x4000 | 0b10), "BEFORE CREATE TABLE");
        assert_eq!(trigger_event(0x4000 | 0b1101), "AFTER ALTER TABLE OR DROP TABLE");
        let any = DDL_EVENTS.iter().fold(0x4000i64, |code, (bit, _)| code | 1 << bit);
        assert_eq!(trigger_event(any | 1), "AFTER ANY DDL STATEMENT");
    }
}
