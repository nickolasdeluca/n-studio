//! What an object uses, and what uses it.
//!
//! Most of it is in `rdb$dependencies`, which the engine fills in as it
//! compiles views, PSQL, computed columns and checks. Three things are not
//! there and are read from where they live: foreign keys, the domains
//! columns are declared with, and the tables triggers belong to.
//!
//! The catalog names a computed column by its hidden domain (`RDB$<n>`) and
//! a CHECK by the trigger that enforces it; both are shown as the table
//! they belong to.

use fb_core::{Connection, Result, Value};
use serde::Serialize;

use crate::row::{rows, Row};
use crate::{Generation, ObjectKind};

/// One object on the other end of a dependency
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Dependency {
    /// The object to open for it; `None` for what the explorer does not
    /// list
    pub kind: Option<ObjectKind>,
    /// What it is, when that says more than the kind: "Computed column",
    /// "Check constraint", "Foreign key"
    pub what: &'static str,
    pub name: String,
    /// The part of `name` involved: the column, the constraint, a packaged
    /// routine
    pub detail: Option<String>,
    /// The columns involved, of whichever side is the table
    pub fields: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Dependencies {
    pub uses: Vec<Dependency>,
    pub used_by: Vec<Dependency>,
}

// `rdb$dependencies` object types
const RELATION: i64 = 0;
const VIEW: i64 = 1;
const TRIGGER: i64 = 2;
const COMPUTED: i64 = 3;
const VALIDATION: i64 = 4;
const PROCEDURE: i64 = 5;
const EXPRESSION_INDEX: i64 = 6;
const EXCEPTION: i64 = 7;
const FIELD: i64 = 9;
const INDEX: i64 = 10;
const GENERATOR: i64 = 14;
const FUNCTION: i64 = 15;
const COLLATION: i64 = 17;
const PACKAGE_HEADER: i64 = 18;
const PACKAGE_BODY: i64 = 19;

/// What a script has to create in an order: the objects that can use one
/// another. A package is two, since its header is all a caller needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Part {
    View,
    Procedure,
    Function,
    PackageHeader,
    PackageBody,
}

/// Every use of a view or a routine by another, as (the one using, the one
/// used). A relation used is given as a view, whichever it is: the caller
/// knows its views.
pub(crate) fn uses_between_routines(conn: &Connection) -> Result<Vec<((Part, String), (Part, String))>> {
    let fb3 = Generation::of(conn.info()) == Generation::Fb3;
    let package = if fb3 { "trim(d.rdb$package_name)" } else { "cast(null as char(31))" };
    let dependents = list(&[VIEW, PROCEDURE, FUNCTION, PACKAGE_HEADER, PACKAGE_BODY]);
    let targets = list(&[RELATION, VIEW, PROCEDURE, FUNCTION, PACKAGE_HEADER, PACKAGE_BODY]);
    let sql = format!(
        "select distinct trim(d.rdb$dependent_name), d.rdb$dependent_type, trim(d.rdb$depended_on_name), \
         d.rdb$depended_on_type, {package} from rdb$dependencies d \
         where d.rdb$dependent_type in ({dependents}) and d.rdb$depended_on_type in ({targets})"
    );
    let part = |code: i64| match code {
        RELATION | VIEW => Some(Part::View),
        PROCEDURE => Some(Part::Procedure),
        FUNCTION => Some(Part::Function),
        PACKAGE_HEADER => Some(Part::PackageHeader),
        PACKAGE_BODY => Some(Part::PackageBody),
        _ => None,
    };
    Ok(rows(conn, &sql, vec![])?
        .into_iter()
        .filter_map(|row| {
            let user = (part(row.int(1)?)?, row.text(0)?);
            let used = match row.text(4) {
                // A packaged routine: what is used is its package
                Some(package) => (Part::PackageHeader, package),
                None => match part(row.int(3)?)? {
                    Part::PackageBody => (Part::PackageHeader, row.text(2)?),
                    part => (part, row.text(2)?),
                },
            };
            Some((user, used))
        })
        .collect())
}

