//! The tables of a database and the foreign keys between them, as a diagram
//! draws them: each table with its columns, each reference from the columns
//! that hold it to the key it points at.
//!
//! A database of a thousand tables is not a picture, so a diagram starts
//! from the tables it is asked for and takes in what they reference and
//! what references them, as far as it is told to; each table says which of
//! its neighbours were left out, for the picture to grow from there.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use fb_core::{Connection, Result, Value};
use serde::Serialize;

use crate::row::rows;
use crate::types::{type_joins, DataType, TYPE_COLUMNS};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagramColumn {
    pub name: String,
    /// As declared: `VARCHAR(60)`
    pub type_name: String,
    /// The domain it is declared with, when that is a domain of the user's
    pub domain: Option<String>,
    pub primary_key: bool,
    /// It is part of a reference to another table
    pub foreign_key: bool,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagramTable {
    pub name: String,
    pub columns: Vec<DiagramColumn>,
    /// The tables it references or is referenced by that the diagram does
    /// not show
    pub hidden: Vec<String>,
}

/// A foreign key: `from` holds it, `to` has the key it points at
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagramReference {
    pub name: String,
    pub from: String,
    pub from_columns: Vec<String>,
    pub to: String,
    pub to_columns: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagram {
    pub tables: Vec<DiagramTable>,
    /// Those between two tables of the diagram
    pub references: Vec<DiagramReference>,
    /// Every table of the database, to add one by name
    pub all: Vec<String>,
}

/// More tables than this are not drawn at once: the picture says nothing,
/// and laying it out takes long
pub const MOST: usize = 250;

fn references(conn: &Connection) -> Result<Vec<DiagramReference>> {
    let sql = "select trim(rc.rdb$constraint_name), trim(rc.rdb$relation_name), trim(s.rdb$field_name), \
                      trim(pk.rdb$relation_name), trim(ps.rdb$field_name) \
               from rdb$relation_constraints rc \
               join rdb$ref_constraints ref on ref.rdb$constraint_name = rc.rdb$constraint_name \
               join rdb$relation_constraints pk on pk.rdb$constraint_name = ref.rdb$const_name_uq \
               join rdb$index_segments s on s.rdb$index_name = rc.rdb$index_name \
               join rdb$index_segments ps on ps.rdb$index_name = pk.rdb$index_name \
                    and ps.rdb$field_position = s.rdb$field_position \
               where rc.rdb$constraint_type = 'FOREIGN KEY' \
               order by rc.rdb$relation_name, rc.rdb$constraint_name, s.rdb$field_position";
    let mut list: Vec<DiagramReference> = Vec::new();
    for row in rows(conn, sql, vec![])? {
        let (Some(name), Some(from), Some(column), Some(to), Some(target)) =
            (row.text(0), row.text(1), row.text(2), row.text(3), row.text(4))
        else {
            continue;
        };
        match list.last_mut() {
            Some(last) if last.name == name => {
                last.from_columns.push(column);
                last.to_columns.push(target);
            }
            _ => list.push(DiagramReference { name, from, from_columns: vec![column], to, to_columns: vec![target] }),
        }
    }
    Ok(list)
}

/// The tables to show: `seeds`, and what is within `depth` references of
/// them, nearest first, until there are [`MOST`]. Every table when there
/// are no seeds.
fn pick(all: &[String], references: &[DiagramReference], seeds: &[String], depth: u32) -> Vec<String> {
    if seeds.is_empty() {
        return all.iter().take(MOST).cloned().collect();
    }
    let known: HashSet<&str> = all.iter().map(String::as_str).collect();
    let mut neighbours: HashMap<&str, BTreeSet<&str>> = HashMap::new();
    for r in references {
        neighbours.entry(&r.from).or_default().insert(&r.to);
        neighbours.entry(&r.to).or_default().insert(&r.from);
    }
    let mut picked: Vec<String> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut frontier: Vec<&str> = Vec::new();
    for seed in seeds {
        if known.contains(seed.as_str()) && seen.insert(seed) {
            frontier.push(seed);
        }
    }
    for level in 0..=depth {
        let mut next = Vec::new();
        for table in frontier {
            if picked.len() == MOST {
                return picked;
            }
            picked.push(table.to_string());
            if level < depth {
                for near in neighbours.get(table).into_iter().flatten() {
                    if seen.insert(near) {
                        next.push(*near);
                    }
                }
            }
        }
        frontier = next;
    }
    picked
}

fn columns(conn: &Connection, tables: &[String], dialect: u32) -> Result<BTreeMap<String, Vec<DiagramColumn>>> {
    // Past a few hundred names the list costs more than reading them all
    let listed = tables.len() <= 200;
    let filter = if listed {
        format!("rf.rdb$relation_name in ({})", vec!["?"; tables.len()].join(", "))
    } else {
        "coalesce(r.rdb$system_flag, 0) = 0 and r.rdb$view_blr is null".to_string()
    };
    let params: Vec<Value> = if listed { tables.iter().map(|t| t.as_str().into()).collect() } else { Vec::new() };
    let sql = format!(
        "select trim(rf.rdb$relation_name), trim(rf.rdb$field_name), trim(rf.rdb$field_source), \
                rf.rdb$null_flag, f.rdb$null_flag, {TYPE_COLUMNS} \
         from rdb$relation_fields rf \
         join rdb$relations r on r.rdb$relation_name = rf.rdb$relation_name \
         join rdb$fields f on f.rdb$field_name = rf.rdb$field_source \
         {joins} \
         where {filter} \
         order by rf.rdb$relation_name, rf.rdb$field_position",
        joins = type_joins("coalesce(rf.rdb$collation_id, f.rdb$collation_id)"),
    );
    let mut by_table: BTreeMap<String, Vec<DiagramColumn>> = BTreeMap::new();
    for row in rows(conn, &sql, params)? {
        let (Some(table), Some(name), Some(source)) = (row.text(0), row.text(1), row.text(2)) else { continue };
        let data_type = DataType::read(&row, 5);
        by_table.entry(table).or_default().push(DiagramColumn {
            name,
            type_name: data_type.name(dialect),
            domain: (!source.starts_with("RDB$")).then_some(source),
            primary_key: false,
            foreign_key: false,
            required: row.flag(3) || row.flag(4),
        });
    }
    Ok(by_table)
}

fn primary_keys(conn: &Connection) -> Result<HashSet<(String, String)>> {
    let sql = "select trim(rc.rdb$relation_name), trim(s.rdb$field_name) \
               from rdb$relation_constraints rc \
               join rdb$index_segments s on s.rdb$index_name = rc.rdb$index_name \
               where rc.rdb$constraint_type = 'PRIMARY KEY'";
    Ok(rows(conn, sql, vec![])?.iter().filter_map(|r| Some((r.text(0)?, r.text(1)?))).collect())
}

/// The names of the relations nobody but the user made: the tables, and
/// the views too when `views` says so
fn relations(conn: &Connection, views: bool) -> Result<Vec<String>> {
    let only = if views { "" } else { " and r.rdb$view_blr is null" };
    let sql = format!(
        "select trim(r.rdb$relation_name) from rdb$relations r where coalesce(r.rdb$system_flag, 0) = 0{only} order by 1"
    );
    Ok(rows(conn, &sql, vec![])?.iter().filter_map(|r| r.text(0)).collect())
}

/// The diagram of `seeds` and what is within `depth` references of them,
/// or of the whole database when there are no seeds
pub fn diagram(conn: &Connection, seeds: &[String], depth: u32) -> Result<Diagram> {
    let all = relations(conn, false)?;
    let every = references(conn)?;
    let picked = pick(&all, &every, seeds, depth);
    described(conn, all, every, picked)
}

/// What a query can read from: the tables and views named, in the order
/// they are named, with the references between them. `all` names every
/// table and view; none named, that is all it says.
pub fn sources(conn: &Connection, names: &[String]) -> Result<Diagram> {
    let all = relations(conn, true)?;
    let mut picked: Vec<String> = Vec::new();
    for name in names {
        if all.contains(name) && !picked.contains(name) {
            picked.push(name.clone());
        }
    }
    described(conn, all, references(conn)?, picked)
}

fn described(conn: &Connection, all: Vec<String>, every: Vec<DiagramReference>, picked: Vec<String>) -> Result<Diagram> {
    if picked.is_empty() {
        return Ok(Diagram { all, ..Default::default() });
    }
    let shown: HashSet<&str> = picked.iter().map(String::as_str).collect();
    let keys = primary_keys(conn)?;
    let mut columns = columns(conn, &picked, conn.info().dialect)?;

    let mut tables = Vec::with_capacity(picked.len());
    for name in &picked {
        let mut columns = columns.remove(name).unwrap_or_default();
        let mut hidden = BTreeSet::new();
        let mut referencing: HashSet<&str> = HashSet::new();
        for r in &every {
            if &r.from == name {
                referencing.extend(r.from_columns.iter().map(String::as_str));
                if !shown.contains(r.to.as_str()) {
                    hidden.insert(r.to.clone());
                }
            }
            if &r.to == name && !shown.contains(r.from.as_str()) {
                hidden.insert(r.from.clone());
            }
        }
        for column in &mut columns {
            column.foreign_key = referencing.contains(column.name.as_str());
            column.primary_key = keys.contains(&(name.clone(), column.name.clone()));
        }
        tables.push(DiagramTable { name: name.clone(), columns, hidden: hidden.into_iter().collect() });
    }
    let references =
        every.into_iter().filter(|r| shown.contains(r.from.as_str()) && shown.contains(r.to.as_str())).collect();
    Ok(Diagram { tables, references, all })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(from: &str, to: &str) -> DiagramReference {
        DiagramReference {
            name: format!("FK_{from}_{to}"),
            from: from.into(),
            from_columns: vec!["X".into()],
            to: to.into(),
            to_columns: vec!["ID".into()],
        }
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_diagram_grows_from_its_seeds_by_references_either_way() {
        let all = names(&["A", "B", "C", "D", "E"]);
        let references = [reference("A", "B"), reference("C", "B"), reference("D", "C")];
        assert_eq!(pick(&all, &references, &names(&["B"]), 0), ["B"]);
        assert_eq!(pick(&all, &references, &names(&["B"]), 1), ["B", "A", "C"]);
        assert_eq!(pick(&all, &references, &names(&["B"]), 2), ["B", "A", "C", "D"]);
        // A table that is not there is not a seed; no seeds is every table
        assert_eq!(pick(&all, &references, &names(&["NOPE", "E"]), 3), ["E"]);
        assert_eq!(pick(&all, &references, &[], 0), all);
    }

    #[test]
    fn a_diagram_stops_at_what_can_be_drawn() {
        let all: Vec<String> = (0..MOST + 50).map(|i| format!("T{i:04}")).collect();
        let references: Vec<_> = all.iter().skip(1).map(|t| reference(t, "T0000")).collect();
        assert_eq!(pick(&all, &references, &names(&["T0000"]), 1).len(), MOST);
        assert_eq!(pick(&all, &references, &[], 0).len(), MOST);
    }
}
