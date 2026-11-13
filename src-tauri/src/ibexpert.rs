//! Importing from IBExpert: its registered databases and its code
//! formatter's options, read from the User Database IBExpert keeps its
//! settings in. That is a Firebird database like any other, so it is read
//! over a connection, in a read-only transaction.
//!
//! What is there: `DATABASES` holds the explorer's tree, a row a folder
//! (`REC_TYPE` 1) or a database (0) under `PARENT_ID`, its settings as
//! `Key=Value` lines in the `PROPS` blob. `CONFIG` holds one blob per group
//! of options, the formatter's an INI under `CodeFormatOptions`.
//!
//! Passwords are not imported: IBExpert stores them obfuscated, and here
//! they belong to the keychain, asked for on the first connect.

use std::collections::HashMap;

use fb_core::{Connection, Dialect, Value};
use fb_sql::format::{Case, Lists, Style};
use serde::Serialize;
use tauri::AppHandle;

use crate::commands::{blocking, open};
use crate::error::{AppError, AppResult};
use crate::registry::{Auth, Engine, FirebirdSettings, Registration};

/// What an IBExpert User Database has to bring over
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    /// In the order of IBExpert's explorer
    pub databases: Vec<FoundDatabase>,
    /// IBExpert's formatter options, the ones this formatter has too
    pub style: Option<Style>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FoundDatabase {
    /// Not saved yet: without an id, under the folders IBExpert has it in
    pub registration: Registration,
    /// The same server, port and database are registered here already
    pub registered: bool,
    /// The character set IBExpert connected with, when this app cannot
    pub charset_replaced: Option<String>,
}

/// A row of `DATABASES`
struct Row {
    id: i64,
    parent: i64,
    folder: bool,
    order: i64,
    props: String,
}

/// The character set an imported registration gets when IBExpert's is not
/// one to connect with (`NONE`, or none at all)
const FALLBACK_CHARSET: &str = "WIN1252";

/// Folders inside folders: more than this is a loop in the data
const MAX_DEPTH: usize = 16;

/// `Key=Value` lines. A value may have `=` in it.
fn props(text: &str) -> HashMap<&str, &str> {
    text.lines().filter_map(|line| line.split_once('=')).map(|(k, v)| (k.trim(), v.trim())).collect()
}

/// "host", "host/3051" or "host/service": IBExpert's server name
fn server(name: &str) -> (String, u16) {
    let (host, port) = match name.split_once('/') {
        Some((host, port)) => (host, port.trim().parse().unwrap_or(3050)),
        None => (name, 3050),
    };
    let host = host.trim();
    (if host.is_empty() { "localhost".into() } else { host.into() }, port)
}

/// "C:\data\Sales.FDB" → "SALES", for a database IBExpert has no alias for
fn alias_from(database: &str) -> String {
    let file = database.rsplit(['\\', '/', ':']).next().unwrap_or(database);
    let stem = match file.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => file,
    };
    stem.to_uppercase()
}

/// A database row's registration. None when it names no database.
fn registration(text: &str) -> Option<(Registration, Option<String>)> {
    let p = props(text);
    let get = |key: &str| p.get(key).copied().unwrap_or_default();
    let database = get("DBName");
    if database.is_empty() {
        return None;
    }
    let (host, port) = server(get("SrvName"));
    let theirs = get("Charset").to_uppercase();
    let known = fb_core::charset_names().any(|c| c == theirs);
    let name = match get("Alias") {
        "" => alias_from(database),
        alias => alias.to_string(),
    };
    let user = match get("UserName") {
        "" => "SYSDBA",
        user => user,
    };
    let registration = Registration {
        id: String::new(),
        name,
        folder: Vec::new(),
        save_password: false,
        engine: Engine::Firebird(FirebirdSettings {
            host,
            port,
            database: database.to_string(),
            user: user.to_string(),
            role: get("Role").to_string(),
            charset: if known { theirs.clone() } else { FALLBACK_CHARSET.to_string() },
            // IBExpert keeps neither: it asks the database for its dialect,
            // and its client library settles the authentication
            dialect: Dialect::D3,
            auth: Auth::Srp,
        }),
    };
    Some((registration, Some(theirs).filter(|_| !known)))
}

