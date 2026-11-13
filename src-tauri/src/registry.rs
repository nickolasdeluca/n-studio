//! Database registrations: what the explorer lists, stored as JSON in the
//! app's config directory. Passwords never go in the file; see `secrets`.

use std::fs;
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
    /// The explorer's folders it is under, outermost first; none is the
    /// top. A folder is there while a registration names it.
    #[serde(default)]
    pub folder: Vec<String>,
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

/// A folder path as it is kept: each name trimmed, the empty ones gone
fn folder_path(folder: &[String]) -> Vec<String> {
    folder.iter().map(|name| name.trim().to_string()).filter(|name| !name.is_empty()).collect()
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
        registration.folder = folder_path(&registration.folder);
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

    /// Puts a registration in `folder`, beside another one: before `anchor`,
    /// or after it. Without an anchor it goes last in the folder. This is
    /// what a drag in the explorer does, so the order kept is the order
    /// shown.
    pub fn place(&mut self, id: &str, folder: &[String], anchor: Option<&str>, after: bool) -> AppResult<()> {
        let folder = folder_path(folder);
        if let Some(anchor) = anchor {
            self.get(anchor)?;
        }
        let from = self
            .registrations
            .iter()
            .position(|r| r.id == id)
            .ok_or_else(|| AppError::NotFound { message: format!("No registration {id}.") })?;
        if anchor == Some(id) {
            self.registrations[from].folder = folder;
            return self.write();
        }
        let mut registration = self.registrations.remove(from);
        let at = match anchor {
            Some(anchor) => {
                self.registrations.iter().position(|r| r.id == anchor).expect("the anchor is there") + usize::from(after)
            }
            // After the last one under the folder, the ones in folders
            // inside it too; a new folder starts where the one it is in ends
            None => (0..=folder.len())
                .rev()
                .find_map(|depth| self.registrations.iter().rposition(|r| r.folder.starts_with(&folder[..depth])))
                .map_or(0, |last| last + 1),
        };
        registration.folder = folder;
        self.registrations.insert(at, registration);
        self.write()
    }

    /// Renames or moves a folder: every registration under `from`, however
    /// deep, goes under `to` instead, in one write. An empty `to` takes
    /// them to the top. Returns how many registrations it moved.
    pub fn move_folder(&mut self, from: &[String], to: &[String]) -> AppResult<usize> {
        let (from, to) = (folder_path(from), folder_path(to));
        if from.is_empty() {
            return Err(AppError::Invalid { message: "Name the folder to rename.".into() });
        }
        let mut moved = 0;
        for registration in self.registrations.iter_mut().filter(|r| r.folder.starts_with(&from)) {
            registration.folder.splice(..from.len(), to.iter().cloned());
            moved += 1;
        }
        if moved > 0 && from != to {
            self.write()?;
        }
        Ok(moved)
    }

    pub fn remove(&mut self, id: &str) -> AppResult<()> {
        self.get(id)?;
        self.registrations.retain(|r| r.id != id);
        self.write()
    }

    fn write(&self) -> AppResult<()> {
        let json = serde_json::to_vec_pretty(&File { version: VERSION, registrations: self.registrations.clone() })
            .expect("registrations serialize");
        crate::files::replace(&self.path, &json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Registration {
        Registration {
            id: String::new(),
            name: " Local ".into(),
            folder: Vec::new(),
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
    fn a_folder_is_kept_trimmed_and_a_file_without_one_still_reads() {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = Registry::load(dir.path()).unwrap();
        let mut r = sample();
        r.folder = vec![" Customers ".into(), "".into(), "North".into()];
        let saved = registry.save(r).unwrap();
        assert_eq!(saved.folder, ["Customers", "North"]);
        assert_eq!(Registry::load(dir.path()).unwrap().get(&saved.id).unwrap().folder, ["Customers", "North"]);

        // Written before there were folders
        let old = r#"{"version":1,"registrations":[{"id":"a","name":"Old","savePassword":false,"engine":"firebird",
            "host":"localhost","port":3050,"database":"employee","user":"SYSDBA","charset":"UTF8","dialect":"3"}]}"#;
        std::fs::write(dir.path().join("registrations.json"), old).unwrap();
        assert!(Registry::load(dir.path()).unwrap().get("a").unwrap().folder.is_empty());
    }

    #[test]
    fn a_folder_is_renamed_for_everything_under_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = Registry::load(dir.path()).unwrap();
        let path = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        let mut ids = Vec::new();
        for folder in [path(&["Customers"]), path(&["Customers", "North"]), path(&["Customers 2"]), path(&[])] {
            ids.push(registry.save(Registration { folder, ..sample() }).unwrap().id);
        }
        // A folder inside goes along; one that only starts alike stays
        assert_eq!(registry.move_folder(&path(&["Customers"]), &path(&[" Clients "])).unwrap(), 2);
        let reloaded = Registry::load(dir.path()).unwrap();
        let folders: Vec<_> = ids.iter().map(|id| reloaded.get(id).unwrap().folder.join("/")).collect();
        assert_eq!(folders, ["Clients", "Clients/North", "Customers 2", ""]);

        // Into another folder, and out to the top
        assert_eq!(registry.move_folder(&path(&["Clients", "North"]), &path(&["Regions", "North"])).unwrap(), 1);
        assert_eq!(registry.get(&ids[1]).unwrap().folder, ["Regions", "North"]);
        assert_eq!(registry.move_folder(&path(&["Regions"]), &[]).unwrap(), 1);
        assert_eq!(registry.get(&ids[1]).unwrap().folder, ["North"]);
        assert_eq!(registry.move_folder(&path(&["Nowhere"]), &path(&["X"])).unwrap(), 0);
        assert!(registry.move_folder(&[], &path(&["X"])).is_err());
    }

    #[test]
    fn a_registration_is_placed_in_a_folder_beside_another() {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = Registry::load(dir.path()).unwrap();
        let path = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        let mut add = |name: &str, folder: &[&str]| {
            registry.save(Registration { name: name.into(), folder: path(folder), ..sample() }).unwrap().id
        };
        let (a, b, c, d) = (add("A", &["X"]), add("B", &["X", "Y"]), add("C", &[]), add("D", &[]));
        let order = |registry: &Registry| {
            registry.all().iter().map(|r| format!("{}:{}", r.name, r.folder.join("/"))).collect::<Vec<_>>().join(" ")
        };

        // Out of its folder, before and after one at the top
        registry.place(&b, &[], Some(&d), false).unwrap();
        assert_eq!(order(&registry), "A:X C: B: D:");
        registry.place(&b, &[], Some(&d), true).unwrap();
        assert_eq!(order(&registry), "A:X C: D: B:");
        // Into a folder: last in it, the folders inside it counted
        registry.place(&c, &path(&["X", "Y"]), None, false).unwrap();
        registry.place(&d, &path(&["X"]), None, false).unwrap();
        assert_eq!(order(&registry), "A:X C:X/Y D:X B:");
        // To the top without an anchor: last of all
        registry.place(&a, &[], None, false).unwrap();
        assert_eq!(order(&registry), "C:X/Y D:X B: A:");
        // Onto itself is only the folder; what is not there is refused and changes nothing
        registry.place(&a, &path(&["X"]), Some(&a), true).unwrap();
        assert!(registry.place(&a, &[], Some("nobody"), false).is_err());
        assert!(registry.place("nobody", &[], None, false).is_err());
        assert_eq!(order(&registry), "C:X/Y D:X B: A:X");
        assert_eq!(order(&Registry::load(dir.path()).unwrap()), "C:X/Y D:X B: A:X");
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