/// The types a kind is depended on as, and the types it depends as
fn codes(kind: ObjectKind) -> (&'static [i64], &'static [i64]) {
    match kind {
        ObjectKind::Table => (&[RELATION, VIEW], &[]),
        ObjectKind::View => (&[RELATION, VIEW], &[RELATION, VIEW]),
        ObjectKind::Trigger => (&[TRIGGER], &[TRIGGER]),
        ObjectKind::Procedure => (&[PROCEDURE], &[PROCEDURE]),
        ObjectKind::Function => (&[FUNCTION], &[FUNCTION]),
        ObjectKind::Udf => (&[FUNCTION], &[]),
        ObjectKind::Package => (&[PACKAGE_HEADER, PACKAGE_BODY], &[PACKAGE_HEADER, PACKAGE_BODY]),
        ObjectKind::Exception => (&[EXCEPTION], &[]),
        ObjectKind::Domain => (&[FIELD], &[COMPUTED, VALIDATION, FIELD]),
        ObjectKind::Index => (&[INDEX], &[EXPRESSION_INDEX]),
        ObjectKind::Generator => (&[GENERATOR], &[]),
        ObjectKind::Role => (&[], &[]),
    }
}

fn list(codes: &[i64]) -> String {
    codes.iter().map(i64::to_string).collect::<Vec<_>>().join(", ")
}

/// Collects dependencies, merging the rows that differ only by column
#[derive(Default)]
struct Found(Vec<Dependency>);

impl Found {
    fn add(
        &mut self,
        kind: Option<ObjectKind>,
        what: &'static str,
        name: String,
        detail: Option<String>,
        field: Option<String>,
    ) {
        let at = self.0.iter().position(|d| d.what == what && d.name == name && d.detail == detail);
        let at = at.unwrap_or_else(|| {
            self.0.push(Dependency { kind, what, name, detail, fields: Vec::new() });
            self.0.len() - 1
        });
        let fields = &mut self.0[at].fields;
        if let Some(field) = field.filter(|f| !fields.contains(f)) {
            fields.push(field);
        }
    }

    fn done(mut self) -> Vec<Dependency> {
        self.0.sort_by(|a, b| (a.what, &a.name, &a.detail).cmp(&(b.what, &b.name, &b.detail)));
        self.0
    }
}

