//! A SELECT put together from parts picked in a builder: tables, how they
//! join, the columns to show, group, sort and filter by. The builder holds
//! the parts; this writes the statement, so names are quoted and clauses
//! ordered the way every other statement here is.

use serde::Deserialize;

use crate::ident::quote;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct QueryTable {
    pub name: String,
    /// What the statement calls it: the name itself, unless the table is
    /// there twice
    pub alias: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinKind {
    Inner,
    Left,
    Right,
    Full,
}

/// Two tables, by alias, and the columns that match between them
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct QueryJoin {
    pub kind: JoinKind,
    pub left: String,
    pub right: String,
    /// Pairs of a column of `left` and a column of `right`
    pub on: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Aggregate {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sort {
    Asc,
    Desc,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct QueryColumn {
    /// Its table's alias
    pub table: String,
    pub column: String,
    /// The name it is shown under, when not its own
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub aggregate: Option<Aggregate>,
    #[serde(default)]
    pub sort: Option<Sort>,
    /// What it must satisfy, as it is written after the column: `> 10`,
    /// `LIKE 'A%'`, `IS NULL`
    #[serde(default)]
    pub filter: Option<String>,
    /// In the result; a column can be there only to sort or filter by
    pub output: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct QuerySpec {
    pub tables: Vec<QueryTable>,
    pub joins: Vec<QueryJoin>,
    pub columns: Vec<QueryColumn>,
    pub distinct: bool,
    /// Only the first rows, this many
    pub first: Option<u32>,
}

fn filled(text: &Option<String>) -> Option<&str> {
    text.as_deref().map(str::trim).filter(|t| !t.is_empty())
}

/// The statement the parts make. Empty without a table.
pub fn select(spec: &QuerySpec, dialect: u32) -> String {
    let q = |name: &str| quote(name, dialect);
    let Some(first) = spec.tables.first() else { return String::new() };
    // One table needs no qualifiers, which read better left out
    let single = spec.tables.len() == 1;
    let expression = |c: &QueryColumn| {
        let column = if single { q(&c.column) } else { format!("{}.{}", q(&c.table), q(&c.column)) };
        match c.aggregate {
            Some(Aggregate::Count) => format!("COUNT({column})"),
            Some(Aggregate::Sum) => format!("SUM({column})"),
            Some(Aggregate::Avg) => format!("AVG({column})"),
            Some(Aggregate::Min) => format!("MIN({column})"),
            Some(Aggregate::Max) => format!("MAX({column})"),
            None => column,
        }
    };

    let mut sql = String::from("SELECT");
    if let Some(n) = spec.first {
        sql.push_str(&format!(" FIRST {n}"));
    }
    if spec.distinct {
        sql.push_str(" DISTINCT");
    }
    let shown: Vec<String> = spec
        .columns
        .iter()
        .filter(|c| c.output)
        .map(|c| match filled(&c.alias) {
            Some(alias) => format!("{} AS {}", expression(c), q(alias)),
            None => expression(c),
        })
        .collect();
    if shown.is_empty() {
        sql.push_str(" *");
    } else {
        sql.push_str("\n    ");
        sql.push_str(&shown.join(",\n    "));
    }

    let named = |t: &QueryTable| if t.alias == t.name { q(&t.name) } else { format!("{} {}", q(&t.name), q(&t.alias)) };
    sql.push_str(&format!("\nFROM {}", named(first)));
    let mut placed: Vec<&str> = vec![&first.alias];
    for table in &spec.tables[1..] {
        // Every join between it and the tables before it, as one
        let mut kind = None;
        let mut conditions = Vec::new();
        for join in &spec.joins {
            let (other, flipped) = if join.right == table.alias && placed.contains(&join.left.as_str()) {
                (&join.left, false)
            } else if join.left == table.alias && placed.contains(&join.right.as_str()) {
                (&join.right, true)
            } else {
                continue;
            };
            let _ = other;
            // Written from the other side, a left join is a right one
            kind.get_or_insert(match (join.kind, flipped) {
                (JoinKind::Left, true) => JoinKind::Right,
                (JoinKind::Right, true) => JoinKind::Left,
                (k, _) => k,
            });
            for (l, r) in &join.on {
                conditions.push(format!("{}.{} = {}.{}", q(&join.left), q(l), q(&join.right), q(r)));
            }
        }
        match kind {
            Some(kind) if !conditions.is_empty() => {
                let word = match kind {
                    JoinKind::Inner => "JOIN",
                    JoinKind::Left => "LEFT JOIN",
                    JoinKind::Right => "RIGHT JOIN",
                    JoinKind::Full => "FULL JOIN",
                };
                sql.push_str(&format!("\n{word} {} ON {}", named(table), conditions.join(" AND ")));
            }
            // Nothing ties it to the others: every row with every row
            _ => sql.push_str(&format!("\nCROSS JOIN {}", named(table))),
        }
        placed.push(&table.alias);
    }

    let conditions = |aggregated: bool| -> Vec<String> {
        spec.columns
            .iter()
            .filter(|c| c.aggregate.is_some() == aggregated)
            .filter_map(|c| filled(&c.filter).map(|f| format!("{} {f}", expression(c))))
            .collect()
    };
    let wheres = conditions(false);
    if !wheres.is_empty() {
        sql.push_str(&format!("\nWHERE {}", wheres.join("\n  AND ")));
    }
    if spec.columns.iter().any(|c| c.aggregate.is_some()) {
        let mut groups: Vec<String> = Vec::new();
        for c in spec.columns.iter().filter(|c| c.aggregate.is_none() && (c.output || c.sort.is_some())) {
            let e = expression(c);
            if !groups.contains(&e) {
                groups.push(e);
            }
        }
        if !groups.is_empty() {
            sql.push_str(&format!("\nGROUP BY {}", groups.join(", ")));
        }
        let having = conditions(true);
        if !having.is_empty() {
            sql.push_str(&format!("\nHAVING {}", having.join("\n   AND ")));
        }
    }
    let sorts: Vec<String> = spec
        .columns
        .iter()
        .filter_map(|c| c.sort.map(|s| format!("{}{}", expression(c), if s == Sort::Desc { " DESC" } else { "" })))
        .collect();
    if !sorts.is_empty() {
        sql.push_str(&format!("\nORDER BY {}", sorts.join(", ")));
    }
    sql.push('\n');
    sql
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(name: &str) -> QueryTable {
        QueryTable { name: name.into(), alias: name.into() }
    }

    fn column(table: &str, column: &str) -> QueryColumn {
        QueryColumn { table: table.into(), column: column.into(), alias: None, aggregate: None, sort: None, filter: None, output: true }
    }

    fn join(kind: JoinKind, left: &str, l: &str, right: &str, r: &str) -> QueryJoin {
        QueryJoin { kind, left: left.into(), right: right.into(), on: vec![(l.into(), r.into())] }
    }

    #[test]
    fn one_table_reads_plainly() {
        assert_eq!(select(&QuerySpec::default(), 3), "");
        let all = QuerySpec { tables: vec![table("PEDIDO")], ..Default::default() };
        assert_eq!(select(&all, 3), "SELECT *\nFROM PEDIDO\n");
        let some = QuerySpec {
            tables: vec![table("PEDIDO")],
            columns: vec![
                column("PEDIDO", "ID"),
                QueryColumn { alias: Some("quem".into()), sort: Some(Sort::Desc), ..column("PEDIDO", "CLIENTE") },
                QueryColumn { output: false, filter: Some(">= '2024-01-01' ".into()), ..column("PEDIDO", "EMISSAO") },
            ],
            distinct: true,
            first: Some(10),
            ..Default::default()
        };
        assert_eq!(
            select(&some, 3),
            "SELECT FIRST 10 DISTINCT\n    ID,\n    CLIENTE AS \"quem\"\nFROM PEDIDO\nWHERE EMISSAO >= '2024-01-01'\nORDER BY CLIENTE DESC\n"
        );
    }

    #[test]
    fn tables_join_to_the_ones_before_them() {
        let spec = QuerySpec {
            tables: vec![table("PEDIDO"), table("PEDIDO_ITEM"), QueryTable { name: "PEDIDO".into(), alias: "P2".into() }, table("TIPOS")],
            joins: vec![
                join(JoinKind::Left, "PEDIDO_ITEM", "PEDIDO", "PEDIDO", "ID"),
                join(JoinKind::Inner, "P2", "ID", "PEDIDO", "ID"),
            ],
            columns: vec![column("PEDIDO", "ID"), column("PEDIDO_ITEM", "ITEM")],
            ..Default::default()
        };
        assert_eq!(
            select(&spec, 3),
            "SELECT\n    PEDIDO.ID,\n    PEDIDO_ITEM.ITEM\nFROM PEDIDO\n\
             RIGHT JOIN PEDIDO_ITEM ON PEDIDO_ITEM.PEDIDO = PEDIDO.ID\n\
             JOIN PEDIDO P2 ON P2.ID = PEDIDO.ID\n\
             CROSS JOIN TIPOS\n"
        );
    }

    #[test]
    fn aggregates_group_by_the_rest_and_filter_after() {
        let spec = QuerySpec {
            tables: vec![table("PEDIDO_ITEM")],
            columns: vec![
                column("PEDIDO_ITEM", "PEDIDO"),
                QueryColumn {
                    aggregate: Some(Aggregate::Sum),
                    alias: Some("TOTAL".into()),
                    filter: Some("> 100".into()),
                    sort: Some(Sort::Asc),
                    ..column("PEDIDO_ITEM", "QTD")
                },
                QueryColumn { output: false, filter: Some("= 1".into()), ..column("PEDIDO_ITEM", "PAGO") },
            ],
            ..Default::default()
        };
        assert_eq!(
            select(&spec, 3),
            "SELECT\n    PEDIDO,\n    SUM(QTD) AS TOTAL\nFROM PEDIDO_ITEM\nWHERE PAGO = 1\nGROUP BY PEDIDO\nHAVING SUM(QTD) > 100\nORDER BY SUM(QTD)\n"
        );
    }

    #[test]
    fn names_that_need_quotes_get_them() {
        let spec = QuerySpec {
            tables: vec![table("Order"), table("B")],
            joins: vec![join(JoinKind::Inner, "Order", "id", "B", "DATE")],
            columns: vec![column("Order", "id")],
            ..Default::default()
        };
        assert_eq!(select(&spec, 3), "SELECT\n    \"Order\".\"id\"\nFROM \"Order\"\nJOIN B ON \"Order\".\"id\" = B.\"DATE\"\n");
    }
}
