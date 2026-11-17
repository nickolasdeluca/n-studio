use rsfbclient_core::{charset, Charset};
use serde::{Deserialize, Serialize};

pub use rsfbclient_rust::AuthPlugin;

/// SQL dialect the connection speaks. Dialect 1 is the InterBase 5 dialect,
/// still found in old databases.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dialect {
    #[serde(rename = "1")]
    D1,
    #[default]
    #[serde(rename = "3")]
    D3,
}

impl From<Dialect> for rsfbclient_core::Dialect {
    fn from(d: Dialect) -> Self {
        match d {
            Dialect::D1 => rsfbclient_core::Dialect::D1,
            Dialect::D3 => rsfbclient_core::Dialect::D3,
        }
    }
}

/// Everything needed to attach to a database
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectParams {
    pub host: String,
    pub port: u16,
    /// Path on the server, or an alias
    pub database: String,
    pub user: String,
    pub password: String,
    pub role: Option<String>,
    /// Connection character set, by its Firebird name: see [`charset_names`]
    pub charset: String,
    pub dialect: Dialect,
    pub auth: AuthPlugin,
}

impl Default for ConnectParams {
    fn default() -> Self {
        Self {
            host: "localhost".into(),
            port: 3050,
            database: String::new(),
            user: "SYSDBA".into(),
            password: String::new(),
            role: None,
            // Brazilian legacy databases are almost all WIN1252
            charset: "WIN1252".into(),
            dialect: Dialect::D3,
            auth: AuthPlugin::Srp,
        }
    }
}

impl ConnectParams {
    /// From `FB_HOST`, `FB_PORT`, `FB_DB`, `FB_USER`, `FB_PASS`, and optionally
    /// `FB_CHARSET`, `FB_ROLE` and `FB_AUTH=legacy`; `None` without `FB_DB`.
    /// For tests and tools, which point at a server through the environment.
    pub fn from_env() -> Option<Self> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let defaults = Self::default();
        Some(Self {
            database: var("FB_DB")?,
            host: var("FB_HOST").unwrap_or(defaults.host),
            port: var("FB_PORT").and_then(|p| p.parse().ok()).unwrap_or(defaults.port),
            user: var("FB_USER").unwrap_or(defaults.user),
            password: var("FB_PASS").unwrap_or_default(),
            role: var("FB_ROLE"),
            charset: var("FB_CHARSET").unwrap_or(defaults.charset),
            dialect: Dialect::D3,
            auth: match var("FB_AUTH").as_deref() {
                Some("legacy") => AuthPlugin::Legacy,
                _ => AuthPlugin::Srp,
            },
        })
    }
}

impl ConnectParams {
    /// These parameters pointed at what a script's `CONNECT` names: its
    /// database, and its user, password and role where it says them; the
    /// rest (auth, dialect, and what it leaves out) stays. `charset` is the
    /// script's `SET NAMES`, if it had one.
    pub fn with_connect(&self, target: &fb_sql::isql::Connect, charset: Option<&str>) -> Self {
        let (host, port, database) = split_database(&target.database);
        let mut params = self.clone();
        params.host = host.unwrap_or_else(|| "localhost".into());
        params.port = port.unwrap_or(Self::default().port);
        params.database = database;
        if let Some(user) = &target.user {
            params.user = user.clone();
        }
        if let Some(password) = &target.password {
            params.password = password.clone();
        }
        if target.role.is_some() {
            params.role = target.role.clone();
        }
        if let Some(charset) = charset {
            params.charset = charset.to_string();
        }
        params
    }
}

/// A connection string as isql takes it, as host, port and the database's
/// path or alias: `host/port:path`, `host:path`, `inet://host:port/path`,
/// or a path or alias alone (on this machine). `C:\…` is a drive, not a
/// host.
pub fn split_database(s: &str) -> (Option<String>, Option<u16>, String) {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("inet://").or_else(|| s.strip_prefix("inet4://")).or_else(|| s.strip_prefix("inet6://")) {
        let (server, path) = rest.split_once('/').unwrap_or((rest, ""));
        let (host, port) = match server.rsplit_once(':') {
            Some((host, port)) if port.parse::<u16>().is_ok() => (host, port.parse().ok()),
            _ => (server, None),
        };
        return (Some(host.to_string()).filter(|h| !h.is_empty()), port, path.to_string());
    }
    let Some((server, path)) = s.split_once(':') else { return (None, None, s.to_string()) };
    // A drive letter: C:\data or C:/data
    if server.len() == 1 && server.chars().all(|c| c.is_ascii_alphabetic()) {
        return (None, None, s.to_string());
    }
    let (host, port) = match server.split_once('/') {
        Some((host, port)) => (host, port.parse().ok()),
        None => (server, None),
    };
    (Some(host.to_string()), port, path.to_string())
}

