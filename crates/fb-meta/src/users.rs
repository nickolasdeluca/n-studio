//! The server's users. Firebird 3 lists them in `SEC$USERS`; before it only
//! the services manager can (`fb_core::service_users`). Either way they are
//! changed in SQL, which every version here has.

use fb_core::{Connection, Result};
use serde::{Deserialize, Serialize};

use crate::ddl::{Context, Statement};
use crate::ident::{literal, quote};
use crate::row::rows;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct User {
    pub name: String,
    pub first_name: Option<String>,
    pub middle_name: Option<String>,
    pub last_name: Option<String>,
    /// An inactive user cannot log in. A user never given a state has none
    /// stored, and is active. Always true before Firebird 3.
    pub active: bool,
    /// Holds RDB$ADMIN in the security database: may manage users.
    /// Unknown before Firebird 3.
    pub admin: bool,
    /// The user manager that keeps it (`Srp`, `Legacy_UserManager`):
    /// Firebird 3 and later, where one name may be kept by several
    pub plugin: Option<String>,
    pub description: Option<String>,
}

/// The users this connection may see (Firebird 3 and later): all of them
/// for an administrator, itself for anyone else
pub fn users(conn: &Connection) -> Result<Vec<User>> {
    let sql = "select sec$user_name, sec$first_name, sec$middle_name, sec$last_name, coalesce(sec$active, true), sec$admin, \
               sec$plugin, sec$description from sec$users order by sec$user_name, sec$plugin";
    Ok(rows(conn, sql, vec![])?
        .iter()
        .filter_map(|r| {
            Some(User {
                name: r.text(0)?,
                first_name: r.text(1),
                middle_name: r.text(2),
                last_name: r.text(3),
                active: r.flag(4),
                admin: r.flag(5),
                plugin: r.text(6),
                description: r.text(7),
            })
        })
        .collect())
}

/// What a user is created with or changed to. `None` leaves a thing as it
/// is; an empty name removes it.
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct UserChange {
    pub password: Option<String>,
    pub first_name: Option<String>,
    pub middle_name: Option<String>,
    pub last_name: Option<String>,
    /// Firebird 3 and later
    pub active: Option<bool>,
    pub admin: Option<bool>,
    /// Firebird 3 and later
    pub plugin: Option<String>,
}

fn clauses(change: &UserChange, ctx: &Context, altering: bool) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(password) = &change.password {
        out.push(format!("PASSWORD {}", literal(password)));
    }
    for (keyword, value) in [
        ("FIRSTNAME", &change.first_name),
        ("MIDDLENAME", &change.middle_name),
        ("LASTNAME", &change.last_name),
    ] {
        if let Some(value) = value {
            out.push(format!("{keyword} {}", literal(value.trim())));
        }
    }
    if ctx.major >= 3 {
        if let Some(active) = change.active {
            out.push(if active { "ACTIVE" } else { "INACTIVE" }.to_string());
        }
        if let Some(plugin) = change.plugin.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            out.push(format!("USING PLUGIN {plugin}"));
        }
    }
    match change.admin {
        Some(true) => out.push("GRANT ADMIN ROLE".to_string()),
        Some(false) if altering => out.push("REVOKE ADMIN ROLE".to_string()),
        _ => {}
    }
    out
}

fn statement(verb: &str, name: &str, clauses: Vec<String>, ctx: &Context) -> Statement {
    let mut sql = format!("{verb} USER {}", quote(name, ctx.dialect));
    for clause in clauses {
        sql.push_str("\n    ");
        sql.push_str(&clause);
    }
    Statement::plain(sql)
}

/// `CREATE USER`. A user needs a password: `None` when there is none.
pub fn create(name: &str, change: &UserChange, ctx: &Context) -> Option<Statement> {
    change.password.as_deref().filter(|p| !p.is_empty())?;
    Some(statement("CREATE", name, clauses(change, ctx, false), ctx))
}

/// `ALTER USER`, or `None` when nothing changes
pub fn alter(name: &str, change: &UserChange, ctx: &Context) -> Option<Statement> {
    let clauses = clauses(change, ctx, true);
    // The plugin only says where the user is kept: alone, it changes nothing
    let changes = clauses.iter().any(|c| !c.starts_with("USING PLUGIN"));
    changes.then(|| statement("ALTER", name, clauses, ctx))
}

pub fn drop(name: &str, plugin: Option<&str>, ctx: &Context) -> Statement {
    let plugin = plugin.map(str::trim).filter(|p| !p.is_empty() && ctx.major >= 3);
    statement("DROP", name, plugin.map(|p| format!("USING PLUGIN {p}")).into_iter().collect(), ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(major: u32) -> Context {
        Context { dialect: 3, charset: "UTF8".into(), major }
    }

    #[test]
    fn a_new_user_needs_a_password() {
        assert!(create("ANA", &UserChange::default(), &ctx(4)).is_none());
        let change = UserChange {
            password: Some("it's".into()),
            first_name: Some("Ana".into()),
            active: Some(false),
            admin: Some(true),
            plugin: Some("Srp".into()),
            ..Default::default()
        };
        assert_eq!(
            create("ANA", &change, &ctx(4)).unwrap().sql,
            "CREATE USER ANA\n    PASSWORD 'it''s'\n    FIRSTNAME 'Ana'\n    INACTIVE\n    USING PLUGIN Srp\n    GRANT ADMIN ROLE"
        );
        // Firebird 2.5 has neither a state nor plugins
        assert_eq!(
            create("ANA", &change, &ctx(2)).unwrap().sql,
            "CREATE USER ANA\n    PASSWORD 'it''s'\n    FIRSTNAME 'Ana'\n    GRANT ADMIN ROLE"
        );
    }

    #[test]
    fn altering_says_only_what_changes() {
        assert!(alter("ANA", &UserChange::default(), &ctx(4)).is_none());
        assert!(alter("ANA", &UserChange { plugin: Some("Srp".into()), ..Default::default() }, &ctx(4)).is_none());
        let change = UserChange { last_name: Some(String::new()), admin: Some(false), ..Default::default() };
        assert_eq!(alter("ANA", &change, &ctx(4)).unwrap().sql, "ALTER USER ANA\n    LASTNAME ''\n    REVOKE ADMIN ROLE");
    }

    #[test]
    fn a_name_that_is_not_a_plain_one_is_quoted() {
        assert_eq!(drop("ana", Some("Srp"), &ctx(4)).sql, "DROP USER \"ana\"\n    USING PLUGIN Srp");
        assert_eq!(drop("ANA", Some("Srp"), &ctx(2)).sql, "DROP USER ANA");
    }
}
