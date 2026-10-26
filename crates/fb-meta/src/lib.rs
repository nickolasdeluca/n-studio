//! What a database holds, read from its system tables.
//!
//! The catalog changed shape with Firebird 3 (ODS 12): stored functions and
//! packages appeared, UDFs moved to sit beside functions in
//! `rdb$functions`, and packaged routines share their tables with standalone
//! ones. Every query here is picked by [`Generation`], so the rest of the
//! application asks for an [`ObjectKind`] and never sees the difference.
//!
//! Counts for the whole tree come back in one query, since every round trip
//! costs a full network latency on a remote server.

use fb_core::{Connection, Error, Result, ServerInfo, Value};
use serde::{Deserialize, Serialize};

/// The kinds of object the explorer shows, in its order
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    Domain,
    Table,
    View,
    Procedure,
    /// Stored (PSQL) functions, Firebird 3 and later
    Function,
    /// Firebird 3 and later
    Package,
    Trigger,
    Generator,
    Exception,
    /// External functions declared with DECLARE EXTERNAL FUNCTION
    Udf,
    Role,
    Index,
}

impl ObjectKind {
    pub const ALL: [ObjectKind; 12] = [
        ObjectKind::Domain,
        ObjectKind::Table,
        ObjectKind::View,
        ObjectKind::Procedure,
        ObjectKind::Function,
        ObjectKind::Package,
        ObjectKind::Trigger,
        ObjectKind::Generator,
        ObjectKind::Exception,
        ObjectKind::Udf,
        ObjectKind::Role,
        ObjectKind::Index,
    ];

    /// The explorer's heading for the kind, as IBExpert names it
    pub fn label(self) -> &'static str {
        match self {
            ObjectKind::Domain => "Domains",
            ObjectKind::Table => "Tables",
            ObjectKind::View => "Views",
            ObjectKind::Procedure => "Procedures",
            ObjectKind::Function => "Functions",
            ObjectKind::Package => "Packages",
            ObjectKind::Trigger => "Triggers",
            ObjectKind::Generator => "Generators",
            ObjectKind::Exception => "Exceptions",
            ObjectKind::Udf => "UDFs",
            ObjectKind::Role => "Roles",
            ObjectKind::Index => "Indices",
        }
    }
}

/// Which catalog layout a database has
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generation {
    /// Firebird 2.x: ODS 11
    Fb2,
    /// Firebird 3 and later: ODS 12 and up
    Fb3,
}

impl Generation {
    pub fn of(info: &ServerInfo) -> Self {
        if info.ods_major >= 12 {
            Generation::Fb3
        } else {
            Generation::Fb2
        }
    }

    /// The kinds this generation has, in explorer order
    pub fn kinds(self) -> impl Iterator<Item = ObjectKind> {
        ObjectKind::ALL.into_iter().filter(move |k| self.source(*k).is_some())
    }

    fn source(self, kind: ObjectKind) -> Option<Source> {
        use Generation::*;
        use ObjectKind::*;
        // Every user object has a null or zero system flag; Firebird 3 also
        // flags the sequences behind identity columns.
        const USER: &str = "coalesce(o.rdb$system_flag, 0) = 0";
        let source = |table, name, parent, filter: &str| Source {
            table,
            name,
            parent,
            filter: if filter.is_empty() { USER.to_string() } else { format!("{USER} and {filter}") },
        };
        Some(match (kind, self) {
            // Columns declared without a domain get one of their own, named
            // RDB$<n>.
            (Domain, _) => source(
                "rdb$fields",
                "rdb$field_name",
                None,
                "o.rdb$field_name not starting with 'RDB$'",
            ),
            (Table, _) => source("rdb$relations", "rdb$relation_name", None, "o.rdb$view_blr is null"),
            (View, _) => source("rdb$relations", "rdb$relation_name", None, "o.rdb$view_blr is not null"),
            (Procedure, Fb2) => source("rdb$procedures", "rdb$procedure_name", None, ""),
            (Procedure, Fb3) => source("rdb$procedures", "rdb$procedure_name", None, "o.rdb$package_name is null"),
            (Function, Fb2) | (Package, Fb2) => return None,
            (Function, Fb3) => source(
                "rdb$functions",
                "rdb$function_name",
                None,
                "o.rdb$package_name is null and coalesce(o.rdb$legacy_flag, 0) = 0",
            ),
            (Package, Fb3) => source("rdb$packages", "rdb$package_name", None, ""),
            // CHECK constraints are enforced by triggers the engine writes
            // and flags as user objects.
            (Trigger, _) => source(
                "rdb$triggers",
                "rdb$trigger_name",
                Some("rdb$relation_name"),
                "not exists (select 1 from rdb$check_constraints c \
                             where c.rdb$trigger_name = o.rdb$trigger_name)",
            ),
            (Generator, _) => source("rdb$generators", "rdb$generator_name", None, ""),
            (Exception, _) => source("rdb$exceptions", "rdb$exception_name", None, ""),
            (Udf, Fb2) => source("rdb$functions", "rdb$function_name", None, ""),
            (Udf, Fb3) => source("rdb$functions", "rdb$function_name", None, "o.rdb$legacy_flag = 1"),
            (Role, _) => source("rdb$roles", "rdb$role_name", None, ""),
            (Index, _) => source("rdb$indices", "rdb$index_name", Some("rdb$relation_name"), ""),
        })
    }
}