pub fn dependencies(conn: &Connection, kind: ObjectKind, name: &str) -> Result<Dependencies> {
    let fb3 = Generation::of(conn.info()) == Generation::Fb3;
    let (as_target, as_dependent) = codes(kind);
    let mut uses = Found::default();
    let mut used_by = Found::default();

    // What it uses
    let mut filters = Vec::new();
    let mut params: Vec<Value> = Vec::new();
    if !as_dependent.is_empty() {
        filters.push(format!("d.rdb$dependent_name = ? and d.rdb$dependent_type in ({})", list(as_dependent)));
        params.push(name.into());
    }
    if kind == ObjectKind::Table {
        // Its computed columns and its checks depend in its place
        filters.push(format!(
            "d.rdb$dependent_type = {COMPUTED} and d.rdb$dependent_name in \
             (select rf.rdb$field_source from rdb$relation_fields rf where rf.rdb$relation_name = ?)"
        ));
        filters.push(format!(
            "d.rdb$dependent_type = {TRIGGER} and d.rdb$dependent_name in \
             (select cc.rdb$trigger_name from rdb$check_constraints cc \
              join rdb$relation_constraints rc on rc.rdb$constraint_name = cc.rdb$constraint_name \
              where rc.rdb$relation_name = ?)"
        ));
        params.extend([name.into(), name.into()]);
    }
    if !filters.is_empty() {
        let (package, legacy) = if fb3 {
            (
                "trim(d.rdb$package_name)",
                "(select first 1 fn.rdb$legacy_flag from rdb$functions fn \
                  where fn.rdb$function_name = d.rdb$depended_on_name and fn.rdb$package_name is null)",
            )
        } else {
            ("cast(null as char(1))", "1")
        };
        let sql = format!(
            "select d.rdb$depended_on_type, trim(d.rdb$depended_on_name), trim(d.rdb$field_name), \
                    case when rr.rdb$view_blr is null then 0 else 1 end, {package}, {legacy} \
             from rdb$dependencies d \
             left join rdb$relations rr on rr.rdb$relation_name = d.rdb$depended_on_name \
                  and d.rdb$depended_on_type in ({RELATION}, {VIEW}) \
             where ({})",
            filters.join(") or (")
        );
        for row in rows(conn, &sql, params)? {
            let Some(target) = row.text(1) else { continue };
            // A table's own columns, in its computed columns and checks
            if kind == ObjectKind::Table && target == name && matches!(row.int(0), Some(RELATION | VIEW)) {
                continue;
            }
            let (target_kind, what) = match row.int(0).unwrap_or(-1) {
                RELATION | VIEW if row.flag(3) => (Some(ObjectKind::View), "View"),
                RELATION | VIEW => (Some(ObjectKind::Table), "Table"),
                TRIGGER => (Some(ObjectKind::Trigger), "Trigger"),
                PROCEDURE => (Some(ObjectKind::Procedure), "Procedure"),
                EXCEPTION => (Some(ObjectKind::Exception), "Exception"),
                FIELD => (Some(ObjectKind::Domain), "Domain"),
                INDEX => (Some(ObjectKind::Index), "Index"),
                GENERATOR => (Some(ObjectKind::Generator), "Generator"),
                FUNCTION if row.flag(5) => (Some(ObjectKind::Udf), "UDF"),
                FUNCTION => (Some(ObjectKind::Function), "Function"),
                // Every text column has one: noise
                COLLATION => continue,
                PACKAGE_HEADER | PACKAGE_BODY => (Some(ObjectKind::Package), "Package"),
                _ => (None, "Other"),
            };
            match row.text(4) {
                // A packaged routine: the package is the object
                Some(package) => uses.add(Some(ObjectKind::Package), "Package", package, Some(target), None),
                None => uses.add(target_kind, what, target, None, row.text(2)),
            }
        }
    }
    if kind == ObjectKind::Table {
        table_uses(conn, name, &mut uses)?;
    }
    if kind == ObjectKind::Trigger || kind == ObjectKind::Index {
        let sql = match kind {
            ObjectKind::Trigger => {
                "select trim(o.rdb$relation_name), case when r.rdb$view_blr is null then 0 else 1 end \
                 from rdb$triggers o join rdb$relations r on r.rdb$relation_name = o.rdb$relation_name \
                 where o.rdb$trigger_name = ?"
            }
            _ => "select trim(o.rdb$relation_name), 0 from rdb$indices o where o.rdb$index_name = ?",
        };
        for row in rows(conn, sql, vec![name.into()])? {
            if let Some(table) = row.text(0) {
                let (kind, what) = if row.flag(1) { (ObjectKind::View, "View") } else { (ObjectKind::Table, "Table") };
                uses.add(Some(kind), what, table, None, None);
            }
        }
    }

    // What uses it
    if !as_target.is_empty() {
        let (filter, package) = match (kind, fb3) {
            (ObjectKind::Package, _) => ("d.rdb$package_name = ?".to_string(), "trim(d.rdb$depended_on_name)"),
            (_, true) => (
                format!(
                    "d.rdb$depended_on_name = ? and d.rdb$depended_on_type in ({}) and d.rdb$package_name is null",
                    list(as_target)
                ),
                "cast(null as char(1))",
            ),
            (_, false) => (
                format!("d.rdb$depended_on_name = ? and d.rdb$depended_on_type in ({})", list(as_target)),
                "cast(null as char(1))",
            ),
        };
        let sql = format!(
            "select d.rdb$dependent_type, trim(d.rdb$dependent_name), trim(d.rdb$field_name), \
                    trim(t.rdb$relation_name), trim(cc.rdb$constraint_name), \
                    trim(rf.rdb$relation_name), trim(rf.rdb$field_name), {package} \
             from rdb$dependencies d \
             left join rdb$triggers t on d.rdb$dependent_type = {TRIGGER} and t.rdb$trigger_name = d.rdb$dependent_name \
             left join rdb$check_constraints cc on cc.rdb$trigger_name = t.rdb$trigger_name \
             left join rdb$relation_fields rf on d.rdb$dependent_type = {COMPUTED} \
                  and rf.rdb$field_source = d.rdb$dependent_name \
             where {filter}"
        );
        for row in rows(conn, &sql, vec![name.into()])? {
            add_dependent(&mut used_by, kind, name, &row);
        }
    }
    match kind {
        ObjectKind::Table => {
            // Foreign keys pointing here
            let sql = "select trim(rc.rdb$relation_name), trim(rc.rdb$constraint_name) \
                       from rdb$ref_constraints ref \
                       join rdb$relation_constraints rc on rc.rdb$constraint_name = ref.rdb$constraint_name \
                       join rdb$relation_constraints uq on uq.rdb$constraint_name = ref.rdb$const_name_uq \
                       where uq.rdb$relation_name = ?";
            for row in rows(conn, sql, vec![name.into()])? {
                if let Some(table) = row.text(0) {
                    used_by.add(Some(ObjectKind::Table), "Foreign key", table, row.text(1), None);
                }
            }
        }
        ObjectKind::Domain => {
            // Columns declared with it; a view's column inherits its
            // table's, and is not one
            let sql = "select trim(rf.rdb$relation_name), trim(rf.rdb$field_name) \
                       from rdb$relation_fields rf \
                       join rdb$relations r on r.rdb$relation_name = rf.rdb$relation_name \
                       where rf.rdb$field_source = ? and r.rdb$view_blr is null";
            for row in rows(conn, sql, vec![name.into()])? {
                if let Some(table) = row.text(0) {
                    used_by.add(Some(ObjectKind::Table), "Column", table, None, row.text(1));
                }
            }
        }
        ObjectKind::Generator if fb3 => {
            let sql = "select trim(rf.rdb$relation_name), trim(rf.rdb$field_name) \
                       from rdb$relation_fields rf where rf.rdb$generator_name = ?";
            for row in rows(conn, sql, vec![name.into()])? {
                if let Some(table) = row.text(0) {
                    used_by.add(Some(ObjectKind::Table), "Identity column", table, None, row.text(1));
                }
            }
        }
        _ => {}
    }

    Ok(Dependencies { uses: uses.done(), used_by: used_by.done() })
}

