//! An object's DDL, read from a live database.

use std::collections::HashSet;

use fb_core::{Connection, Result};

use crate::ddl::{self, Context, Statement};
use crate::table::TriggerScope;
use crate::{object, routine, table, ObjectKind};

/// The statements that would create one object again. A table comes with
/// its constraints, indices and triggers; a generator with its value.
pub fn statements(conn: &Connection, kind: ObjectKind, name: &str) -> Result<Vec<Statement>> {
    let ctx = Context::of(conn.info());
    Ok(match kind {
        ObjectKind::Table => {
            let mut statements = ddl::table(&table::table(conn, name)?, &ctx);
            for trigger in table::triggers(conn, TriggerScope::On(name), true)? {
                statements.extend(ddl::trigger(&trigger, &ctx));
            }
            statements
        }
        ObjectKind::View => {
            let mut statements = ddl::view(&table::view(conn, name)?, &ctx);
            for trigger in table::triggers(conn, TriggerScope::On(name), true)? {
                statements.extend(ddl::trigger(&trigger, &ctx));
            }
            statements
        }
        ObjectKind::Trigger => ddl::trigger(&table::trigger(conn, name)?, &ctx),
        ObjectKind::Index => ddl::index(&table::index(conn, name)?, &ctx),
        ObjectKind::Procedure => ddl::routine(&routine::procedure(conn, name)?, &ctx),
        ObjectKind::Function => ddl::routine(&routine::function(conn, name)?, &ctx),
        ObjectKind::Package => ddl::package(&routine::package(conn, name)?, &ctx),
        ObjectKind::Udf => ddl::udf(&routine::udf(conn, name)?, &ctx),
        ObjectKind::Domain => ddl::domain(&object::domain(conn, name)?, &ctx),
        ObjectKind::Generator => ddl::generator(&object::generator(conn, name)?, &ctx),
        ObjectKind::Exception => ddl::exception(&object::exception(conn, name)?, &ctx),
        ObjectKind::Role => ddl::role(&object::role(conn, name)?, &ctx),
    })
}

/// One object's DDL as a script
pub fn script(conn: &Connection, kind: ObjectKind, name: &str) -> Result<String> {
    Ok(ddl::script(&statements(conn, kind, name)?))
}

/// Past this many tables of a selection, the whole catalog is read at once
/// instead of table by table: a handful of queries, not a handful each
const ONE_BY_ONE: usize = 12;

/// The statements that would create a selection of objects again, in an
/// order they can run in: what tables are made of first, then the tables
/// and the foreign keys between them, the views (each after the views it
/// selects from), the routines, and the triggers and indices last. What a
/// table brings with it is not repeated when it was selected too.
///
/// Routines come in the order given: one that calls another declared after
/// it needs that one to exist first, which only the reader can arrange.
pub fn selection(conn: &Connection, objects: &[(ObjectKind, String)]) -> Result<Vec<Statement>> {
    let ctx = Context::of(conn.info());
    let of = |kind: ObjectKind| -> Vec<&str> {
        objects.iter().filter(|(k, _)| *k == kind).map(|(_, name)| name.as_str()).collect()
    };
    let mut out = Vec::new();
    for kind in [ObjectKind::Domain, ObjectKind::Generator, ObjectKind::Exception, ObjectKind::Udf, ObjectKind::Role]
    {
        for name in of(kind) {
            out.extend(statements(conn, kind, name)?);
        }
    }

    let names = of(ObjectKind::Table);
    let tables = if names.len() > ONE_BY_ONE {
        let wanted: HashSet<&str> = names.iter().copied().collect();
        table::tables(conn, None)?.into_iter().filter(|t| wanted.contains(t.name.as_str())).collect()
    } else {
        names.iter().map(|name| table::table(conn, name)).collect::<Result<Vec<_>>>()?
    };
    let mut foreign_keys = Vec::new();
    for t in &tables {
        let (keys, rest): (Vec<_>, Vec<_>) = ddl::table(t, &ctx).into_iter().partition(|s| s.foreign_key);
        out.extend(rest);
        foreign_keys.extend(keys);
    }
    out.extend(foreign_keys);

    let views = of(ObjectKind::View);
    for name in view_order(conn, &views)? {
        out.extend(ddl::view(&table::view(conn, name)?, &ctx));
    }
    for kind in [ObjectKind::Function, ObjectKind::Procedure, ObjectKind::Package] {
        for name in of(kind) {
            out.extend(statements(conn, kind, name)?);
        }
    }

    // Triggers can call anything above, so they wait for it
    let relations: Vec<&str> = names.iter().chain(&views).copied().collect();
    if relations.len() > ONE_BY_ONE {
        let wanted: HashSet<&str> = relations.iter().copied().collect();
        for trigger in table::triggers(conn, TriggerScope::All, true)? {
            if trigger.table.as_deref().is_some_and(|t| wanted.contains(t)) {
                out.extend(ddl::trigger(&trigger, &ctx));
            }
        }
    } else {
        for name in relations {
            for trigger in table::triggers(conn, TriggerScope::On(name), true)? {
                out.extend(ddl::trigger(&trigger, &ctx));
            }
        }
    }
    for kind in [ObjectKind::Trigger, ObjectKind::Index] {
        for name in of(kind) {
            out.extend(statements(conn, kind, name)?);
        }
    }

    // A trigger or an index selected along with its table came with it
    let mut seen = HashSet::new();
    out.retain(|s| seen.insert(s.sql.clone()));
    Ok(out)
}

