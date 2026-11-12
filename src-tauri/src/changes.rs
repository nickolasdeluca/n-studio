//! What can be done to an object from its viewer: each action a form to
//! fill (or none), and the script that carries it out. The UI draws the
//! forms and shows the scripts; which statements change a Firebird column
//! is known only here and in `fb_meta::change`.

use std::collections::HashMap;

use fb_core::Connection;
use fb_meta::change::{self, ColumnChange, DomainChange, NewColumn, NewConstraint, NewIndex};
use fb_meta::ddl::{script, Context, Statement};
use fb_meta::ObjectKind;
use serde::Serialize;
use tauri::AppHandle;

use crate::commands::blocking;
use crate::error::{AppError, AppResult};
use crate::objects::GridSection;

/// Something to do to an object, or to a part of it
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Action {
    id: &'static str,
    label: &'static str,
    /// It asks for values first: see [`object_action_form`]
    form: bool,
    /// It destroys something
    danger: bool,
}

const fn asks(id: &'static str, label: &'static str) -> Action {
    Action { id, label, form: true, danger: false }
}

const fn does(id: &'static str, label: &'static str) -> Action {
    Action { id, label, form: false, danger: false }
}

const fn destroys(id: &'static str, label: &'static str) -> Action {
    Action { id, label, form: false, danger: true }
}

const INDEX_ACTIONS: [Action; 3] = [
    does("index_activate", "Activate (Rebuild)"),
    does("index_deactivate", "Deactivate"),
    does("index_statistics", "Recompute Selectivity"),
];

