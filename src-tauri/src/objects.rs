//! The object viewers' commands. A viewer is a row of tabs, and each tab one
//! of a few shapes: grids of text, SQL, plain text, or a query whose rows
//! to show. The UI draws the shapes and knows nothing of what a Firebird
//! table is made of, so another engine's objects need only say theirs.

use fb_core::Connection;
use fb_meta::{
    extract, Column, Constraint, ConstraintKind, Dependency, Grant, Index, ObjectKind, Param, Routine, Trigger, TriggerScope,
};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::changes::{self, Action};
use crate::commands::blocking;
use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    Grid,
    Sql,
    Text,
    Data,
}

/// One tab of a viewer, before its content is asked for
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TabInfo {
    id: &'static str,
    label: &'static str,
    shape: Shape,
}

const fn tab(id: &'static str, label: &'static str, shape: Shape) -> TabInfo {
    TabInfo { id, label, shape }
}

const FIELDS: TabInfo = tab("fields", "Fields", Shape::Grid);
const CONSTRAINTS: TabInfo = tab("constraints", "Constraints", Shape::Grid);
const INDICES: TabInfo = tab("indices", "Indices", Shape::Grid);
const DEPENDENCIES: TabInfo = tab("dependencies", "Dependencies", Shape::Grid);
const TRIGGERS: TabInfo = tab("triggers", "Triggers", Shape::Grid);
const DATA: TabInfo = tab("data", "Data", Shape::Data);
const DESCRIPTION: TabInfo = tab("description", "Description", Shape::Text);
const DDL: TabInfo = tab("ddl", "DDL", Shape::Sql);
const PARAMETERS: TabInfo = tab("parameters", "Parameters", Shape::Grid);
const PROPERTIES: TabInfo = tab("properties", "Properties", Shape::Grid);
const GRANTS: TabInfo = tab("grants", "Grants", Shape::Grid);

/// A kind's tabs, the first being the one a viewer opens on: IBExpert's
/// layout for tables, the source first for what is mostly source
fn tabs(kind: ObjectKind) -> &'static [TabInfo] {
    match kind {
        ObjectKind::Table => &[FIELDS, CONSTRAINTS, INDICES, DEPENDENCIES, TRIGGERS, DATA, GRANTS, DESCRIPTION, DDL],
        ObjectKind::View => &[FIELDS, DEPENDENCIES, TRIGGERS, DATA, GRANTS, DESCRIPTION, DDL],
        ObjectKind::Procedure | ObjectKind::Function => &[DDL, PARAMETERS, DEPENDENCIES, GRANTS, DESCRIPTION],
        ObjectKind::Package => &[DDL, DEPENDENCIES, GRANTS, DESCRIPTION],
        ObjectKind::Trigger | ObjectKind::Udf => &[DDL, DEPENDENCIES, DESCRIPTION],
        ObjectKind::Generator | ObjectKind::Exception => &[PROPERTIES, DEPENDENCIES, GRANTS, DESCRIPTION, DDL],
        ObjectKind::Domain | ObjectKind::Index => &[PROPERTIES, DEPENDENCIES, DESCRIPTION, DDL],
        ObjectKind::Role => &[GRANTS, PROPERTIES, DESCRIPTION, DDL],
    }
}