/// Connection character sets, by Firebird name. `NONE` is not among them:
/// with it the server sends bytes untranslated and the client has to guess.
const CHARSETS: &[(&str, Charset)] = &[
    ("UTF8", charset::UTF_8),
    ("WIN1252", charset::WIN_1252),
    ("ISO8859_1", charset::ISO_8859_1),
    ("WIN1250", charset::WIN_1250),
    ("WIN1251", charset::WIN_1251),
    ("WIN1253", charset::WIN_1253),
    ("WIN1254", charset::WIN_1254),
    ("WIN1256", charset::WIN_1256),
    ("WIN1257", charset::WIN_1257),
    ("WIN1258", charset::WIN_1258),
    ("ISO8859_2", charset::ISO_8859_2),
    ("ISO8859_3", charset::ISO_8859_3),
    ("ISO8859_4", charset::ISO_8859_4),
    ("ISO8859_5", charset::ISO_8859_5),
    ("ISO8859_6", charset::ISO_8859_6),
    ("ISO8859_7", charset::ISO_8859_7),
    ("ISO8859_13", charset::ISO_8859_13),
    ("KOI8R", charset::KOI8_R),
    ("KOI8U", charset::KOI8_U),
    ("EUCJ_0208", charset::EUC_JP),
    ("BIG_5", charset::BIG5_2003),
    ("ASCII", charset::ASCII),
];

/// The connection character sets a registration can pick from
pub fn charset_names() -> impl Iterator<Item = &'static str> {
    CHARSETS.iter().map(|(name, _)| *name)
}

pub(crate) fn charset_by_name(name: &str) -> Option<Charset> {
    let name = name.trim();
    CHARSETS
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, c)| c.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_charset_resolves() {
        for name in charset_names() {
            assert!(charset_by_name(name).is_some(), "{name}");
        }
        assert!(charset_by_name("win1252").is_some());
        assert!(charset_by_name("NONE").is_none());
    }

    #[test]
    fn connection_strings_split_as_isql_reads_them() {
        let split = |s: &str| {
            let (host, port, path) = split_database(s);
            (host.unwrap_or_default(), port, path)
        };
        assert_eq!(split("srv/3051:C:\\data\\x.fdb"), ("srv".into(), Some(3051), "C:\\data\\x.fdb".into()));
        assert_eq!(split("srv:/var/db/x.fdb"), ("srv".into(), None, "/var/db/x.fdb".into()));
        assert_eq!(split("C:\\data\\x.fdb"), ("".into(), None, "C:\\data\\x.fdb".into()));
        assert_eq!(split("employee"), ("".into(), None, "employee".into()));
        assert_eq!(split("inet://srv:3052/employee"), ("srv".into(), Some(3052), "employee".into()));
        assert_eq!(split("10.0.0.1:employee"), ("10.0.0.1".into(), None, "employee".into()));
    }

    #[test]
    fn a_connect_overrides_what_it_says() {
        let base = ConnectParams { password: "kept".into(), auth: AuthPlugin::Legacy, ..Default::default() };
        let target = fb_sql::isql::Connect {
            database: "srv/3051:db".into(),
            user: Some("u".into()),
            password: None,
            role: Some("r".into()),
        };
        let p = base.with_connect(&target, Some("UTF8"));
        assert_eq!((p.host.as_str(), p.port, p.database.as_str()), ("srv", 3051, "db"));
        assert_eq!((p.user.as_str(), p.password.as_str(), p.role.as_deref()), ("u", "kept", Some("r")));
        assert_eq!((p.charset.as_str(), p.auth), ("UTF8", AuthPlugin::Legacy));
    }
}
