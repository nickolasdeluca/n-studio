//! What can be done to an object from its viewer: each action a form to
//! fill (or none), and the script that carries it out. The UI draws the
//! forms and shows the scripts; which statements change a Firebird column
//! is known only here and in `fb_meta::change`.

use std::collections::HashMap;

use fb_core::Connection;
use fb_meta::change::{self, ColumnChange, DomainChange, NewColumn, NewConstraint, NewIndex};
use fb_meta::ddl::{script, Context, Statement};
use fb_meta::grant::{self, Grant, GranteeKind};
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
pub fn attach(sections: &mut [GridSection], kind: ObjectKind, tab: &str, major: u32) {
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
        ("grants", _) if !grantable(kind, major) => {}
        ("grants", ObjectKind::Role) => {
            section.actions = vec![asks("add_member", "Add Member…")];
            section.row_actions = vec![destroys("revoke", "Revoke…")];
        }
        ("grants", _) => {
            section.actions = vec![asks("grant", "Grant…")];
            section.row_actions = vec![destroys("revoke", "Revoke…")];
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

/// Whether the server has privileges on the kind: on sequences and
/// exceptions, as on functions and packages, from Firebird 3
fn grantable(kind: ObjectKind, major: u32) -> bool {
    match kind {
        ObjectKind::Generator | ObjectKind::Exception | ObjectKind::Function | ObjectKind::Package => major >= 3,
        _ => !grant::object_types(kind).is_empty(),
    }
}

/// What parts a grant's key; no name has it
const KEY_PART: char = '\u{1f}';

/// A grant's row key: the grantee and the one privilege, enough to revoke it
pub fn grant_key(g: &Grant) -> String {
    let column = g.column.as_deref().unwrap_or_default();
    [g.grantee_kind.label(), &g.grantee, &g.privilege.to_string(), column].join(&KEY_PART.to_string())
}

/// The grant on an object that a row key names
fn keyed_grant(kind: ObjectKind, name: &str, key: &str) -> AppResult<Grant> {
    let invalid = || AppError::Invalid { message: "This row is not a grant that can be revoked.".into() };
    let parts: Vec<&str> = key.split(KEY_PART).collect();
    let [grantee_kind, grantee, privilege, column] = parts[..] else { return Err(invalid()) };
    Ok(Grant {
        object: name.to_string(),
        object_type: *grant::object_types(kind).first().ok_or_else(invalid)?,
        privilege: privilege.chars().next().ok_or_else(invalid)?,
        column: (!column.is_empty()).then(|| column.to_string()),
        grantee: grantee.to_string(),
        grantee_kind: GranteeKind::labelled(grantee_kind).ok_or_else(invalid)?,
        option: false,
        default_role: false,
        grantor: None,
    })
}

/// The privileges a table or a view has, by the form's field for each
const RELATION_PRIVILEGES: [(&str, &str, char); 5] = [
    ("select", "Select", 'S'),
    ("insert", "Insert", 'I'),
    ("update", "Update", 'U'),
    ("delete", "Delete", 'D'),
    ("references", "References", 'R'),
];

fn role_names(conn: &Connection) -> AppResult<Vec<String>> {
    Ok(fb_meta::objects(conn, ObjectKind::Role)?.into_iter().map(|r| r.name).collect())
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
        ("grant", _) if kind != ObjectKind::Role && grantable(kind, ctx.major) => {
            let kinds = GranteeKind::ALL
                .into_iter()
                .filter(|k| ctx.major >= 3 || !matches!(k, GranteeKind::Function | GranteeKind::Package))
                .map(GranteeKind::label);
            let mut fields = vec![
                field("grantee", "Grantee", Combo)
                    .required()
                    .options(std::iter::once("PUBLIC".to_string()).chain(role_names(conn)?))
                    .hint("A user's name, PUBLIC, a role, or an object of the database."),
                field("grantee_kind", "Grantee is a", Select)
                    .value(GranteeKind::User.label())
                    .options(kinds)
                    .hint("A role's name given as a user's is taken as the role."),
            ];
            if matches!(kind, ObjectKind::Table | ObjectKind::View) {
                for (id, label, privilege) in RELATION_PRIVILEGES {
                    fields.push(field(id, label, Check).value(flag(privilege == 'S')));
                }
                fields.push(
                    field("columns", "Columns", Text)
                        .hint("For Update and References only. Left empty, every column; separated by commas."),
                );
            }
            fields.push(field("option", "With grant option", Check).value("false"));
            fields
        }
        ("add_member", ObjectKind::Role) => {
            let mut fields = vec![field("grantee", "Member", Text).required().hint("A user's name.")];
            if ctx.major >= 4 {
                fields[0] = field("grantee", "Member", Combo)
                    .required()
                    .options(role_names(conn)?.into_iter().filter(|r| r != name))
                    .hint("A user's name, or another role.");
                fields.push(
                    field("grantee_kind", "Member is a", Select)
                        .value(GranteeKind::User.label())
                        .options([GranteeKind::User.label(), GranteeKind::Role.label()]),
                );
            }
            fields.push(field("option", "With admin option", Check).value("false"));
            fields
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
    Ok(Form { title: title(name, action, key), fields })
}

/// An action as the script that carries it out
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActionScript {
    script: String,
    /// What the script is about, for whoever reads it before it runs
    title: String,
    /// The name the object goes by once the script ran, when it renames it
    renamed: Option<String>,
}

/// What an action is about: the object, and the part of it when it has one
fn title(name: &str, action: &str, key: &Option<String>) -> String {
    match key.as_deref() {
        // A grant's key is not for reading: its grantee is
        Some(key) if action == "revoke" => format!("{name}, {}", key.split(KEY_PART).nth(1).unwrap_or_default()),
        Some(key) => format!("{name}.{key}"),
        None => name.to_string(),
    }
}

/// The name a form gives the object itself, when it is not the one it has
fn renamed(action: &str, name: &str, values: &HashMap<String, String>, ctx: &Context) -> Option<String> {
    let typed = values.get("name").filter(|_| action == "edit_domain")?;
    let quoted = change::typed(typed, ctx);
    (quoted != fb_meta::quote(name, ctx.dialect)).then(|| change::stored(typed))
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
        Ok(ActionScript { script: script(&statements), title: title(&name, &action, &key), renamed })
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
        ("grant" | "add_member", _) if grantable(kind, ctx.major) && (action == "add_member") == (kind == ObjectKind::Role) => {
            let typed = needs("grantee", if kind == ObjectKind::Role { "Member" } else { "Grantee" })?;
            let public = typed.eq_ignore_ascii_case("PUBLIC");
            let grantee = if public { "PUBLIC".to_string() } else { change::stored(&typed) };
            let mut grantee_kind = GranteeKind::labelled(get("grantee_kind")).unwrap_or(GranteeKind::User);
            // Before Firebird 4 the server itself reads a role's name so
            if grantee_kind == GranteeKind::User && !public && kind != ObjectKind::Role && role_names(conn)?.contains(&grantee) {
                grantee_kind = GranteeKind::Role;
            }
            let columns: Vec<String> = get("columns").split(',').map(change::stored).filter(|c| !c.is_empty()).collect();
            let privileges: Vec<char> = match grant::only_privilege(kind) {
                Some(only) => vec![only],
                None => RELATION_PRIVILEGES.iter().filter(|(id, ..)| on(id)).map(|(.., p)| *p).collect(),
            };
            if privileges.is_empty() {
                return Err(AppError::Invalid { message: "Pick at least one privilege.".into() });
            }
            let one = |privilege: char, column: Option<String>| Grant {
                object: name.to_string(),
                object_type: grant::object_types(kind)[0],
                privilege,
                column,
                grantee: grantee.clone(),
                grantee_kind,
                option: on("option"),
                default_role: false,
                grantor: None,
            };
            let mut grants = Vec::new();
            for privilege in privileges {
                if matches!(privilege, 'U' | 'R') && !columns.is_empty() {
                    grants.extend(columns.iter().map(|c| one(privilege, Some(c.clone()))));
                } else {
                    grants.push(one(privilege, None));
                }
            }
            grant::statements(&grants, &ctx)
        }
        ("revoke", _) if grantable(kind, ctx.major) => {
            let revoked = grant::revoke(&keyed_grant(kind, name, key_of(key)?)?, &ctx);
            vec![revoked.ok_or_else(|| AppError::Invalid { message: "This privilege is not one that can be revoked here.".into() })?]
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

    #[test]
    fn a_grants_row_key_names_the_grant_again() {
        let granted = Grant {
            object: "PEDIDO".into(),
            object_type: 0,
            privilege: 'U',
            column: Some("CLIENTE".into()),
            grantee: "R VENDAS".into(),
            grantee_kind: GranteeKind::Role,
            option: false,
            default_role: false,
            grantor: None,
        };
        let key = grant_key(&granted);
        assert_eq!(keyed_grant(ObjectKind::Table, "PEDIDO", &key).unwrap(), granted);
        assert_eq!(title("PEDIDO", "revoke", &Some(key)), "PEDIDO, R VENDAS");
        assert_eq!(title("PEDIDO", "edit_field", &Some("CLIENTE".into())), "PEDIDO.CLIENTE");
        // A row of another grid is no grant, and an index has none
        assert!(keyed_grant(ObjectKind::Table, "PEDIDO", "CLIENTE").is_err());
        assert!(keyed_grant(ObjectKind::Index, "PEDIDO", &grant_key(&granted)).is_err());
    }

    #[test]
    fn privileges_wait_for_the_server_that_has_them() {
        assert!(grantable(ObjectKind::Table, 2) && grantable(ObjectKind::Procedure, 2) && grantable(ObjectKind::Role, 2));
        assert!(!grantable(ObjectKind::Generator, 2) && grantable(ObjectKind::Generator, 3));
        assert!(!grantable(ObjectKind::Index, 5) && !grantable(ObjectKind::Domain, 5));
    }

    /// Against the server the `FB_*` variables name; skipped without them.
    /// Only reads: the scripts are written, never run.
    #[test]
    fn grants_are_written_and_each_one_there_can_be_revoked() {
        let Some(params) = fb_core::ConnectParams::from_env() else {
            eprintln!("FB_DB not set: skipping live test");
            return;
        };
        let conn = Connection::open(&params).expect("connect");
        let ctx = Context::of(conn.info());
        let values = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
        };
        let sql = |statements: Vec<Statement>| statements.into_iter().map(|s| s.sql).collect::<Vec<_>>();
        for kind in [ObjectKind::Table, ObjectKind::Procedure, ObjectKind::Role] {
            let objects = fb_meta::objects(&conn, kind).unwrap();
            let Some(object) = objects.first() else { continue };
            let name = fb_meta::quote(&object.name, ctx.dialect);
            let action = if kind == ObjectKind::Role { "add_member" } else { "grant" };
            assert!(!form(&conn, kind, &object.name, action, &None).unwrap().fields.is_empty(), "{kind:?}");
            let filled = values(&[("grantee", "someone"), ("select", "true"), ("update", "true"), ("columns", "a, b")]);
            let written = sql(statements(&conn, kind, &object.name, action, &None, &filled).unwrap());
            let expected = match kind {
                ObjectKind::Table => format!("GRANT SELECT, UPDATE (A, B) ON {name} TO SOMEONE"),
                ObjectKind::Procedure => format!("GRANT EXECUTE ON PROCEDURE {name} TO SOMEONE"),
                _ => format!("GRANT {name} TO SOMEONE"),
            };
            assert_eq!(written, [expected]);
            // What is granted on a few of the kind's objects
            for object in objects.iter().take(20) {
                for granted in grant::on(&conn, kind, &object.name).unwrap() {
                    let key = Some(grant_key(&granted));
                    let revoked = statements(&conn, kind, &object.name, "revoke", &key, &HashMap::new()).unwrap();
                    assert_eq!(revoked.len(), 1);
                    assert!(revoked[0].sql.starts_with("REVOKE "), "{}", revoked[0].sql);
                }
            }
        }
        let table = fb_meta::objects(&conn, ObjectKind::Table).unwrap();
        if let Some(table) = table.first() {
            let nothing = statements(&conn, ObjectKind::Table, &table.name, "grant", &None, &values(&[("grantee", "x")]));
            assert!(nothing.is_err(), "a grant of no privilege");
            assert!(statements(&conn, ObjectKind::Table, &table.name, "add_member", &None, &HashMap::new()).is_err());
        }
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