/// How a grid column's cells are drawn
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CellKind {
    Text,
    /// Right-aligned
    Number,
    /// A check mark when the cell is not null
    Flag,
    /// SQL, in the editor's font
    Code,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GridColumn {
    label: &'static str,
    kind: CellKind,
}

const fn text(label: &'static str) -> GridColumn {
    GridColumn { label, kind: CellKind::Text }
}

const fn number(label: &'static str) -> GridColumn {
    GridColumn { label, kind: CellKind::Number }
}

const fn flag(label: &'static str) -> GridColumn {
    GridColumn { label, kind: CellKind::Flag }
}

const fn code(label: &'static str) -> GridColumn {
    GridColumn { label, kind: CellKind::Code }
}

/// An object a grid row stands for, which opening the row opens
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectRef {
    kind: ObjectKind,
    name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GridRow {
    cells: Vec<Option<String>>,
    open: Option<ObjectRef>,
    /// What the section's row actions act on: a column's name, a
    /// constraint's
    pub(crate) key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GridSection {
    /// `None` for a tab's only grid
    title: Option<&'static str>,
    columns: Vec<GridColumn>,
    pub(crate) rows: Vec<GridRow>,
    /// What can be done here: add a field, edit the domain
    pub(crate) actions: Vec<Action>,
    /// What can be done to a row with a key
    pub(crate) row_actions: Vec<Action>,
}

impl GridSection {
    fn new(title: Option<&'static str>, columns: Vec<GridColumn>, rows: Vec<GridRow>) -> Self {
        Self { title, columns, rows, actions: Vec::new(), row_actions: Vec::new() }
    }

    /// A row's cell, to key the row by
    pub(crate) fn cell(row: &GridRow, column: usize) -> Option<String> {
        row.cells.get(column).cloned().flatten()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum TabContent {
    Grid {
        sections: Vec<GridSection>,
    },
    /// `editable`: the text can be changed and compiled back, as
    /// [`object_tab_script`] has it
    Sql {
        text: String,
        editable: bool,
    },
    Text {
        text: String,
        editable: bool,
    },
    /// A query for the viewer to run and page through, as an editor would
    Data {
        sql: String,
    },
}

fn row(cells: Vec<Option<String>>) -> GridRow {
    GridRow { cells, open: None, key: None }
}

fn opens(cells: Vec<Option<String>>, kind: ObjectKind, name: &str) -> GridRow {
    GridRow { cells, open: Some(ObjectRef { kind, name: name.to_string() }), key: None }
}

fn some(text: impl Into<String>) -> Option<String> {
    Some(text.into())
}

/// A flag cell: checked when `on`
fn mark(on: bool) -> Option<String> {
    on.then(|| "✓".to_string())
}

fn one(columns: Vec<GridColumn>, rows: Vec<GridRow>) -> TabContent {
    TabContent::Grid { sections: vec![GridSection::new(None, columns, rows)] }
}

#[tauri::command]
pub fn object_tabs(kind: ObjectKind) -> Vec<TabInfo> {
    tabs(kind).to_vec()
}

#[tauri::command]
pub async fn object_tab(
    app: AppHandle,
    id: String,
    kind: ObjectKind,
    name: String,
    tab: String,
) -> AppResult<TabContent> {
    blocking(app, move |state| {
        let conn = state.session(&id)?;
        content(&conn, kind, &name, &tab)
    })
    .await
}

/// The script that would create the picked objects again, in an order it
/// can run in
#[tauri::command]
pub async fn extract_metadata(app: AppHandle, id: String, objects: Vec<ObjectRef>) -> AppResult<String> {
    blocking(app, move |state| {
        let conn = state.session(&id)?;
        let picked: Vec<(ObjectKind, String)> = objects.into_iter().map(|o| (o.kind, o.name)).collect();
        Ok(fb_meta::ddl::script(&extract::selection(&conn, &picked)?))
    })
    .await
}

/// Whether a kind's script creates the object over the one that is there
/// (`CREATE OR ALTER`, `RECREATE`): then the script, edited, is how the
/// object is changed
fn recompiles(kind: ObjectKind) -> bool {
    matches!(
        kind,
        ObjectKind::View | ObjectKind::Procedure | ObjectKind::Function | ObjectKind::Package | ObjectKind::Trigger
    )
}

/// The script that makes an editable tab's new text the object's: for the
/// user to see, and then to run
#[tauri::command]
pub async fn object_tab_script(
    app: AppHandle,
    id: String,
    kind: ObjectKind,
    name: String,
    tab: String,
    text: String,
) -> AppResult<String> {
    blocking(app, move |state| {
        let conn = state.session(&id)?;
        let ctx = fb_meta::ddl::Context::of(conn.info());
        match tab.as_str() {
            // Already the script
            "ddl" if recompiles(kind) => Ok(text),
            "description" => Ok(fb_meta::ddl::script(&[fb_meta::change::describe(kind, &name, &text, &ctx)])),
            _ => Err(AppError::Invalid { message: format!("The {tab} tab cannot be changed.") }),
        }
    })
    .await
}

/// The script that drops an object
#[tauri::command]
pub async fn object_drop_script(app: AppHandle, id: String, kind: ObjectKind, name: String) -> AppResult<String> {
    blocking(app, move |state| {
        let conn = state.session(&id)?;
        let ctx = fb_meta::ddl::Context::of(conn.info());
        Ok(fb_meta::ddl::script(&[fb_meta::change::drop(kind, &name, &ctx)]))
    })
    .await
}

/// A script to start a new object of a kind from
#[tauri::command]
pub async fn object_template(app: AppHandle, id: String, kind: ObjectKind) -> AppResult<String> {
    blocking(app, move |state| {
        let conn = state.session(&id)?;
        Ok(fb_meta::change::template(kind, &fb_meta::ddl::Context::of(conn.info())))
    })
    .await
}

fn content(conn: &Connection, kind: ObjectKind, name: &str, tab: &str) -> AppResult<TabContent> {
    let mut content = read(conn, kind, name, tab)?;
    if let TabContent::Grid { sections } = &mut content {
        changes::attach(sections, kind, tab, conn.info().major);
    }
    Ok(content)
}

fn read(conn: &Connection, kind: ObjectKind, name: &str, tab: &str) -> AppResult<TabContent> {
    let dialect = conn.info().dialect;
    Ok(match (tab, kind) {
        ("ddl", _) => TabContent::Sql { text: extract::script(conn, kind, name)?, editable: recompiles(kind) },
        ("description", _) => {
            TabContent::Text { text: extract::description(conn, kind, name)?.unwrap_or_default(), editable: true }
        }
        ("dependencies", _) => {
            let found = fb_meta::dependencies(conn, kind, name)?;
            TabContent::Grid {
                sections: vec![dependencies("Uses", &found.uses), dependencies("Used by", &found.used_by)],
            }
        }
        ("grants", ObjectKind::Role) => {
            let held = fb_meta::grant::held_by_role(conn, name)?;
            // A privilege on a relation does not say whether it is a view
            let views: Vec<String> = if held.iter().any(|g| g.object_type <= 1) {
                fb_meta::objects(conn, ObjectKind::View)?.into_iter().map(|v| v.name).collect()
            } else {
                Vec::new()
            };
            TabContent::Grid { sections: vec![members(&fb_meta::grant::on(conn, kind, name)?), held_by(&held, &views)] }
        }
        ("grants", _) => grants(&fb_meta::grant::on(conn, kind, name)?),
        ("data", ObjectKind::Table | ObjectKind::View) => {
            TabContent::Data { sql: format!("select * from {}", fb_meta::quote(name, dialect)) }
        }
        ("fields", ObjectKind::Table) => {
            let columns: Vec<Column> = fb_meta::columns(conn, Some(name))?.into_iter().map(|(_, c)| c).collect();
            let constraints: Vec<Constraint> =
                fb_meta::constraints(conn, Some(name))?.into_iter().map(|(_, c)| c).collect();
            fields(&columns, &constraints, dialect)
        }
        ("fields", ObjectKind::View) => {
            let columns: Vec<Column> = fb_meta::columns(conn, Some(name))?.into_iter().map(|(_, c)| c).collect();
            fields(&columns, &[], dialect)
        }
        ("constraints", ObjectKind::Table) => {
            let list: Vec<Constraint> = fb_meta::constraints(conn, Some(name))?.into_iter().map(|(_, c)| c).collect();
            constraints(list)
        }
        ("indices", ObjectKind::Table) => indices(&fb_meta::indices(conn, Some(name))?),
        ("triggers", ObjectKind::Table | ObjectKind::View) => {
            triggers(&fb_meta::triggers(conn, TriggerScope::On(name), false)?)
        }
        ("parameters", ObjectKind::Procedure) => parameters(&fb_meta::procedure(conn, name)?, dialect),
        ("parameters", ObjectKind::Function) => parameters(&fb_meta::function(conn, name)?, dialect),
        ("properties", ObjectKind::Domain) => {
            let d = fb_meta::domain(conn, name)?;
            properties(vec![
                ("Type", some(d.data_type.name(dialect))),
                ("Character set", d.data_type.charset.clone()),
                ("Collation", d.data_type.collation.clone()),
                ("Not null", some(if d.not_null { "Yes" } else { "No" })),
                ("Default", d.default),
                ("Check", d.check),
            ])
        }
        ("properties", ObjectKind::Generator) => {
            let g = fb_meta::generator(conn, name)?;
            properties(vec![
                ("Value", g.value.map(|v| v.to_string())),
                ("Initial value", g.initial.map(|v| v.to_string())),
                ("Increment", g.increment.map(|v| v.to_string())),
            ])
        }
        ("properties", ObjectKind::Exception) => {
            let e = fb_meta::exception(conn, name)?;
            properties(vec![("Number", some(e.number.to_string())), ("Message", e.message)])
        }
        ("properties", ObjectKind::Role) => {
            let r = fb_meta::role(conn, name)?;
            properties(vec![("Owner", r.owner)])
        }
        ("properties", ObjectKind::Index) => {
            let i = fb_meta::index(conn, name)?;
            let table = i.table.clone();
            let mut content = properties(vec![
                ("Table", some(&i.table)),
                ("On", index_on(&i)),
                ("Unique", some(if i.unique { "Yes" } else { "No" })),
                ("Order", some(if i.descending { "Descending" } else { "Ascending" })),
                ("Active", some(if i.inactive { "No" } else { "Yes" })),
                ("Condition", i.condition.clone()),
                ("Enforces", i.constraint.map(|c| c.label().to_string())),
                ("Selectivity", i.statistics.map(selectivity)),
            ]);
            // The table's row opens the table
            if let TabContent::Grid { sections } = &mut content {
                sections[0].rows[0].open = Some(ObjectRef { kind: ObjectKind::Table, name: table });
            }
            content
        }
        _ => return Err(AppError::NotFound { message: format!("There is no {tab} tab for this object.") }),
    })
}

fn fields(columns: &[Column], constraints: &[Constraint], dialect: u32) -> TabContent {
    let in_key = |kind: ConstraintKind, column: &str| {
        constraints.iter().any(|c| c.kind == kind && c.columns.iter().any(|k| k == column))
    };
    let rows = columns
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let cells = vec![
                some((i + 1).to_string()),
                mark(in_key(ConstraintKind::PrimaryKey, &c.name)),
                mark(in_key(ConstraintKind::ForeignKey, &c.name)),
                some(&c.name),
                some(c.data_type.name(dialect)),
                c.domain.clone(),
                mark(c.required()),
                c.effective_default().map(str::to_string).or_else(|| c.identity.as_ref().map(identity)),
                c.computed.clone(),
                c.data_type.charset.clone(),
                c.data_type.collation.clone(),
                c.description.clone(),
            ];
            match &c.domain {
                Some(domain) => opens(cells, ObjectKind::Domain, domain),
                None => row(cells),
            }
        })
        .collect();
    one(
        vec![
            number("#"),
            flag("PK"),
            flag("FK"),
            text("Name"),
            text("Type"),
            text("Domain"),
            flag("Not null"),
            code("Default"),
            code("Computed by"),
            text("Character set"),
            text("Collation"),
            text("Description"),
        ],
        rows,
    )
}

fn identity(i: &fb_meta::Identity) -> String {
    if i.always { "GENERATED ALWAYS AS IDENTITY" } else { "GENERATED BY DEFAULT AS IDENTITY" }.to_string()
}

fn constraints(list: Vec<Constraint>) -> TabContent {
    let rows = list
        .into_iter()
        .map(|c| {
            let target = c.references.as_ref().map(|r| format!("{} ({})", r.table, r.columns.join(", ")));
            let rule = |rule: Option<&String>| c.references.as_ref().map(|_| rule.map_or("RESTRICT", |r| r).to_string());
            let cells = vec![
                some(&c.name),
                some(c.kind.label()),
                (!c.columns.is_empty()).then(|| c.columns.join(", ")),
                target,
                rule(c.references.as_ref().and_then(|r| r.on_update.as_ref())),
                rule(c.references.as_ref().and_then(|r| r.on_delete.as_ref())),
                c.index.clone(),
                c.check.clone(),
            ];
            match &c.references {
                Some(r) => opens(cells, ObjectKind::Table, &r.table),
                None => row(cells),
            }
        })
        .collect();
    one(
        vec![
            text("Name"),
            text("Type"),
            text("On"),
            text("References"),
            text("On update"),
            text("On delete"),
            text("Index"),
            code("Check"),
        ],
        rows,
    )
}

/// What an index is on: its columns, or its expression
fn index_on(i: &Index) -> Option<String> {
    i.expression.clone().or_else(|| (!i.columns.is_empty()).then(|| i.columns.join(", ")))
}

fn selectivity(s: f64) -> String {
    if s != 0.0 && s < 1e-4 {
        format!("{s:.3e}")
    } else {
        format!("{s:.6}")
    }
}

fn indices(list: &[Index]) -> TabContent {
    let rows = list
        .iter()
        .map(|i| {
            opens(
                vec![
                    some(&i.name),
                    index_on(i),
                    mark(i.unique),
                    mark(i.descending),
                    mark(!i.inactive),
                    i.constraint.map(|c| c.label().to_string()),
                    i.statistics.map(selectivity),
                    i.condition.clone(),
                    i.description.clone(),
                ],
                ObjectKind::Index,
                &i.name,
            )
        })
        .collect();
    one(
        vec![
            text("Name"),
            text("On"),
            flag("Unique"),
            flag("Descending"),
            flag("Active"),
            text("Enforces"),
            number("Selectivity"),
            code("Condition"),
            text("Description"),
        ],
        rows,
    )
}

fn triggers(list: &[Trigger]) -> TabContent {
    let rows = list
        .iter()
        .map(|t| {
            opens(
                vec![
                    some(&t.name),
                    some(t.event()),
                    some(t.position.to_string()),
                    mark(!t.inactive),
                    t.description.clone(),
                ],
                ObjectKind::Trigger,
                &t.name,
            )
        })
        .collect();
    one(vec![text("Name"), text("Fires"), number("Position"), flag("Active"), text("Description")], rows)
}

fn parameters(routine: &Routine, dialect: u32) -> TabContent {
    let columns = || {
        vec![
            number("#"),
            text("Name"),
            text("Type"),
            flag("Not null"),
            code("Default"),
            text("Character set"),
            text("Description"),
        ]
    };
    let rows = |list: &[Param]| -> Vec<GridRow> {
        list.iter()
            .enumerate()
            .map(|(i, p)| {
                let declared = match (&p.type_of_column, &p.domain) {
                    (Some((table, column)), _) => format!("TYPE OF COLUMN {table}.{column}"),
                    (None, Some(domain)) if p.type_of => format!("TYPE OF {domain}"),
                    (None, Some(domain)) => domain.clone(),
                    (None, None) => p.data_type.name(dialect),
                };
                let cells = vec![
                    some((i + 1).to_string()),
                    (!p.name.is_empty()).then(|| p.name.clone()),
                    some(declared),
                    mark(p.not_null),
                    p.default.clone(),
                    p.data_type.charset.clone(),
                    p.description.clone(),
                ];
                match &p.domain {
                    Some(domain) => opens(cells, ObjectKind::Domain, domain),
                    None => row(cells),
                }
            })
            .collect()
    };
    let returns = if routine.kind == fb_meta::RoutineKind::Function { "Returns" } else { "Output" };
    TabContent::Grid {
        sections: vec![
            GridSection::new(Some("Input"), columns(), rows(&routine.inputs)),
            GridSection::new(Some(returns), columns(), rows(&routine.outputs)),
        ],
    }
}

fn dependencies(title: &'static str, list: &[Dependency]) -> GridSection {
    let rows = list
        .iter()
        .map(|d| {
            let cells = vec![
                some(d.what),
                some(&d.name),
                d.detail.clone(),
                (!d.fields.is_empty()).then(|| d.fields.join(", ")),
            ];
            GridRow { cells, open: d.kind.map(|kind| ObjectRef { kind, name: d.name.clone() }), key: None }
        })
        .collect();
    GridSection::new(Some(title), vec![text("Type"), text("Name"), text("Part"), text("Columns")], rows)
}

/// A grant's row: it opens the grantee when that is an object, and is
/// keyed for what revokes it
fn granted_to(cells: Vec<Option<String>>, g: &Grant) -> GridRow {
    let open = g.grantee_kind.object().map(|kind| ObjectRef { kind, name: g.grantee.clone() });
    GridRow { cells, open, key: Some(changes::grant_key(g)) }
}

fn privilege(g: &Grant) -> String {
    match g.privilege_name() {
        Some(name) => name.to_string(),
        None if g.is_membership() => "MEMBER".to_string(),
        None => g.privilege.to_string(),
    }
}

fn grants(list: &[Grant]) -> TabContent {
    let rows = list
        .iter()
        .map(|g| {
            let cells = vec![
                some(&g.grantee),
                some(g.grantee_kind.label()),
                some(privilege(g)),
                g.column.clone(),
                mark(g.option),
                g.grantor.clone(),
            ];
            granted_to(cells, g)
        })
        .collect();
    one(
        vec![text("Grantee"), text("Kind"), text("Privilege"), text("Column"), flag("Grant option"), text("Granted by")],
        rows,
    )
}

/// Who is in a role
fn members(list: &[Grant]) -> GridSection {
    let rows = list
        .iter()
        .filter(|g| g.is_membership())
        .map(|g| {
            let cells = vec![
                some(&g.grantee),
                some(g.grantee_kind.label()),
                mark(g.option),
                mark(g.default_role),
                g.grantor.clone(),
            ];
            granted_to(cells, g)
        })
        .collect();
    GridSection::new(
        Some("Members"),
        vec![text("Member"), text("Kind"), flag("Admin option"), flag("Default"), text("Granted by")],
        rows,
    )
}

/// What a role was granted; each row opens what the privilege is on
fn held_by(list: &[Grant], views: &[String]) -> GridSection {
    let rows = list
        .iter()
        .map(|g| {
            let kind = match g.object_type {
                0 | 1 if views.contains(&g.object) => ObjectKind::View,
                0 | 1 => ObjectKind::Table,
                5 => ObjectKind::Procedure,
                15 => ObjectKind::Function,
                18 => ObjectKind::Package,
                14 => ObjectKind::Generator,
                7 => ObjectKind::Exception,
                13 => ObjectKind::Role,
                _ => return row(vec![some(&g.object), None, some(privilege(g)), g.column.clone(), mark(g.option), g.grantor.clone()]),
            };
            let cells = vec![
                some(&g.object),
                some(kind.singular()),
                some(privilege(g)),
                g.column.clone(),
                mark(g.option),
                g.grantor.clone(),
            ];
            opens(cells, kind, &g.object)
        })
        .collect();
    GridSection::new(
        Some("Privileges"),
        vec![text("On"), text("Kind"), text("Privilege"), text("Column"), flag("Grant option"), text("Granted by")],
        rows,
    )
}

fn properties(list: Vec<(&'static str, Option<String>)>) -> TabContent {
    let rows = list.into_iter().map(|(label, value)| row(vec![some(label), value])).collect();
    one(vec![text("Property"), code("Value")], rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_opens_on_a_tab_and_can_be_described() {
        for kind in ObjectKind::ALL {
            let tabs = tabs(kind);
            assert!(!tabs.is_empty(), "{kind:?}");
            assert!(tabs.contains(&DESCRIPTION) && tabs.contains(&DDL), "{kind:?}");
            let mut ids: Vec<_> = tabs.iter().map(|t| t.id).collect();
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), tabs.len(), "{kind:?} has a tab twice");
        }
    }

    /// Against the server the `FB_*` variables name; skipped without them
    #[test]
    fn every_tab_of_every_kind_reads_from_a_live_database() {
        let Some(params) = fb_core::ConnectParams::from_env() else {
            eprintln!("FB_DB not set: skipping live test");
            return;
        };
        let conn = Connection::open(&params).expect("connect");
        for kind in fb_meta::Generation::of(conn.info()).kinds() {
            let objects = fb_meta::objects(&conn, kind).unwrap();
            // The first, the middle and the last of each kind
            let mut picks = vec![0, objects.len() / 2, objects.len().saturating_sub(1)];
            picks.dedup();
            for object in picks.into_iter().filter_map(|i| objects.get(i)) {
                for tab in tabs(kind) {
                    let content = content(&conn, kind, &object.name, tab.id)
                        .unwrap_or_else(|e| panic!("{kind:?}, {} tab: {e:?}", tab.id));
                    let shape = match &content {
                        TabContent::Grid { sections } => {
                            for section in sections {
                                for row in &section.rows {
                                    assert_eq!(row.cells.len(), section.columns.len(), "{kind:?}, {} tab", tab.id);
                                }
                            }
                            Shape::Grid
                        }
                        TabContent::Sql { text, .. } => {
                            assert!(!text.trim().is_empty(), "{kind:?}: an empty script");
                            Shape::Sql
                        }
                        TabContent::Text { .. } => Shape::Text,
                        TabContent::Data { .. } => Shape::Data,
                    };
                    assert_eq!(shape, tab.shape, "{kind:?}, {} tab", tab.id);
                }
            }
        }
    }

    #[test]
    fn a_tab_serializes_by_its_shape() {
        let grid = one(vec![flag("PK")], vec![opens(vec![mark(true)], ObjectKind::Domain, "D_ID")]);
        assert_eq!(
            serde_json::to_string(&grid).unwrap(),
            r#"{"shape":"grid","sections":[{"title":null,"columns":[{"label":"PK","kind":"flag"}],"rows":[{"cells":["✓"],"open":{"kind":"domain","name":"D_ID"},"key":null}],"actions":[],"row_actions":[]}]}"#
        );
        assert_eq!(
            serde_json::to_string(&TabContent::Data { sql: "select 1".into() }).unwrap(),
            r#"{"shape":"data","sql":"select 1"}"#
        );
    }
}
