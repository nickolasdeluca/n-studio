//! An object's DDL, read from a live database.

use std::collections::{HashMap, HashSet};

use fb_core::{Connection, Result};

use crate::ddl::{self, Context, Statement};
use crate::table::TriggerScope;
use crate::deps::{self, Part};
use crate::{grant, object, routine, table, ObjectKind};

/// The statements that would create one object again. A table comes with
/// its constraints, indices and triggers; a generator with its value. What
/// was granted on it comes last.
pub fn statements(conn: &Connection, kind: ObjectKind, name: &str) -> Result<Vec<Statement>> {
    let mut statements = object_statements(conn, kind, name)?;
    statements.extend(grant::statements(&grant::on(conn, kind, name)?, &Context::of(conn.info())));
    Ok(statements)
}

/// An object without its grants
fn object_statements(conn: &Connection, kind: ObjectKind, name: &str) -> Result<Vec<Statement>> {
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
/// selects from), the routines, the triggers and indices, and what was
/// granted on any of them last: a grant to a procedure needs the procedure.
/// What a table brings with it is not repeated when it was selected too.
///
/// Views and routines come each after the ones it uses; routines that
/// call each other are declared with empty bodies first, then defined.
pub fn selection(conn: &Connection, objects: &[(ObjectKind, String)]) -> Result<Vec<Statement>> {
    let ctx = Context::of(conn.info());
    let of = |kind: ObjectKind| -> Vec<&str> {
        objects.iter().filter(|(k, _)| *k == kind).map(|(_, name)| name.as_str()).collect()
    };
    let mut out = Vec::new();
    for kind in [ObjectKind::Domain, ObjectKind::Generator, ObjectKind::Exception, ObjectKind::Udf, ObjectKind::Role]
    {
        for name in of(kind) {
            out.extend(object_statements(conn, kind, name)?);
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

    // Views and routines use one another: each goes after what it uses
    let views = of(ObjectKind::View);
    let mut parts: Vec<Piece> = Vec::new();
    for &name in &views {
        parts.push(Piece::new(Part::View, name, ddl::view(&table::view(conn, name)?, &ctx)));
    }
    for name in of(ObjectKind::Function) {
        let function = routine::function(conn, name)?;
        let stub = ddl::routine_stub(&function, &ctx);
        parts.push(Piece { stub, ..Piece::new(Part::Function, name, ddl::routine(&function, &ctx)) });
    }
    for name in of(ObjectKind::Procedure) {
        let procedure = routine::procedure(conn, name)?;
        let stub = ddl::routine_stub(&procedure, &ctx);
        parts.push(Piece { stub, ..Piece::new(Part::Procedure, name, ddl::routine(&procedure, &ctx)) });
    }
    for name in of(ObjectKind::Package) {
        let (body, header): (Vec<_>, Vec<_>) = ddl::package(&routine::package(conn, name)?, &ctx)
            .into_iter()
            .partition(|s| s.sql.starts_with("RECREATE PACKAGE BODY"));
        parts.push(Piece::new(Part::PackageHeader, name, header));
        if !body.is_empty() {
            parts.push(Piece::new(Part::PackageBody, name, body));
        }
    }
    let uses = if parts.len() > 1 { uses_among(&parts, deps::uses_between_routines(conn)?) } else { vec![vec![]; parts.len()] };
    for group in components(&uses) {
        // Routines that call each other: all declared, then all defined
        let declared: Vec<usize> =
            if group.len() > 1 { group.iter().copied().filter(|i| parts[*i].stub.is_some()).collect() } else { Vec::new() };
        out.extend(declared.iter().filter_map(|i| parts[*i].stub.clone()));
        let within: Vec<Vec<usize>> = group
            .iter()
            .map(|i| {
                let on = uses[*i].iter().filter(|on| !declared.contains(on));
                on.filter_map(|on| group.iter().position(|member| member == on)).collect()
            })
            .collect();
        for i in ordered(&within) {
            out.extend(parts[group[i]].statements.iter().cloned());
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
            out.extend(object_statements(conn, kind, name)?);
        }
    }

    let granted: Vec<&(ObjectKind, String)> =
        objects.iter().filter(|(kind, _)| !grant::object_types(*kind).is_empty()).collect();
    if granted.len() > ONE_BY_ONE {
        let wanted: HashSet<(i64, &str)> = granted
            .iter()
            .flat_map(|(kind, name)| grant::object_types(*kind).iter().map(move |t| (*t, name.as_str())))
            .collect();
        let grants: Vec<_> =
            grant::all(conn)?.into_iter().filter(|g| wanted.contains(&(g.object_type, g.object.as_str()))).collect();
        out.extend(grant::statements(&grants, &ctx));
    } else {
        for (kind, name) in granted {
            out.extend(grant::statements(&grant::on(conn, *kind, name)?, &ctx));
        }
    }

    // A trigger or an index selected along with its table came with it
    let mut seen = HashSet::new();
    out.retain(|s| seen.insert(s.sql.clone()));
    Ok(out)
}

/// A view, a routine or half a package, with the statements that create it
struct Piece {
    part: Part,
    name: String,
    statements: Vec<Statement>,
    /// What declares it before it is defined, when it can be
    stub: Option<Statement>,
}

impl Piece {
    fn new(part: Part, name: &str, statements: Vec<Statement>) -> Self {
        Self { part, name: name.to_string(), statements, stub: None }
    }
}

/// For each piece, the pieces it uses, from the catalog's uses. A package
/// body uses its header, which the catalog does not say.
fn uses_among(parts: &[Piece], between: Vec<((Part, String), (Part, String))>) -> Vec<Vec<usize>> {
    let index: HashMap<(Part, &str), usize> =
        parts.iter().enumerate().map(|(i, p)| ((p.part, p.name.as_str()), i)).collect();
    let mut uses = vec![Vec::new(); parts.len()];
    for ((user, user_name), (used, used_name)) in &between {
        let (Some(&i), Some(&on)) = (index.get(&(*user, user_name.as_str())), index.get(&(*used, used_name.as_str()))) else {
            continue;
        };
        // One that calls itself needs nothing before it
        if i != on && !uses[i].contains(&on) {
            uses[i].push(on);
        }
    }
    for (i, piece) in parts.iter().enumerate() {
        if piece.part == Part::PackageBody {
            uses[i].extend(index.get(&(Part::PackageHeader, piece.name.as_str())));
        }
    }
    for on in &mut uses {
        on.sort_unstable();
    }
    uses
}

/// The indices grouped so that each group comes after the groups it uses:
/// a group of several is a cycle, each of them using the others. Groups
/// keep the order given wherever nothing says otherwise. (Tarjan's
/// strongly connected components.)
fn components(uses: &[Vec<usize>]) -> Vec<Vec<usize>> {
    struct Walk<'a> {
        uses: &'a [Vec<usize>],
        /// When each was reached, and the earliest reached from it
        reached: Vec<Option<(usize, usize)>>,
        stack: Vec<usize>,
        on_stack: Vec<bool>,
        clock: usize,
        out: Vec<Vec<usize>>,
    }
    fn visit(i: usize, w: &mut Walk) {
        w.reached[i] = Some((w.clock, w.clock));
        w.clock += 1;
        w.stack.push(i);
        w.on_stack[i] = true;
        for &on in &w.uses[i] {
            let low = match w.reached[on] {
                None => {
                    visit(on, w);
                    w.reached[on].map(|(_, low)| low)
                }
                Some((at, _)) if w.on_stack[on] => Some(at),
                Some(_) => None,
            };
            if let (Some(low), Some((_, mine))) = (low, w.reached[i].as_mut()) {
                *mine = (*mine).min(low);
            }
        }
        if w.reached[i].is_some_and(|(at, low)| at == low) {
            let mut group = Vec::new();
            while let Some(member) = w.stack.pop() {
                w.on_stack[member] = false;
                group.push(member);
                if member == i {
                    break;
                }
            }
            group.sort_unstable();
            w.out.push(group);
        }
    }
    let n = uses.len();
    let mut walk = Walk { uses, reached: vec![None; n], stack: Vec::new(), on_stack: vec![false; n], clock: 0, out: Vec::new() };
    for i in 0..n {
        if walk.reached[i].is_none() {
            visit(i, &mut walk);
        }
    }
    walk.out
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
    use super::{components, ordered};

    #[test]
    fn groups_come_after_what_they_use_and_cycles_stay_together() {
        // Nothing uses anything: the order given
        assert_eq!(components(&[vec![], vec![], vec![]]), [[0], [1], [2]]);
        // 0 uses 2, 1 uses 0
        assert_eq!(components(&[vec![2], vec![0], vec![]]), [[2], [0], [1]]);
        // 1 and 2 call each other; 0 uses 1, 3 is used by 2
        assert_eq!(components(&[vec![1], vec![2], vec![1, 3], vec![]]), [vec![3], vec![1, 2], vec![0]]);
    }

    #[test]
    fn what_is_used_comes_first() {
        assert_eq!(ordered(&[vec![], vec![], vec![]]), [0, 1, 2]);
        assert_eq!(ordered(&[vec![2], vec![0], vec![]]), [2, 0, 1]);
        // A cycle still lists each once
        assert_eq!(ordered(&[vec![1], vec![0]]), [1, 0]);
    }
}