struct Source {
    table: &'static str,
    name: &'static str,
    /// The table a trigger or an index belongs to
    parent: Option<&'static str>,
    filter: String,
}

/// One object, as the explorer lists it
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DbObject {
    pub name: String,
    /// The table it belongs to, for triggers and indices
    pub parent: Option<String>,
    /// The start of its COMMENT ON text
    pub description: Option<String>,
}

/// How much of a description the object list shows. Fetching whole blobs
/// would cost a round trip per object.
const DESCRIPTION_CHARS: u32 = 255;

/// The number of objects of each kind, for every kind the database has, in
/// explorer order. One query.
pub fn counts(conn: &Connection) -> Result<Vec<(ObjectKind, u64)>> {
    let generation = Generation::of(conn.info());
    let kinds: Vec<ObjectKind> = generation.kinds().collect();
    let sql = kinds
        .iter()
        .enumerate()
        .map(|(i, kind)| {
            let s = generation.source(*kind).expect("listed kinds have a source");
            format!("select {i}, count(*) from {} o where {}", s.table, s.filter)
        })
        .collect::<Vec<_>>()
        .join(" union all ");
    let result = conn.query(&sql, vec![])?;

    let mut counts: Vec<(ObjectKind, u64)> = kinds.iter().map(|k| (*k, 0)).collect();
    for row in result.rows {
        let int = |i: usize| row.get(i).and_then(Value::as_i64);
        let (Some(i), Some(n)) = (int(0), int(1)) else {
            return Err(Error::Client { message: format!("unexpected count row {row:?}") });
        };
        if let Some(entry) = counts.get_mut(i as usize) {
            entry.1 = n as u64;
        }
    }
    Ok(counts)
}

/// The objects of one kind, by name. Empty for a kind the database's
/// generation does not have.
pub fn objects(conn: &Connection, kind: ObjectKind) -> Result<Vec<DbObject>> {
    let Some(s) = Generation::of(conn.info()).source(kind) else {
        return Ok(Vec::new());
    };
    let parent = match s.parent {
        Some(col) => format!("trim(o.{col})"),
        None => "cast(null as char(1))".into(),
    };
    // A blob cast to a string comes back inline with the row. The metadata
    // character set keeps it from failing on a database whose default
    // cannot hold the text.
    let sql = format!(
        "select trim(o.{name}), {parent}, \
                cast(substring(o.rdb$description from 1 for {DESCRIPTION_CHARS}) \
                     as varchar({DESCRIPTION_CHARS}) character set unicode_fss) \
         from {table} o where {filter} order by 1",
        name = s.name,
        table = s.table,
        filter = s.filter,
    );
    let text = |v: Option<&Value>| v.and_then(Value::as_str).map(str::to_owned);
    conn.query(&sql, vec![])?
        .rows
        .into_iter()
        .map(|row| {
            let name = text(row.first())
                .ok_or_else(|| Error::Client { message: format!("unexpected object row {row:?}") })?;
            Ok(DbObject {
                name,
                parent: text(row.get(1)).filter(|p| !p.is_empty()),
                description: text(row.get(2)).filter(|d| !d.trim().is_empty()),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firebird_2_has_no_functions_or_packages() {
        let fb2: Vec<_> = Generation::Fb2.kinds().collect();
        assert!(!fb2.contains(&ObjectKind::Function));
        assert!(!fb2.contains(&ObjectKind::Package));
        assert!(fb2.contains(&ObjectKind::Udf));
        assert_eq!(Generation::Fb3.kinds().count(), ObjectKind::ALL.len());
    }

    #[test]
    fn generation_follows_the_ods() {
        let info = |ods_major| ServerInfo { ods_major, ..Default::default() };
        assert_eq!(Generation::of(&info(11)), Generation::Fb2);
        assert_eq!(Generation::of(&info(12)), Generation::Fb3);
        assert_eq!(Generation::of(&info(13)), Generation::Fb3);
    }
}
