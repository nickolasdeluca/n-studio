//! Privileges: who may do what to an object, and who is in a role.
//!
//! The catalog keeps one row for each privilege, grantee and column
//! (`RDB$USER_PRIVILEGES`); a `GRANT` statement gives several at once, so
//! writing the statements back is grouping the rows again. What an owner
//! holds for having created the object is in the catalog too, and is not a
//! grant anyone made: those rows are left out.

use fb_core::{Connection, Result};
use serde::Serialize;

use crate::ddl::{Context, Statement};
use crate::ident::quote;
use crate::row::rows;
use crate::ObjectKind;

/// `RDB$OBJECT_TYPE` of a role, which is what a membership is granted on
const ROLE: i64 = 13;

/// What holds a privilege
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GranteeKind {
    User,
    Role,
    View,
    Trigger,
    Procedure,
    Function,
    Package,
    Other,
}

impl GranteeKind {
    pub const ALL: [GranteeKind; 7] = [
        GranteeKind::User,
        GranteeKind::Role,
        GranteeKind::Procedure,
        GranteeKind::Trigger,
        GranteeKind::View,
        GranteeKind::Function,
        GranteeKind::Package,
    ];

    /// From `RDB$USER_TYPE`
    fn of(user_type: i64) -> Self {
        match user_type {
            8 => GranteeKind::User,
            13 => GranteeKind::Role,
            1 => GranteeKind::View,
            2 => GranteeKind::Trigger,
            5 => GranteeKind::Procedure,
            15 => GranteeKind::Function,
            18 | 19 => GranteeKind::Package,
            _ => GranteeKind::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            GranteeKind::User => "User",
            GranteeKind::Role => "Role",
            GranteeKind::View => "View",
            GranteeKind::Trigger => "Trigger",
            GranteeKind::Procedure => "Procedure",
            GranteeKind::Function => "Function",
            GranteeKind::Package => "Package",
            GranteeKind::Other => "Other",
        }
    }

    pub fn labelled(label: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.label() == label)
    }

    /// The object a grantee of this kind is, when it is one the explorer has
    pub fn object(self) -> Option<ObjectKind> {
        Some(match self {
            GranteeKind::Role => ObjectKind::Role,
            GranteeKind::View => ObjectKind::View,
            GranteeKind::Trigger => ObjectKind::Trigger,
            GranteeKind::Procedure => ObjectKind::Procedure,
            GranteeKind::Function => ObjectKind::Function,
            GranteeKind::Package => ObjectKind::Package,
            GranteeKind::User | GranteeKind::Other => return None,
        })
    }
}

/// One row of the catalog: one privilege, on one object or one column of
/// it, held by one grantee
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    pub object: String,
    /// `RDB$OBJECT_TYPE`: what kind of thing `object` is
    pub object_type: i64,
    /// `RDB$PRIVILEGE`: S, I, U, D, R, X (execute), G (usage), M (member)
    pub privilege: char,
    /// The one column an UPDATE or a REFERENCES is for
    pub column: Option<String>,
    pub grantee: String,
    pub grantee_kind: GranteeKind,
    /// The grantee may grant it on: WITH GRANT OPTION, or WITH ADMIN OPTION
    pub option: bool,
    /// A membership the member has without asking for it (Firebird 4)
    pub default_role: bool,
    pub grantor: Option<String>,
}

impl Grant {
    /// The privilege as GRANT names it; `None` for one this does not write
    pub fn privilege_name(&self) -> Option<&'static str> {
        privilege_name(self.privilege)
    }

    pub fn is_membership(&self) -> bool {
        self.object_type == ROLE && self.privilege == 'M'
    }
}

pub fn privilege_name(privilege: char) -> Option<&'static str> {
    Some(match privilege {
        'S' => "SELECT",
        'I' => "INSERT",
        'U' => "UPDATE",
        'D' => "DELETE",
        'R' => "REFERENCES",
        'X' => "EXECUTE",
        'G' => "USAGE",
        _ => return None,
    })
}

