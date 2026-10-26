//! Database registrations: what the explorer lists, stored as JSON in the
//! app's config directory. Passwords never go in the file; see `secrets`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use fb_core::{AuthPlugin, ConnectParams, Dialect};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Registration {
    /// Empty on a registration the UI has not saved yet
    #[serde(default)]
    pub id: String,
    /// What the explorer shows: an alias the user picks
    pub name: String,
    /// Whether the password lives in the OS keychain, or is asked on connect
    pub save_password: bool,
    #[serde(flatten)]
    pub engine: Engine,
}

/// The engine a registration connects to, and its own settings
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "engine", rename_all = "snake_case")]
pub enum Engine {
    Firebird(FirebirdSettings),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirebirdSettings {
    pub host: String,
    pub port: u16,
    /// Path on the server, or an alias from its databases.conf
    pub database: String,
    pub user: String,
    #[serde(default)]
    pub role: String,
    pub charset: String,
    pub dialect: Dialect,
    #[serde(default)]
    pub auth: Auth,
}

/// The authentication the handshake leads with
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Auth {
    #[default]
    Srp,
    Legacy,
}

impl FirebirdSettings {
    pub fn connect_params(&self, password: String) -> ConnectParams {
        ConnectParams {
            host: self.host.trim().to_string(),
            port: self.port,
            database: self.database.trim().to_string(),
            user: self.user.trim().to_string(),
            password,
            role: Some(self.role.trim().to_string()).filter(|r| !r.is_empty()),
            charset: self.charset.clone(),
            dialect: self.dialect,
            auth: match self.auth {
                Auth::Srp => AuthPlugin::Srp,
                Auth::Legacy => AuthPlugin::Legacy,
            },
        }
    }
}

impl Registration {
    /// What is wrong with it, for the dialog to show before saving
    pub fn validate(&self) -> AppResult<()> {
        let invalid = |message: &str| Err(AppError::Invalid { message: message.into() });
        if self.name.trim().is_empty() {
            return invalid("The registration needs a name.");
        }
        match &self.engine {
            Engine::Firebird(fb) => {
                if fb.database.trim().is_empty() {
                    return invalid("Enter the database path or alias.");
                }
                if fb.host.trim().is_empty() {
                    return invalid("Enter the server host.");
                }
                if fb.user.trim().is_empty() {
                    return invalid("Enter the user name.");
                }
                if !fb_core::charset_names().any(|c| c == fb.charset) {
                    return invalid("Pick a connection character set.");
                }
            }
        }
        Ok(())
    }
}

/// The registrations file. Every change is written through at once.
pub struct Registry {
    path: PathBuf,
    registrations: Vec<Registration>,
}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    registrations: Vec<Registration>,
}

const VERSION: u32 = 1;

impl Registry {
    /// Loads `registrations.json` from `dir`; none there is an empty list.
    pub fn load(dir: &Path) -> AppResult<Self> {
        let path = dir.join("registrations.json");
        let registrations = match fs::read(&path) {
            Ok(bytes) => {
                let file: File = serde_json::from_slice(&bytes).map_err(|e| AppError::Storage {
                    message: format!("{} is not readable: {e}", path.display()),
                })?;
                if file.version > VERSION {
                    return Err(AppError::Storage {
                        message: format!("{} was written by a newer N Studio", path.display()),
                    });
                }
                file.registrations
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(AppError::storage(&path, e)),
        };
        Ok(Self { path, registrations })
    }

    pub fn all(&self) -> &[Registration] {
        &self.registrations
    }

    pub fn get(&self, id: &str) -> AppResult<&Registration> {
        self.registrations
            .iter()
            .find(|r| r.id == id)
            .ok_or_else(|| AppError::NotFound { message: format!("No registration {id}.") })
    }

    /// Adds a registration without an id, or replaces the one with its id.
    /// Returns it as stored.
    pub fn save(&mut self, mut registration: Registration) -> AppResult<Registration> {
        registration.validate()?;
        registration.name = registration.name.trim().to_string();
        if registration.id.is_empty() {
            registration.id = uuid::Uuid::new_v4().to_string();
            self.registrations.push(registration.clone());
        } else {
            let slot = self
                .registrations
                .iter_mut()
                .find(|r| r.id == registration.id)
                .ok_or_else(|| AppError::NotFound { message: format!("No registration {}.", registration.id) })?;
            *slot = registration.clone();
        }
        self.write()?;
        Ok(registration)
    }

    pub fn remove(&mut self, id: &str) -> AppResult<()> {
        self.get(id)?;
        self.registrations.retain(|r| r.id != id);
        self.write()
    }

    /// Through a temporary file and a rename, so a crash mid-write cannot
    /// leave half a file behind.
    fn write(&self) -> AppResult<()> {
        let dir = self.path.parent().expect("the file lives in a directory");
        fs::create_dir_all(dir).map_err(|e| AppError::storage(dir, e))?;
        let json = serde_json::to_vec_pretty(&File { version: VERSION, registrations: self.registrations.clone() })
            .expect("registrations serialize");
        let tmp = self.path.with_extension("json.tmp");
        let mut file = fs::File::create(&tmp).map_err(|e| AppError::storage(&tmp, e))?;
        file.write_all(&json).and_then(|_| file.sync_all()).map_err(|e| AppError::storage(&tmp, e))?;
        fs::rename(&tmp, &self.path).map_err(|e| AppError::storage(&self.path, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Registration {
        Registration {
            id: String::new(),
            name: " Local ".into(),
            save_password: true,
            engine: Engine::Firebird(FirebirdSettings {
                host: "localhost".into(),
                port: 3050,
                database: "employee".into(),
                user: "SYSDBA".into(),
                role: String::new(),
                charset: "UTF8".into(),
                dialect: Dialect::D3,
                auth: Auth::Srp,
            }),
        }
    }

    #[test]
    fn saves_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = Registry::load(dir.path()).unwrap();
        assert!(registry.all().is_empty());

        let saved = registry.save(sample()).unwrap();
        assert!(!saved.id.is_empty());
        assert_eq!(saved.name, "Local");

        let mut renamed = saved.clone();
        renamed.name = "Renamed".into();
        registry.save(renamed).unwrap();

        let reloaded = Registry::load(dir.path()).unwrap();
        assert_eq!(reloaded.all().len(), 1);
        assert_eq!(reloaded.get(&saved.id).unwrap().name, "Renamed");

        let mut registry = reloaded;
        registry.remove(&saved.id).unwrap();
        assert!(Registry::load(dir.path()).unwrap().all().is_empty());
    }

    #[test]
    fn the_file_names_the_engine_and_never_a_password() {
        let dir = tempfile::tempdir().unwrap();
        Registry::load(dir.path()).unwrap().save(sample()).unwrap();
        let text = std::fs::read_to_string(dir.path().join("registrations.json")).unwrap();
        assert!(text.contains(r#""engine": "firebird""#), "{text}");
        assert!(text.contains(r#""dialect": "3""#), "{text}");
        assert!(!text.contains(r#""password""#), "{text}");
    }

    #[test]
    fn rejects_an_incomplete_registration() {
        let mut r = sample();
        r.name = "  ".into();
        assert!(matches!(r.validate(), Err(AppError::Invalid { .. })));
        let mut r = sample();
        let Engine::Firebird(fb) = &mut r.engine;
        fb.charset = "NONE".into();
        assert!(matches!(r.validate(), Err(AppError::Invalid { .. })));
    }
}