/// One row of what depends on `name`
fn add_dependent(found: &mut Found, kind: ObjectKind, name: &str, row: &Row) {
    let Some(dependent) = row.text(1) else { return };
    // For a table, the column of it that is used; for a package, the
    // routine of it
    let field = row.text(2);
    let routine = row.text(7);
    match row.int(0).unwrap_or(-1) {
        TRIGGER => match (row.text(3), row.text(4)) {
            // A table's own checks are part of it
            (Some(table), Some(_)) if kind == ObjectKind::Table && table == name => {}
            (Some(table), Some(check)) => {
                found.add(Some(ObjectKind::Table), "Check constraint", table, Some(check), field)
            }
            (table, _) => found.add(Some(ObjectKind::Trigger), "Trigger", dependent, table.or(routine), field),
        },
        COMPUTED => match (row.text(5), row.text(6)) {
            (Some(table), _) if kind == ObjectKind::Table && table == name => {}
            (Some(table), column) => found.add(Some(ObjectKind::Table), "Computed column", table, column, field),
            (None, _) => found.add(Some(ObjectKind::Domain), "Domain", dependent, routine, field),
        },
        RELATION | VIEW => found.add(Some(ObjectKind::View), "View", dependent, routine, field),
        PROCEDURE => found.add(Some(ObjectKind::Procedure), "Procedure", dependent, routine, field),
        FUNCTION => found.add(Some(ObjectKind::Function), "Function", dependent, routine, field),
        EXPRESSION_INDEX | INDEX => found.add(Some(ObjectKind::Index), "Index", dependent, routine, field),
        VALIDATION | FIELD => found.add(Some(ObjectKind::Domain), "Domain", dependent, routine, field),
        PACKAGE_HEADER | PACKAGE_BODY => found.add(Some(ObjectKind::Package), "Package", dependent, routine, field),
        _ => found.add(None, "Other", dependent, routine, field),
    }
}

/// What a table uses that `rdb$dependencies` does not say: the tables its
/// foreign keys point to, and its columns' domains
fn table_uses(conn: &Connection, name: &str, uses: &mut Found) -> Result<()> {
    let keys = "select trim(uq.rdb$relation_name), trim(rc.rdb$constraint_name) \
                from rdb$relation_constraints rc \
                join rdb$ref_constraints ref on ref.rdb$constraint_name = rc.rdb$constraint_name \
                join rdb$relation_constraints uq on uq.rdb$constraint_name = ref.rdb$const_name_uq \
                where rc.rdb$relation_name = ?";
    for row in rows(conn, keys, vec![name.into()])? {
        if let Some(table) = row.text(0) {
            uses.add(Some(ObjectKind::Table), "Foreign key", table, row.text(1), None);
        }
    }
    let domains = "select trim(rf.rdb$field_source), trim(rf.rdb$field_name) from rdb$relation_fields rf \
                   where rf.rdb$relation_name = ? and rf.rdb$field_source not starting with 'RDB$'";
    for row in rows(conn, domains, vec![name.into()])? {
        if let Some(domain) = row.text(0) {
            uses.add(Some(ObjectKind::Domain), "Domain", domain, None, row.text(1));
        }
    }
    Ok(())
}