fn same_database(a: &Registration, b: &Registration) -> bool {
    let (Engine::Firebird(a), Engine::Firebird(b)) = (&a.engine, &b.engine);
    a.port == b.port
        && a.host.trim().eq_ignore_ascii_case(b.host.trim())
        && a.database.trim().eq_ignore_ascii_case(b.database.trim())
}

/// The tree's databases, as IBExpert's explorer lists them
fn databases(rows: &[Row], existing: &[Registration]) -> Vec<FoundDatabase> {
    fn walk(rows: &[Row], parent: i64, path: &[&str], depth: usize, existing: &[Registration], out: &mut Vec<FoundDatabase>) {
        if depth > MAX_DEPTH {
            return;
        }
        let mut children: Vec<&Row> = rows.iter().filter(|r| r.parent == parent && r.id != parent).collect();
        children.sort_by_key(|r| (r.order, r.id));
        for row in children {
            if row.folder {
                let caption = props(&row.props).get("FolderCaption").copied().unwrap_or_default();
                let mut inner = path.to_vec();
                if !caption.is_empty() {
                    inner.push(caption);
                }
                walk(rows, row.id, &inner, depth + 1, existing, out);
            } else if let Some((mut registration, charset_replaced)) = registration(&row.props) {
                registration.folder = path.iter().map(|name| name.to_string()).collect();
                out.push(FoundDatabase {
                    registered: existing.iter().any(|e| same_database(e, &registration)),
                    registration,
                    charset_replaced,
                });
            }
        }
    }
    let mut out = Vec::new();
    walk(rows, 0, &[], 0, existing, &mut out);
    out
}

/// IBExpert's `CodeFormatOptions`, an INI, as a style. IBExpert has a
/// setting for each clause of each statement; this formatter has a few for
/// all of them, so what comes over is what has a counterpart, and the rest
/// stays at its default. None when the text has none of them.
fn style(text: &str) -> Option<Style> {
    let mut section = "";
    let mut ini: HashMap<(&str, &str), &str> = HashMap::new();
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name;
        } else if let Some((key, value)) = line.split_once('=') {
            ini.insert((section, key.trim()), value.trim());
        }
    }
    let number = |section: &str, key: &str| ini.get(&(section, key)).and_then(|v| v.parse::<i64>().ok());
    let flag = |section: &str, key: &str| number(section, key).map(|n| n != 0);
    // 0: as written, 1: lower, 2: upper, 3: first letter upper, which this
    // formatter does not do
    let case = |key: &str| {
        number("General", key).map(|n| match n {
            1 => Case::Lower,
            2 => Case::Upper,
            _ => Case::Keep,
        })
    };
    let defaults = Style::default();
    let keywords = case("KeywordCharCase");
    let identifiers = case("IdentCharCase");
    let one_per_line = flag("Select", "OneColumnInLine");
    let break_conditions = flag("Where", "CRLFBeforeAND");
    let break_join_on = flag("JoinOn", "CRLFBeforeON");
    let space_after_comma = flag("General", "SpaceAfterComma");
    let indent = number("General", "BlockIndent");
    let right_margin = number("General", "RightMargin");
    let flags = [one_per_line, break_conditions, break_join_on, space_after_comma];
    if keywords.or(identifiers).is_none() && flags.iter().all(Option::is_none) && indent.or(right_margin).is_none() {
        return None;
    }
    Some(Style {
        keywords: keywords.unwrap_or(defaults.keywords),
        identifiers: identifiers.unwrap_or(defaults.identifiers),
        // The limits are the formatter dialog's
        indent: indent.map_or(defaults.indent, |n| n.clamp(1, 8) as usize),
        right_margin: right_margin.map_or(defaults.right_margin, |n| n.clamp(40, 400) as usize),
        select_list: one_per_line.map_or(defaults.select_list, |one| if one { Lists::OnePerLine } else { Lists::Wrap }),
        break_conditions: break_conditions.unwrap_or(defaults.break_conditions),
        break_join_on: break_join_on.unwrap_or(defaults.break_join_on),
        space_after_comma: space_after_comma.unwrap_or(defaults.space_after_comma),
        ..defaults
    })
}