/// Gives a tab's grids their actions, and their rows the keys those act on
pub fn attach(sections: &mut [GridSection], kind: ObjectKind, tab: &str) {
    let Some(section) = sections.first_mut() else { return };
    // The column of the grid that names what a row is
    let mut key = None;
    match (tab, kind) {
        ("fields", ObjectKind::Table) => {
            section.actions = vec![asks("add_field", "Add Field…")];
            section.row_actions = vec![asks("edit_field", "Edit Field…"), destroys("drop_field", "Drop Field…")];
            key = Some(3);
        }
        ("constraints", ObjectKind::Table) => {
            section.actions = vec![
                asks("add_primary_key", "Add Primary Key…"),
                asks("add_foreign_key", "Add Foreign Key…"),
                asks("add_unique", "Add Unique…"),
                asks("add_check", "Add Check…"),
            ];
            section.row_actions = vec![destroys("drop_constraint", "Drop Constraint…")];
            key = Some(0);
        }
        ("indices", ObjectKind::Table) => {
            section.actions = vec![asks("add_index", "Add Index…")];
            section.row_actions = INDEX_ACTIONS.into_iter().chain([destroys("drop_index", "Drop Index…")]).collect();
            key = Some(0);
        }
        ("triggers", ObjectKind::Table | ObjectKind::View) => {
            section.row_actions = vec![
                does("trigger_activate", "Activate"),
                does("trigger_deactivate", "Deactivate"),
                destroys("drop_trigger", "Drop Trigger…"),
            ];
            key = Some(0);
        }
        ("properties", ObjectKind::Domain) => section.actions = vec![asks("edit_domain", "Edit Domain…")],
        ("properties", ObjectKind::Generator) => section.actions = vec![asks("set_generator", "Set Value…")],
        ("properties", ObjectKind::Exception) => section.actions = vec![asks("edit_exception", "Edit Message…")],
        ("properties", ObjectKind::Index) => section.actions = INDEX_ACTIONS.to_vec(),
        _ => {}
    }
    if let Some(column) = key {
        for row in &mut section.rows {
            row.key = GridSection::cell(row, column);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    Text,
    /// SQL, in the editor's font
    Code,
    Number,
    /// "true" or "false"
    Check,
    /// One of `options`
    Select,
    /// Any text, with `options` to pick from
    Combo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FormField {
    id: &'static str,
    label: &'static str,
    kind: FieldKind,
    value: String,
    options: Vec<String>,
    required: bool,
    hint: Option<&'static str>,
}

fn field(id: &'static str, label: &'static str, kind: FieldKind) -> FormField {
    FormField { id, label, kind, value: String::new(), options: Vec::new(), required: false, hint: None }
}

impl FormField {
    fn value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self
    }

    fn options(mut self, options: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.options = options.into_iter().map(Into::into).collect();
        self
    }

    fn required(mut self) -> Self {
        self.required = true;
        self
    }

    fn hint(mut self, hint: &'static str) -> Self {
        self.hint = Some(hint);
        self
    }
}

/// What an action asks before it can be a script
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Form {
    title: String,
    fields: Vec<FormField>,
}

const COLUMNS_HINT: &str = "Separated by commas.";
const RULES: [&str; 4] = ["RESTRICT", "CASCADE", "SET NULL", "SET DEFAULT"];

/// The types a column is usually declared with, then the user's domains
fn types(conn: &Connection) -> AppResult<Vec<String>> {
    let mut types: Vec<String> = [
        "INTEGER",
        "BIGINT",
        "SMALLINT",
        "NUMERIC(18,2)",
        "DECIMAL(18,4)",
        "DOUBLE PRECISION",
        "VARCHAR(60)",
        "CHAR(1)",
        "DATE",
        "TIME",
        "TIMESTAMP",
        "BLOB SUB_TYPE TEXT",
        "BLOB SUB_TYPE BINARY",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    if conn.info().major >= 3 {
        types.push("BOOLEAN".into());
    }
    types.extend(fb_meta::objects(conn, ObjectKind::Domain)?.into_iter().map(|d| d.name));
    Ok(types)
}

fn flag(on: bool) -> &'static str {
    if on {
        "true"
    } else {
        "false"
    }
}

/// A default without its keyword, as the forms show and take it
fn bare_default(default: Option<&str>) -> String {
    let text = default.unwrap_or_default().trim();
    match text.get(..7) {
        Some(keyword) if keyword.eq_ignore_ascii_case("DEFAULT") => text[7..].trim().to_string(),
        _ => text.to_string(),
    }
}

fn column(conn: &Connection, table: &str, name: &str) -> AppResult<(usize, fb_meta::Column)> {
    fb_meta::columns(conn, Some(table))?
        .into_iter()
        .map(|(_, c)| c)
        .enumerate()
        .find(|(_, c)| c.name == name)
        .ok_or_else(|| AppError::NotFound { message: format!("{table} has no column {name}.") })
}

/// A column's type as it would be declared: its domain, or the type itself
fn declared(c: &fb_meta::Column, ctx: &Context) -> String {
    c.domain.clone().unwrap_or_else(|| c.data_type.declaration(ctx.dialect, &ctx.charset))
}

fn key_of(key: &Option<String>) -> AppResult<&str> {
    key.as_deref().ok_or_else(|| AppError::Invalid { message: "This action needs a row to act on.".into() })
}

/// The form an action fills before it runs, with what is there now
#[tauri::command]
pub async fn object_action_form(
    app: AppHandle,
    id: String,
    kind: ObjectKind,
    name: String,
    action: String,
    key: Option<String>,
) -> AppResult<Form> {
    blocking(app, move |state| form(&*state.session(&id)?, kind, &name, &action, &key)).await
}

fn form(conn: &Connection, kind: ObjectKind, name: &str, action: &str, key: &Option<String>) -> AppResult<Form> {
    use FieldKind::*;
    let ctx = Context::of(conn.info());
    let name_field = || field("name", "Name", Text);
    let fields = match (action, kind) {
        ("add_field", ObjectKind::Table) => vec![
            name_field().required(),
            field("type", "Type or domain", Combo).options(types(conn)?).hint("Not needed for a computed column."),
            field("not_null", "Not null", Check).value("false"),
            field("default", "Default", Code),
            field("computed", "Computed by", Code).hint("An expression makes this a computed column."),
            field("description", "Description", Text),
        ],
        ("edit_field", ObjectKind::Table) => {
            let (position, c) = column(conn, name, key_of(key)?)?;
            let mut fields = vec![name_field().value(&c.name).required()];
            // A computed column's type and default are its expression's
            if c.computed.is_none() {
                fields.push(field("type", "Type or domain", Combo).value(declared(&c, &ctx)).options(types(conn)?));
                fields.push(
                    field("not_null", "Not null", Check)
                        .value(flag(c.not_null))
                        .hint("Changing it needs Firebird 3 or later."),
                );
                fields.push(field("default", "Default", Code).value(bare_default(c.default.as_deref())));
            }
            fields.push(field("position", "Position", Number).value((position + 1).to_string()));
            fields.push(field("description", "Description", Text).value(c.description.unwrap_or_default()));
            fields
        }
        ("add_primary_key" | "add_unique", ObjectKind::Table) => vec![
            name_field().hint("Left empty, the server names it."),
            field("columns", "Columns", Text).required().hint(COLUMNS_HINT),
        ],
        ("add_foreign_key", ObjectKind::Table) => vec![
            name_field().hint("Left empty, the server names it."),
            field("columns", "Columns", Text).required().hint(COLUMNS_HINT),
            field("table", "References table", Combo)
                .required()
                .options(fb_meta::objects(conn, ObjectKind::Table)?.into_iter().map(|t| t.name)),
            field("references", "Referenced columns", Text).hint("Left empty, the table's primary key."),
            field("on_update", "On update", Select).value("RESTRICT").options(RULES),
            field("on_delete", "On delete", Select).value("RESTRICT").options(RULES),
        ],
        ("add_check", ObjectKind::Table) => vec![
            name_field().hint("Left empty, the server names it."),
            field("condition", "Condition", Code).required(),
        ],
        ("add_index", ObjectKind::Table) => vec![
            name_field().required(),
            field("columns", "Columns", Text).hint(COLUMNS_HINT),
            field("expression", "Or computed by", Code).hint("An expression instead of columns."),
            field("unique", "Unique", Check).value("false"),
            field("descending", "Descending", Check).value("false"),
        ],
        ("edit_domain", ObjectKind::Domain) => {
            let d = fb_meta::domain(conn, name)?;
            vec![
                name_field().value(&d.name).required(),
                field("type", "Type", Combo)
                    .value(d.data_type.declaration(ctx.dialect, &ctx.charset))
                    .options(types(conn)?.into_iter().filter(|t| t != &d.name)),
                field("not_null", "Not null", Check)
                    .value(flag(d.not_null))
                    .hint("Changing it needs Firebird 3 or later."),
                field("default", "Default", Code).value(bare_default(d.default.as_deref())),
                field("check", "Check", Code).value(d.check.unwrap_or_default()),
            ]
        }
        ("set_generator", ObjectKind::Generator) => {
            let g = fb_meta::generator(conn, name)?;
            vec![field("value", "Value", Number)
                .value(g.value.map(|v| v.to_string()).unwrap_or_default())
                .required()
                .hint("The next value it gives is the one after this.")]
        }
        ("edit_exception", ObjectKind::Exception) => {
            let e = fb_meta::exception(conn, name)?;
            vec![field("message", "Message", Text).value(e.message.unwrap_or_default())]
        }
        _ => return Err(AppError::Invalid { message: format!("{action} asks nothing of this object.") }),
    };
    let title = match key {
        Some(key) => format!("{name}.{key}"),
        None => name.to_string(),
    };
    Ok(Form { title, fields })
}

/// An action as the script that carries it out
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActionScript {
    script: String,
    /// The name the object goes by once the script ran, when it renames it
    renamed: Option<String>,
}

/// The name a form gives the object itself, when it is not the one it has
fn renamed(action: &str, name: &str, values: &HashMap<String, String>, ctx: &Context) -> Option<String> {
    let typed = values.get("name").filter(|_| action == "edit_domain")?;
    let quoted = change::typed(typed, ctx);
    (quoted != fb_meta::quote(name, ctx.dialect)).then(|| unquoted(&quoted))
}

/// A name out of the quotes DDL has it in: as the catalog stores it
fn unquoted(name: &str) -> String {
    match name.strip_prefix('"').and_then(|n| n.strip_suffix('"')) {
        Some(inner) => inner.replace("\"\"", "\""),
        None => name.to_string(),
    }
}

/// The script that carries an action out, with a form's values when the
/// action has a form: for the user to see, and then to run
#[tauri::command]
pub async fn object_action_script(
    app: AppHandle,
    id: String,
    kind: ObjectKind,
    name: String,
    action: String,
    key: Option<String>,
    values: HashMap<String, String>,
) -> AppResult<ActionScript> {
    blocking(app, move |state| {
        let conn = state.session(&id)?;
        let statements = statements(&conn, kind, &name, &action, &key, &values)?;
        if statements.is_empty() {
            return Err(AppError::Invalid { message: "Nothing was changed.".into() });
        }
        let renamed = renamed(&action, &name, &values, &Context::of(conn.info()));
        Ok(ActionScript { script: script(&statements), renamed })
    })
    .await
}

fn statements(
    conn: &Connection,
    kind: ObjectKind,
    name: &str,
    action: &str,
    key: &Option<String>,
    values: &HashMap<String, String>,
) -> AppResult<Vec<Statement>> {
    let ctx = Context::of(conn.info());
    let get = |id: &str| values.get(id).map(String::as_str).unwrap_or_default().trim();
    let on = |id: &str| get(id) == "true";
    let needs = |id: &str, label: &str| -> AppResult<String> {
        match get(id) {
            "" => Err(AppError::Invalid { message: format!("{label} is required.") }),
            value => Ok(value.to_string()),
        }
    };
    // A value that is not what it was, or nothing
    let changed = |id: &str, was: &str| -> Option<String> {
        values.get(id).map(|v| v.trim()).filter(|v| *v != was.trim()).map(str::to_string)
    };
    let null_flag = |was: bool| -> AppResult<Option<bool>> {
        let now = values.get("not_null").map(|v| v.trim() == "true").filter(|now| *now != was);
        if now.is_some() && ctx.major < 3 {
            return Err(AppError::Invalid { message: "Firebird 2.5 cannot change NOT NULL of what exists.".into() });
        }
        Ok(now)
    };

    Ok(match (action, kind) {
        ("add_field", ObjectKind::Table) => {
            let column = NewColumn {
                name: needs("name", "Name")?,
                data_type: if get("computed").is_empty() { needs("type", "Type")? } else { String::new() },
                not_null: on("not_null"),
                default: get("default").into(),
                computed: get("computed").into(),
                description: get("description").into(),
            };
            change::add_column(name, &column, &ctx)
        }
        ("edit_field", ObjectKind::Table) => {
            let (position, c) = column(conn, name, key_of(key)?)?;
            let type_now = declared(&c, &ctx);
            let column_change = ColumnChange {
                rename: changed("name", &c.name).filter(|n| change::typed(n, &ctx) != fb_meta::quote(&c.name, ctx.dialect)),
                data_type: changed("type", &type_now).filter(|t| !t.eq_ignore_ascii_case(&type_now)),
                default: changed("default", &bare_default(c.default.as_deref())),
                not_null: if c.computed.is_none() { null_flag(c.not_null)? } else { None },
                position: changed("position", &(position + 1).to_string())
                    .map(|p| p.parse().map_err(|_| AppError::Invalid { message: "Position is a number, from 1.".into() }))
                    .transpose()?,
                description: changed("description", c.description.as_deref().unwrap_or_default()),
            };
            change::alter_column(name, &c.name, &column_change, &ctx)
        }
        ("drop_field", ObjectKind::Table) => vec![change::drop_column(name, key_of(key)?, &ctx)],
        ("add_primary_key", ObjectKind::Table) => {
            let constraint = NewConstraint::PrimaryKey { columns: needs("columns", "Columns")? };
            vec![change::add_constraint(name, get("name"), &constraint, &ctx)]
        }
        ("add_unique", ObjectKind::Table) => {
            let constraint = NewConstraint::Unique { columns: needs("columns", "Columns")? };
            vec![change::add_constraint(name, get("name"), &constraint, &ctx)]
        }
        ("add_foreign_key", ObjectKind::Table) => {
            let constraint = NewConstraint::ForeignKey {
                columns: needs("columns", "Columns")?,
                table: needs("table", "The referenced table")?,
                references: get("references").into(),
                on_update: get("on_update").into(),
                on_delete: get("on_delete").into(),
            };
            vec![change::add_constraint(name, get("name"), &constraint, &ctx)]
        }
        ("add_check", ObjectKind::Table) => {
            let constraint = NewConstraint::Check { condition: needs("condition", "Condition")? };
            vec![change::add_constraint(name, get("name"), &constraint, &ctx)]
        }
        ("drop_constraint", ObjectKind::Table) => vec![change::drop_constraint(name, key_of(key)?, &ctx)],
        ("add_index", ObjectKind::Table) => {
            let index = NewIndex {
                name: needs("name", "Name")?,
                columns: if get("expression").is_empty() { needs("columns", "Columns")? } else { String::new() },
                expression: get("expression").into(),
                unique: on("unique"),
                descending: on("descending"),
            };
            vec![change::create_index(name, &index, &ctx)]
        }
        ("drop_index", ObjectKind::Table) => vec![change::drop(ObjectKind::Index, key_of(key)?, &ctx)],
        ("index_activate" | "index_deactivate" | "index_statistics", ObjectKind::Table | ObjectKind::Index) => {
            // On a table's tab the row names the index; on its own, it is the object
            let index = if kind == ObjectKind::Index { name } else { key_of(key)? };
            vec![match action {
                "index_activate" => change::index_active(index, true, &ctx),
                "index_deactivate" => change::index_active(index, false, &ctx),
                _ => change::index_statistics(index, &ctx),
            }]
        }
        ("trigger_activate", ObjectKind::Table | ObjectKind::View) => {
            vec![change::trigger_active(key_of(key)?, true, &ctx)]
        }
        ("trigger_deactivate", ObjectKind::Table | ObjectKind::View) => {
            vec![change::trigger_active(key_of(key)?, false, &ctx)]
        }
        ("drop_trigger", ObjectKind::Table | ObjectKind::View) => {
            vec![change::drop(ObjectKind::Trigger, key_of(key)?, &ctx)]
        }
        ("edit_domain", ObjectKind::Domain) => {
            let d = fb_meta::domain(conn, name)?;
            let type_now = d.data_type.declaration(ctx.dialect, &ctx.charset);
            let domain_change = DomainChange {
                rename: changed("name", &d.name).filter(|n| change::typed(n, &ctx) != fb_meta::quote(&d.name, ctx.dialect)),
                data_type: changed("type", &type_now).filter(|t| !t.eq_ignore_ascii_case(&type_now)),
                default: changed("default", &bare_default(d.default.as_deref())),
                check: changed("check", d.check.as_deref().unwrap_or_default()),
                has_check: d.check.is_some(),
                not_null: null_flag(d.not_null)?,
            };
            change::alter_domain(name, &domain_change, &ctx)
        }
        ("set_generator", ObjectKind::Generator) => {
            let value = needs("value", "Value")?
                .parse()
                .map_err(|_| AppError::Invalid { message: "The value is a whole number.".into() })?;
            vec![change::set_generator(name, value, &ctx)]
        }
        ("edit_exception", ObjectKind::Exception) => vec![change::alter_exception(name, get("message"), &ctx)],
        _ => return Err(AppError::Invalid { message: format!("{action} cannot be done to this object.") }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_is_shown_without_its_keyword() {
        assert_eq!(bare_default(Some("DEFAULT 0")), "0");
        assert_eq!(bare_default(Some("default 'N'")), "'N'");
        assert_eq!(bare_default(Some("0")), "0");
        assert_eq!(bare_default(None), "");
    }

    #[test]
    fn a_rename_is_told_by_the_name_the_catalog_will_have() {
        let ctx = Context { dialect: 3, charset: "UTF8".into(), major: 4 };
        let values = |name: &str| HashMap::from([("name".to_string(), name.to_string())]);
        assert_eq!(renamed("edit_domain", "D_ID", &values("d_id"), &ctx), None);
        assert_eq!(renamed("edit_domain", "D_ID", &values("d_key"), &ctx), Some("D_KEY".into()));
        assert_eq!(renamed("edit_domain", "D_ID", &values("\"Key\""), &ctx), Some("Key".into()));
        // A field's name is not the object's
        assert_eq!(renamed("edit_field", "T", &values("other"), &ctx), None);
    }

    /// Against the server the `FB_*` variables name; skipped without them.
    /// Only reads: forms are filled from the catalog, and scripts are
    /// written, never run.
    #[test]
    fn forms_fill_and_unchanged_values_change_nothing() {
        let Some(params) = fb_core::ConnectParams::from_env() else {
            eprintln!("FB_DB not set: skipping live test");
            return;
        };
        let conn = Connection::open(&params).expect("connect");
        let none = None;
        if let Some(table) = fb_meta::objects(&conn, ObjectKind::Table).unwrap().first() {
            for action in ["add_field", "add_primary_key", "add_unique", "add_foreign_key", "add_check", "add_index"] {
                let form = form(&conn, ObjectKind::Table, &table.name, action, &none).unwrap();
                assert!(!form.fields.is_empty(), "{action}");
            }
            let columns = fb_meta::columns(&conn, Some(&table.name)).unwrap();
            for (_, c) in columns.iter().take(5) {
                let key = Some(c.name.clone());
                let form = form(&conn, ObjectKind::Table, &table.name, "edit_field", &key).unwrap();
                // Submitted as it came, the form changes nothing
                let values = form.fields.iter().map(|f| (f.id.to_string(), f.value.clone())).collect();
                let unchanged = statements(&conn, ObjectKind::Table, &table.name, "edit_field", &key, &values).unwrap();
                assert!(unchanged.is_empty(), "{}: {unchanged:?}", c.name);
                let dropped = statements(&conn, ObjectKind::Table, &table.name, "drop_field", &key, &HashMap::new());
                assert_eq!(dropped.unwrap().len(), 1);
            }
        }
        if let Some(domain) = fb_meta::objects(&conn, ObjectKind::Domain).unwrap().first() {
            let form = form(&conn, ObjectKind::Domain, &domain.name, "edit_domain", &none).unwrap();
            let values = form.fields.iter().map(|f| (f.id.to_string(), f.value.clone())).collect();
            let unchanged = statements(&conn, ObjectKind::Domain, &domain.name, "edit_domain", &none, &values).unwrap();
            assert!(unchanged.is_empty(), "{unchanged:?}");
        }
        if let Some(generator) = fb_meta::objects(&conn, ObjectKind::Generator).unwrap().first() {
            let form = form(&conn, ObjectKind::Generator, &generator.name, "set_generator", &none).unwrap();
            assert_eq!(form.fields.len(), 1);
        }
    }
}