/// The views with each one after those of them it selects from
fn view_order<'a>(conn: &Connection, views: &[&'a str]) -> Result<Vec<&'a str>> {
    let mut uses = Vec::with_capacity(views.len());
    for &view in views {
        let found = crate::dependencies(conn, ObjectKind::View, view)?;
        let on: Vec<usize> = found
            .uses
            .iter()
            .filter(|d| d.kind == Some(ObjectKind::View))
            .filter_map(|d| views.iter().position(|v| *v == d.name))
            .collect();
        uses.push(on);
    }
    Ok(ordered(&uses).into_iter().map(|i| views[i]).collect())
}

/// `0..uses.len()` with each index after the ones it uses, otherwise in
/// order. A cycle, which views cannot have, is cut where it closes.
fn ordered(uses: &[Vec<usize>]) -> Vec<usize> {
    fn visit(i: usize, uses: &[Vec<usize>], seen: &mut [bool], out: &mut Vec<usize>) {
        if std::mem::replace(&mut seen[i], true) {
            return;
        }
        for &on in &uses[i] {
            visit(on, uses, seen, out);
        }
        out.push(i);
    }
    let mut seen = vec![false; uses.len()];
    let mut out = Vec::with_capacity(uses.len());
    for i in 0..uses.len() {
        visit(i, uses, &mut seen, &mut out);
    }
    out
}

/// An object's whole COMMENT ON text; the object list has only its start
pub fn description(conn: &Connection, kind: ObjectKind, name: &str) -> Result<Option<String>> {
    let Some(source) = crate::Generation::of(conn.info()).source(kind) else {
        return Ok(None);
    };
    // Packaged routines share their names' table with standalone ones, so
    // the kind's own filter stays
    let sql = format!(
        "select o.rdb$description from {} o where o.{} = ? and {}",
        source.table, source.name, source.filter
    );
    Ok(crate::row::rows(conn, &sql, vec![name.into()])?.first().and_then(|r| r.text(0)))
}

#[cfg(test)]
mod tests {
    use super::ordered;

    #[test]
    fn what_is_used_comes_first() {
        assert_eq!(ordered(&[vec![], vec![], vec![]]), [0, 1, 2]);
        assert_eq!(ordered(&[vec![2], vec![0], vec![]]), [2, 0, 1]);
        // A cycle still lists each once
        assert_eq!(ordered(&[vec![1], vec![0]]), [1, 0]);
    }
}