/// The `RDB$OBJECT_TYPE`s privileges on a kind of object are kept under;
/// empty for a kind nothing is granted on
pub fn object_types(kind: ObjectKind) -> &'static [i64] {
    match kind {
        ObjectKind::Table | ObjectKind::View => &[0, 1],
        ObjectKind::Procedure => &[5],
        ObjectKind::Function => &[15],
        ObjectKind::Package => &[18],
        ObjectKind::Generator => &[14],
        ObjectKind::Exception => &[7],
        ObjectKind::Role => &[ROLE],
        ObjectKind::Domain | ObjectKind::Trigger | ObjectKind::Udf | ObjectKind::Index => &[],
    }
}

/// The privilege that is the only one a kind of object has, if it is so
pub fn only_privilege(kind: ObjectKind) -> Option<char> {
    match kind {
        ObjectKind::Procedure | ObjectKind::Function | ObjectKind::Package => Some('X'),
        ObjectKind::Generator | ObjectKind::Exception => Some('G'),
        ObjectKind::Role => Some('M'),
        _ => None,
    }
}

const SELECT: &str = "select trim(p.rdb$relation_name), p.rdb$object_type, p.rdb$privilege, trim(p.rdb$field_name), \
     trim(p.rdb$user), p.rdb$user_type, p.rdb$grant_option, trim(p.rdb$grantor) \
     from rdb$user_privileges p \
     where not (p.rdb$user_type = 8 and (p.rdb$grantor is null or p.rdb$grantor = p.rdb$user))";

fn read(conn: &Connection, filter: &str, params: Vec<fb_core::Value>) -> Result<Vec<Grant>> {
    let mut grants: Vec<Grant> = rows(conn, &format!("{SELECT} and {filter}"), params)?
        .into_iter()
        .filter_map(|row| {
            let object_type = row.int(1).unwrap_or(0);
            let privilege = row.text(2)?.chars().next()?;
            let field = row.text(3);
            // On a role the field is not a column: 'D' marks a default role
            let default_role = object_type == ROLE && field.as_deref() == Some("D");
            Some(Grant {
                object: row.text(0)?,
                object_type,
                privilege,
                column: if object_type == ROLE { None } else { field },
                grantee: row.text(4)?,
                grantee_kind: GranteeKind::of(row.int(5).unwrap_or(8)),
                option: row.int(6).unwrap_or(0) != 0,
                default_role,
                grantor: row.text(7),
            })
        })
        .collect();
    let order = |privilege: char| "SIUDRXGM".find(privilege).unwrap_or(usize::MAX);
    grants.sort_by(|a, b| {
        (&a.object, a.grantee_kind.label(), &a.grantee, order(a.privilege), &a.column).cmp(&(
            &b.object,
            b.grantee_kind.label(),
            &b.grantee,
            order(b.privilege),
            &b.column,
        ))
    });
    Ok(grants)
}

fn type_list(types: &[i64]) -> String {
    types.iter().map(i64::to_string).collect::<Vec<_>>().join(", ")
}

/// What was granted on an object; for a role, who is in it
pub fn on(conn: &Connection, kind: ObjectKind, name: &str) -> Result<Vec<Grant>> {
    let types = object_types(kind);
    if types.is_empty() {
        return Ok(Vec::new());
    }
    let filter = format!("p.rdb$relation_name = ? and p.rdb$object_type in ({})", type_list(types));
    read(conn, &filter, vec![name.into()])
}

/// What a role was granted: privileges on objects, and other roles
pub fn held_by_role(conn: &Connection, role: &str) -> Result<Vec<Grant>> {
    read(conn, "p.rdb$user = ? and p.rdb$user_type = 13", vec![role.into()])
}

/// Every grant on the user's objects, for many objects to pick theirs from
pub fn all(conn: &Connection) -> Result<Vec<Grant>> {
    read(conn, "p.rdb$relation_name not starting with 'RDB$' and p.rdb$relation_name not starting with 'MON$' and p.rdb$relation_name not starting with 'SEC$'", vec![])
}

fn grantee(grant: &Grant, ctx: &Context) -> String {
    let name = if grant.grantee == "PUBLIC" { "PUBLIC".to_string() } else { quote(&grant.grantee, ctx.dialect) };
    match grant.grantee_kind {
        GranteeKind::User | GranteeKind::Other => name,
        kind => format!("{} {name}", kind.label().to_ascii_uppercase()),
    }
}