/// A blob IBExpert wrote: UTF-8 from its Unicode versions, the Windows code
/// page from the older ones, read here as Latin-1
fn text(value: &Value) -> String {
    match value {
        Value::Text(text) => text.clone(),
        Value::Binary(bytes) => {
            let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
            match std::str::from_utf8(bytes) {
                Ok(text) => text.to_string(),
                Err(_) => bytes.iter().map(|&b| b as char).collect(),
            }
        }
        _ => String::new(),
    }
}

fn integer(value: &Value) -> i64 {
    match value {
        Value::Integer(n) => *n,
        _ => 0,
    }
}

fn scan(conn: &Connection, existing: &[Registration]) -> AppResult<Found> {
    let tables = conn.query(
        "select trim(rdb$relation_name) from rdb$relations where rdb$relation_name in ('DATABASES', 'CONFIG')",
        vec![],
    )?;
    let has = |name: &str| tables.rows.iter().any(|row| text(&row[0]) == name);
    if !has("DATABASES") {
        return Err(AppError::Invalid {
            message: "This is not an IBExpert User Database: it has no DATABASES table.".into(),
        });
    }
    let rows: Vec<Row> = conn
        .query("select id, parent_id, rec_type, db_order, props from databases", vec![])?
        .rows
        .iter()
        .map(|row| Row {
            id: integer(&row[0]),
            parent: integer(&row[1]),
            folder: integer(&row[2]) == 1,
            order: integer(&row[3]),
            props: text(&row[4]),
        })
        .collect();
    let style = if has("CONFIG") {
        let options = conn.query("select data from config where ident = 'CodeFormatOptions'", vec![])?;
        options.rows.first().and_then(|row| style(&text(&row[0])))
    } else {
        None
    };
    Ok(Found { databases: databases(&rows, existing), style })
}

