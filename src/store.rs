//! Saved connections, by name: a local API address or a MyJDownloader account (derived secrets
//! and the current session). Kept in Canopy's plugin folder when run by Canopy, else in the
//! user's configuration folder; encrypted at rest when a key is given.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::api::CloudAccount;
use crate::crypto;
use crate::error::Error;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Connection {
    Local { url: String },
    Cloud(CloudAccount),
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub connections: BTreeMap<String, Connection>,
}

const SEALED_PREFIX: &str = "jdctl-sealed:v1:";

/// The folder jdctl keeps its file in.
pub fn data_dir() -> Result<PathBuf, Error> {
    if let Some(dir) = std::env::var_os("CANOPY_PLUGIN_DATA") {
        return Ok(PathBuf::from(dir));
    }
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(dir).join("jdctl"));
    }
    if let Some(dir) = std::env::var_os("APPDATA") {
        return Ok(PathBuf::from(dir).join("jdctl"));
    }
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".config").join("jdctl"))
        .ok_or_else(|| Error::Config("no folder to keep connections in (set JDCTL_DATA)".into()))
}

/// The at-rest key: Canopy's per-plugin key, or one of the person's own.
fn sealing_key() -> Option<crypto::Token> {
    ["CANOPY_PLUGIN_KEY", "JDCTL_KEY"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
        .map(|key| crypto::sha256(&[b"jdctl.store", key.as_bytes()]))
}

/// The file's text: sealed with the key when there is one.
pub fn seal(json: &str, key: Option<&crypto::Token>) -> String {
    match key {
        Some(key) => format!("{SEALED_PREFIX}{}", crypto::encrypt(key, json)),
        None => json.to_owned(),
    }
}

pub fn unseal(text: &str, key: Option<&crypto::Token>) -> Result<String, Error> {
    match (text.strip_prefix(SEALED_PREFIX), key) {
        (Some(sealed), Some(key)) => crypto::decrypt(key, sealed)
            .map_err(|_| Error::Config("saved connections cannot be opened with this key".into())),
        (Some(_), None) => Err(Error::Config(
            "saved connections are encrypted: the key is missing".into(),
        )),
        (None, _) => Ok(text.to_owned()),
    }
}

impl Store {
    fn path() -> Result<PathBuf, Error> {
        Ok(data_dir()?.join("connections"))
    }

    pub fn load() -> Result<Self, Error> {
        let path = Self::path()?;
        let Ok(text) = fs::read_to_string(&path) else {
            return Ok(Self::default());
        };
        let json = unseal(&text, sealing_key().as_ref())?;
        serde_json::from_str(&json)
            .map_err(|_| Error::Config("saved connections are damaged".into()))
    }

    pub fn save(&self) -> Result<(), Error> {
        let path = Self::path()?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|error| {
                Error::Config(format!("cannot create {}: {error}", dir.display()))
            })?;
        }
        let json = serde_json::to_string_pretty(self).expect("connections serialize");
        let text = seal(&json, sealing_key().as_ref());
        let temporary = path.with_extension("tmp");
        fs::write(&temporary, text)
            .map_err(|error| Error::Config(format!("cannot save connections: {error}")))?;
        restrict(&temporary);
        fs::rename(&temporary, &path)
            .map_err(|error| Error::Config(format!("cannot save connections: {error}")))
    }

    /// The named connection, or the default one, or the only one.
    pub fn pick(&self, name: Option<&str>) -> Result<(String, Connection), Error> {
        let chosen = match name {
            Some(name) => name.to_owned(),
            None => match (&self.default, self.connections.len()) {
                (Some(default), _) => default.clone(),
                (None, 1) => self.connections.keys().next().cloned().unwrap_or_default(),
                (None, 0) => {
                    return Err(Error::Config(
                        "no connection yet: add one with `jdctl connect`".into(),
                    ));
                }
                (None, _) => {
                    return Err(Error::Config(
                        "several connections: pick one with --connection".into(),
                    ));
                }
            },
        };
        self.connections
            .get(&chosen)
            .cloned()
            .map(|connection| (chosen.clone(), connection))
            .ok_or_else(|| Error::Config(format!("no connection named \"{chosen}\"")))
    }
}

/// Only the owner may read the file (it holds account secrets).
#[cfg(unix)]
fn restrict(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict(_path: &std::path::Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seals_and_unseals_with_the_key() {
        let key = crypto::sha256(&[b"k"]);
        let sealed = seal("{\"a\":1}", Some(&key));
        assert!(sealed.starts_with(SEALED_PREFIX));
        assert_eq!(unseal(&sealed, Some(&key)).unwrap(), "{\"a\":1}");
        assert!(unseal(&sealed, None).is_err());
        assert!(unseal(&sealed, Some(&crypto::sha256(&[b"other"]))).is_err());
        assert_eq!(seal("{}", None), "{}");
        assert_eq!(unseal("{}", Some(&key)).unwrap(), "{}");
    }

    #[test]
    fn picks_the_named_default_or_only_connection() {
        let mut store = Store::default();
        assert!(store.pick(None).is_err());
        store.connections.insert(
            "home".into(),
            Connection::Local {
                url: "http://jd:3128".into(),
            },
        );
        assert_eq!(store.pick(None).unwrap().0, "home");
        store.connections.insert(
            "cloud".into(),
            Connection::Local {
                url: "http://x".into(),
            },
        );
        assert!(store.pick(None).is_err());
        store.default = Some("cloud".into());
        assert_eq!(store.pick(None).unwrap().0, "cloud");
        assert_eq!(store.pick(Some("home")).unwrap().0, "home");
        assert!(store.pick(Some("missing")).is_err());
    }

    #[test]
    fn round_trips_both_kinds_as_json() {
        let mut store = Store::default();
        store.connections.insert(
            "home".into(),
            Connection::Local {
                url: "http://jd:3128".into(),
            },
        );
        let json = serde_json::to_string(&store).unwrap();
        assert!(json.contains(r#""kind":"local""#));
        let back: Store = serde_json::from_str(&json).unwrap();
        assert_eq!(back.connections.len(), 1);
    }
}