/// `ON …`, as GRANT and REVOKE name the object
fn target(grant: &Grant, ctx: &Context) -> String {
    let name = quote(&grant.object, ctx.dialect);
    match grant.object_type {
        5 => format!("ON PROCEDURE {name}"),
        15 => format!("ON FUNCTION {name}"),
        18 => format!("ON PACKAGE {name}"),
        14 => format!("ON SEQUENCE {name}"),
        7 => format!("ON EXCEPTION {name}"),
        _ => format!("ON {name}"),
    }
}

/// The GRANT statements that give the grants again: one for each grantee
/// of an object, with all it was given there
pub fn statements(grants: &[Grant], ctx: &Context) -> Vec<Statement> {
    let mut out = Vec::new();
    let mut done = vec![false; grants.len()];
    for (i, first) in grants.iter().enumerate() {
        if done[i] {
            continue;
        }
        if first.is_membership() {
            done[i] = true;
            let default = if first.default_role { "DEFAULT " } else { "" };
            let option = if first.option { " WITH ADMIN OPTION" } else { "" };
            let role = quote(&first.object, ctx.dialect);
            out.push(Statement::plain(format!("GRANT {default}{role} TO {}{option}", grantee(first, ctx))));
            continue;
        }
        let same = |g: &Grant| {
            !g.is_membership()
                && g.object == first.object
                && g.object_type == first.object_type
                && g.grantee == first.grantee
                && g.grantee_kind == first.grantee_kind
                && g.option == first.option
        };
        let group: Vec<&Grant> = grants.iter().enumerate().filter(|(j, g)| !done[*j] && same(g)).map(|(_, g)| g).collect();
        for (j, g) in grants.iter().enumerate() {
            done[j] |= same(g);
        }
        let privileges = privileges(&group, ctx);
        if privileges.is_empty() {
            continue;
        }
        let option = if first.option { " WITH GRANT OPTION" } else { "" };
        out.push(Statement::plain(format!(
            "GRANT {privileges} {} TO {}{option}",
            target(first, ctx),
            grantee(first, ctx)
        )));
    }
    out
}

/// What one GRANT gives: `SELECT, UPDATE (A, B)`, or `ALL`
fn privileges(group: &[&Grant], ctx: &Context) -> String {
    let whole = |p: char| group.iter().any(|g| g.privilege == p && g.column.is_none());
    if "SIUDR".chars().all(whole) {
        return "ALL".into();
    }
    let mut out: Vec<String> = Vec::new();
    for p in "SIUDRXG".chars() {
        let Some(name) = privilege_name(p) else { continue };
        if whole(p) {
            out.push(name.to_string());
            continue;
        }
        let columns: Vec<String> = group
            .iter()
            .filter(|g| g.privilege == p)
            .filter_map(|g| g.column.as_deref())
            .map(|c| quote(c, ctx.dialect))
            .collect();
        if !columns.is_empty() {
            out.push(format!("{name} ({})", columns.join(", ")));
        }
    }
    out.join(", ")
}