/// Connects to an IBExpert User Database with the dialog's settings and
/// reports what it has. Nothing is saved: the dialog shows it first.
#[tauri::command]
pub async fn ibexpert_scan(app: AppHandle, source: Registration, password: String) -> AppResult<Found> {
    blocking(app, move |state| {
        let conn = open(&source, password)?;
        let existing = state.registry.lock().unwrap().all().to_vec();
        scan(&conn, &existing)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database(id: i64, parent: i64, order: i64, props: &str) -> Row {
        Row { id, parent, folder: false, order, props: props.into() }
    }

    fn folder(id: i64, parent: i64, order: i64, caption: &str) -> Row {
        Row { id, parent, folder: true, order, props: format!("FolderCaption={caption}\r\nSrvVer=16\r\n") }
    }

    fn settings(r: &Registration) -> &FirebirdSettings {
        let Engine::Firebird(settings) = &r.engine;
        settings
    }

    const SALES: &str = "DBType=0\r\nSrvType=1\r\nSrvName=db.example/3051\r\nProtocol=0\r\nDBName=C:\\data\\Sales.FDB\r\n\
        Alias=Sales\r\nUserName=APP\r\nPassword=xxxxxxxx\r\nCharset=win1252\r\nRole=READER\r\n";

    #[test]
    fn a_database_row_is_a_registration_without_its_password() {
        let (r, replaced) = registration(SALES).unwrap();
        assert_eq!(r.name, "Sales");
        assert!(r.id.is_empty() && !r.save_password);
        let fb = settings(&r);
        assert_eq!((fb.host.as_str(), fb.port), ("db.example", 3051));
        assert_eq!(fb.database, r"C:\data\Sales.FDB");
        assert_eq!((fb.user.as_str(), fb.role.as_str(), fb.charset.as_str()), ("APP", "READER", "WIN1252"));
        assert_eq!(replaced, None);
        r.validate().unwrap();
    }

    #[test]
    fn what_ibexpert_left_out_takes_a_default() {
        let (r, replaced) = registration("DBName=/srv/fb/stock.fdb\nSrvName=\nCharset=NONE\nAlias=\nUserName=\n").unwrap();
        assert_eq!(r.name, "STOCK");
        let fb = settings(&r);
        assert_eq!((fb.host.as_str(), fb.port, fb.user.as_str()), ("localhost", 3050, "SYSDBA"));
        assert_eq!(fb.charset, FALLBACK_CHARSET);
        assert_eq!(replaced.as_deref(), Some("NONE"));
        r.validate().unwrap();

        // A service name is not a port; an alias has no file name to cut
        assert_eq!(server("host/gds_db"), ("host".to_string(), 3050));
        assert_eq!(alias_from("employee"), "EMPLOYEE");
        assert!(registration("Alias=Nothing\nDBName=\n").is_none());
    }

    #[test]
    fn the_tree_comes_in_the_explorers_order_with_its_folders() {
        let rows = [
            database(9, 2, 2, "DBName=b.fdb\nAlias=B\n"),
            folder(2, 1, 1, "North"),
            database(8, 2, 1, "DBName=a.fdb\nAlias=A\n"),
            folder(1, 0, 2, "Customers"),
            database(7, 0, 1, "DBName=first.fdb\nAlias=First\n"),
            database(6, 1, 0, "Alias=No database\n"),
            // A row under itself, and one under nothing: neither is reached
            folder(20, 20, 0, "Loop"),
            database(21, 99, 0, "DBName=lost.fdb\n"),
        ];
        let (registered, _) = registration("DBName=A.FDB\nSrvName=LOCALHOST\n").unwrap();
        let found = databases(&rows, &[registered]);
        let listed: Vec<_> =
            found.iter().map(|d| (d.registration.name.as_str(), d.registration.folder.join(" / "), d.registered)).collect();
        let under = |name, folder: &str, registered| (name, folder.to_string(), registered);
        assert_eq!(
            listed,
            [under("First", "", false), under("A", "Customers / North", true), under("B", "Customers / North", false)]
        );
    }

    #[test]
    fn the_formatter_options_with_a_counterpart_come_over() {
        let ini = "[General]\r\nBlockIndent=4\r\nIdentCharCase=0\r\nKeywordCharCase=2\r\nSpaceAfterComma=0\r\nRightMargin=120\r\n\r\n\
            [Select]\r\nCRLFBeforeFROM=1\r\nOneColumnInLine=0\r\n\r\n[Where]\r\nCRLFBeforeAND=0\r\n\r\n[JoinOn]\r\nCRLFBeforeAND=1\r\nCRLFBeforeON=1\r\n";
        assert_eq!(
            style(ini),
            Some(Style {
                keywords: Case::Upper,
                identifiers: Case::Keep,
                indent: 4,
                right_margin: 120,
                select_list: Lists::Wrap,
                break_conditions: false,
                break_join_on: true,
                space_after_comma: false,
                ..Style::default()
            })
        );
        // Some of them, and out of range: the rest is the default's
        assert_eq!(
            style("[General]\nKeywordCharCase=1\nBlockIndent=40\n"),
            Some(Style { keywords: Case::Lower, indent: 8, ..Style::default() })
        );
        assert_eq!(style("[Select]\nCRLFBeforeFROM=1\n"), None);
        assert_eq!(style(""), None);
    }

    #[test]
    fn a_blob_is_read_as_utf8_or_else_as_latin1() {
        assert_eq!(text(&Value::Binary(b"\xEF\xBB\xBFAlias=Caf\xC3\xA9".to_vec())), "Alias=Café");
        assert_eq!(text(&Value::Binary(b"Alias=Caf\xE9".to_vec())), "Alias=Café");
        assert_eq!(text(&Value::Null), "");
    }
}
