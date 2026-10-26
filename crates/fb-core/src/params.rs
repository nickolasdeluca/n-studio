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
}