/// The statement that takes one grant back
pub fn revoke(grant: &Grant, ctx: &Context) -> Option<Statement> {
    if grant.is_membership() {
        let role = quote(&grant.object, ctx.dialect);
        return Some(Statement::plain(format!("REVOKE {role} FROM {}", grantee(grant, ctx))));
    }
    let mut privilege = grant.privilege_name()?.to_string();
    if let Some(column) = &grant.column {
        privilege.push_str(&format!(" ({})", quote(column, ctx.dialect)));
    }
    Some(Statement::plain(format!("REVOKE {privilege} {} FROM {}", target(grant, ctx), grantee(grant, ctx))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Context {
        Context { dialect: 3, charset: "UTF8".into(), major: 4 }
    }

    fn grant(privilege: char, column: Option<&str>, grantee: &str, kind: GranteeKind) -> Grant {
        Grant {
            object: "PEDIDO".into(),
            object_type: 0,
            privilege,
            column: column.map(String::from),
            grantee: grantee.into(),
            grantee_kind: kind,
            option: false,
            default_role: false,
            grantor: Some("SYSDBA".into()),
        }
    }

    fn sql(grants: &[Grant]) -> Vec<String> {
        statements(grants, &ctx()).into_iter().map(|s| s.sql).collect()
    }

    #[test]
    fn a_grantees_rows_are_one_grant_again() {
        let grants = [
            grant('S', None, "R_VENDAS", GranteeKind::Role),
            grant('U', Some("CLIENTE"), "R_VENDAS", GranteeKind::Role),
            grant('U', Some("EMISSAO"), "R_VENDAS", GranteeKind::Role),
            grant('S', None, "PUBLIC", GranteeKind::User),
            grant('S', None, "P_TOTAL", GranteeKind::Procedure),
        ];
        assert_eq!(
            sql(&grants),
            [
                "GRANT SELECT, UPDATE (CLIENTE, EMISSAO) ON PEDIDO TO ROLE R_VENDAS",
                "GRANT SELECT ON PEDIDO TO PUBLIC",
                "GRANT SELECT ON PEDIDO TO PROCEDURE P_TOTAL",
            ]
        );
    }

    #[test]
    fn all_five_are_all_and_the_option_parts_grants() {
        let mut grants: Vec<Grant> = "SIUDR".chars().map(|p| grant(p, None, "MARIA", GranteeKind::User)).collect();
        grants.push(Grant { option: true, ..grant('S', None, "MARIA", GranteeKind::User) });
        grants[4].option = true;
        assert_eq!(
            sql(&grants),
            [
                "GRANT SELECT, INSERT, UPDATE, DELETE ON PEDIDO TO MARIA",
                "GRANT SELECT, REFERENCES ON PEDIDO TO MARIA WITH GRANT OPTION",
            ]
        );
        let all: Vec<Grant> = "SIUDR".chars().map(|p| grant(p, None, "Maria", GranteeKind::User)).collect();
        assert_eq!(sql(&all), ["GRANT ALL ON PEDIDO TO \"Maria\""]);
    }

    #[test]
    fn each_kind_of_object_is_named_its_way() {
        let on = |object_type, privilege| Grant { object_type, ..grant(privilege, None, "MARIA", GranteeKind::User) };
        assert_eq!(
            sql(&[on(5, 'X'), on(15, 'X'), on(18, 'X'), on(14, 'G'), on(7, 'G')]),
            [
                "GRANT EXECUTE ON PROCEDURE PEDIDO TO MARIA",
                "GRANT EXECUTE ON FUNCTION PEDIDO TO MARIA",
                "GRANT EXECUTE ON PACKAGE PEDIDO TO MARIA",
                "GRANT USAGE ON SEQUENCE PEDIDO TO MARIA",
                "GRANT USAGE ON EXCEPTION PEDIDO TO MARIA",
            ]
        );
    }

    #[test]
    fn a_membership_is_the_role_granted() {
        let member = Grant { object: "R_VENDAS".into(), object_type: 13, ..grant('M', None, "MARIA", GranteeKind::User) };
        let admin = Grant { option: true, grantee: "JOSE".into(), ..member.clone() };
        let default = Grant { default_role: true, grantee: "R_TODOS".into(), grantee_kind: GranteeKind::Role, ..member.clone() };
        assert_eq!(
            sql(&[member.clone(), admin, default]),
            [
                "GRANT R_VENDAS TO MARIA",
                "GRANT R_VENDAS TO JOSE WITH ADMIN OPTION",
                "GRANT DEFAULT R_VENDAS TO ROLE R_TODOS",
            ]
        );
        assert_eq!(revoke(&member, &ctx()).unwrap().sql, "REVOKE R_VENDAS FROM MARIA");
    }

    #[test]
    fn a_grant_is_taken_back_alone() {
        let column = grant('U', Some("CLIENTE"), "R_VENDAS", GranteeKind::Role);
        assert_eq!(revoke(&column, &ctx()).unwrap().sql, "REVOKE UPDATE (CLIENTE) ON PEDIDO FROM ROLE R_VENDAS");
        let execute = Grant { object_type: 5, ..grant('X', None, "PUBLIC", GranteeKind::User) };
        assert_eq!(revoke(&execute, &ctx()).unwrap().sql, "REVOKE EXECUTE ON PROCEDURE PEDIDO FROM PUBLIC");
        // A privilege this does not write is not guessed at
        assert_eq!(revoke(&grant('C', None, "MARIA", GranteeKind::User), &ctx()), None);
    }
}
