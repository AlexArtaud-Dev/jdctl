//! JDownloader's calls jdctl uses, and the state it prints: downloads and the link grabber, as
//! packages holding links, with progress, speed, ETA and a simple state per row.

use serde::Serialize;
use serde_json::{Value, json};

use crate::api::Api;
use crate::error::Error;

/// Every row's coarse state, from JDownloader's flags and availability.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RowState {
    Running,
    Finished,
    Waiting,
    Disabled,
    Failed,
    Online,
    Offline,
    Unknown,
}

#[derive(Clone, Debug, Serialize)]
pub struct Row {
    /// `123` for a link, `pkg:123` style ids are not used: kind says which.
    pub id: i64,
    pub name: String,
    pub state: RowState,
    /// Status text JDownloader shows ("Downloading", "Waiting for connection"…), as given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub bytes_loaded: i64,
    pub bytes_total: i64,
    /// 0–1, or none when the size is unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    pub speed: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eta: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<Row>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Summary {
    /// Something is downloading (Canopy shows its header widget).
    pub active: bool,
    /// `running`, `paused`, `idle`, `stopped`… lower case.
    pub controller: String,
    pub speed: i64,
    pub bytes_loaded: i64,
    pub bytes_total: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eta: Option<i64>,
    pub running: usize,
    pub links: usize,
    pub grabber_links: usize,
    pub collecting: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct State {
    pub version: u8,
    pub summary: Summary,
    pub downloads: Vec<Row>,
    pub grabber: Vec<Row>,
}

fn int(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|text| !text.is_empty())
}

fn progress(loaded: i64, total: i64) -> Option<f64> {
    (total > 0).then(|| (loaded as f64 / total as f64).clamp(0.0, 1.0))
}

/// JDownloader's controller state in plain words: `RUNNING_STATE` → `running`, `PAUSE` → `paused`.
pub fn controller_state(raw: &str) -> String {
    let state = raw.to_lowercase();
    let state = state.trim_end_matches("_state");
    match state {
        "pause" => "paused".to_owned(),
        other => other.to_owned(),
    }
}

/// A download row's state, from JDownloader's flags (a status mentioning an error means failed).
pub fn download_state(row: &Value) -> RowState {
    let status = text(row, "status").unwrap_or_default().to_lowercase();
    if flag(row, "finished") {
        RowState::Finished
    } else if !flag(row, "enabled") {
        RowState::Disabled
    } else if flag(row, "running") {
        RowState::Running
    } else if ["error", "failed", "offline", "erreur", "échec"]
        .iter()
        .any(|word| status.contains(word))
    {
        RowState::Failed
    } else {
        RowState::Waiting
    }
}

/// A grabbed link's state, from its availability.
pub fn grabber_state(row: &Value) -> RowState {
    match row.get("availability").and_then(Value::as_str) {
        Some("ONLINE") => RowState::Online,
        Some("OFFLINE") => RowState::Offline,
        _ => RowState::Unknown,
    }
}

fn download_row(value: &Value, items: Vec<Row>) -> Row {
    let loaded = int(value, "bytesLoaded");
    let total = int(value, "bytesTotal");
    Row {
        id: int(value, "uuid"),
        name: text(value, "name").unwrap_or_default(),
        state: download_state(value),
        status: text(value, "status"),
        bytes_loaded: loaded,
        bytes_total: total,
        progress: progress(loaded, total),
        speed: int(value, "speed"),
        eta: value
            .get("eta")
            .and_then(Value::as_i64)
            .filter(|eta| *eta >= 0),
        host: text(value, "host"),
        folder: text(value, "saveTo"),
        enabled: flag(value, "enabled"),
        items,
    }
}

fn grabber_row(value: &Value, items: Vec<Row>) -> Row {
    let total = int(value, "bytesTotal");
    let state = if items.is_empty() {
        grabber_state(value)
    } else if items.iter().all(|item| item.state == RowState::Online) {
        RowState::Online
    } else if items.iter().all(|item| item.state == RowState::Offline) {
        RowState::Offline
    } else {
        RowState::Unknown
    };
    Row {
        id: int(value, "uuid"),
        name: text(value, "name").unwrap_or_default(),
        state,
        status: None,
        bytes_loaded: 0,
        bytes_total: total,
        progress: None,
        speed: 0,
        eta: None,
        host: text(value, "host"),
        folder: text(value, "saveTo"),
        enabled: value
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        items,
    }
}

/// Packages with their links, in JDownloader's order; links of unknown packages are dropped.
fn nest(
    packages: &[Value],
    links: &[Value],
    package_row: fn(&Value, Vec<Row>) -> Row,
    link_row: fn(&Value, Vec<Row>) -> Row,
) -> Vec<Row> {
    packages
        .iter()
        .map(|package| {
            let id = int(package, "uuid");
            let items = links
                .iter()
                .filter(|link| int(link, "packageUUID") == id)
                .map(|link| link_row(link, Vec::new()))
                .collect();
            package_row(package, items)
        })
        .collect()
}

pub struct Raw {
    pub controller: String,
    pub speed: i64,
    pub packages: Vec<Value>,
    pub links: Vec<Value>,
    pub grabber_packages: Vec<Value>,
    pub grabber_links: Vec<Value>,
    pub collecting: bool,
}

/// The printed state, from JDownloader's raw answers.
pub fn build_state(raw: Raw) -> State {
    let downloads = nest(&raw.packages, &raw.links, download_row, download_row);
    let grabber = nest(
        &raw.grabber_packages,
        &raw.grabber_links,
        grabber_row,
        grabber_row,
    );
    let pending: Vec<&Row> = downloads
        .iter()
        .filter(|row| row.state != RowState::Finished && row.enabled)
        .collect();
    let bytes_loaded: i64 = pending.iter().map(|row| row.bytes_loaded).sum();
    let bytes_total: i64 = pending.iter().map(|row| row.bytes_total).sum();
    let running = downloads
        .iter()
        .flat_map(|package| package.items.iter())
        .filter(|link| link.state == RowState::Running)
        .count();
    let controller = controller_state(&raw.controller);
    // A paused or stopped controller still reports a trickle of speed: no time left then.
    let eta = (controller == "running" && raw.speed > 0 && bytes_total > bytes_loaded)
        .then(|| (bytes_total - bytes_loaded) / raw.speed);
    State {
        version: 1,
        summary: Summary {
            active: running > 0 || controller == "running",
            controller,
            speed: raw.speed,
            bytes_loaded,
            bytes_total,
            progress: progress(bytes_loaded, bytes_total),
            eta,
            running,
            links: raw.links.len(),
            grabber_links: raw.grabber_links.len(),
            collecting: raw.collecting,
        },
        downloads,
        grabber,
    }
}

fn list(value: Value) -> Vec<Value> {
    match value {
        Value::Array(items) => items,
        _ => Vec::new(),
    }
}

/// Reads everything `state` prints, in six calls.
pub fn fetch_state(api: &mut dyn Api) -> Result<State, Error> {
    let controller = api.call("/downloadcontroller/getCurrentState", &[])?;
    let speed = api.call("/downloadcontroller/getSpeedInBps", &[])?;
    let packages = api.call(
        "/downloadsV2/queryPackages",
        &[json!({
            "bytesLoaded": true, "bytesTotal": true, "enabled": true, "eta": true, "finished": true,
            "running": true, "speed": true, "status": true, "childCount": true, "hosts": true,
            "saveTo": true, "priority": true, "maxResults": -1, "startAt": 0
        })],
    )?;
    let links = api.call(
        "/downloadsV2/queryLinks",
        &[json!({
            "bytesLoaded": true, "bytesTotal": true, "enabled": true, "eta": true, "finished": true,
            "running": true, "speed": true, "status": true, "host": true, "priority": true,
            "maxResults": -1, "startAt": 0
        })],
    )?;
    let grabber_packages = api.call(
        "/linkgrabberv2/queryPackages",
        &[json!({ "bytesTotal": true, "childCount": true, "hosts": true, "saveTo": true, "maxResults": -1, "startAt": 0 })],
    )?;
    let grabber_links = api.call(
        "/linkgrabberv2/queryLinks",
        &[json!({ "bytesTotal": true, "availability": true, "host": true, "enabled": true, "maxResults": -1, "startAt": 0 })],
    )?;
    let collecting = api
        .call("/linkgrabberv2/isCollecting", &[])
        .map(|value| value.as_bool().unwrap_or(false))
        .unwrap_or(false);
    Ok(build_state(Raw {
        controller: controller.as_str().unwrap_or("UNKNOWN").to_owned(),
        speed: speed.as_i64().unwrap_or(0),
        packages: list(packages),
        links: list(links),
        grabber_packages: list(grabber_packages),
        grabber_links: list(grabber_links),
        collecting,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw() -> Raw {
        Raw {
            controller: "RUNNING".into(),
            speed: 2_000,
            packages: vec![
                json!({"uuid": 1, "name": "Show", "bytesLoaded": 500, "bytesTotal": 1000, "enabled": true, "running": true, "saveTo": "/output/Show"}),
                json!({"uuid": 2, "name": "Done", "bytesLoaded": 10, "bytesTotal": 10, "enabled": true, "finished": true}),
            ],
            links: vec![
                json!({"uuid": 11, "packageUUID": 1, "name": "e01.mkv", "bytesLoaded": 500, "bytesTotal": 600, "enabled": true, "running": true, "speed": 2000, "eta": 50, "host": "example.com", "status": "Downloading"}),
                json!({"uuid": 12, "packageUUID": 1, "name": "e02.mkv", "bytesTotal": 400, "enabled": false}),
                json!({"uuid": 21, "packageUUID": 2, "name": "a.zip", "bytesLoaded": 10, "bytesTotal": 10, "enabled": true, "finished": true}),
                json!({"uuid": 99, "packageUUID": 404, "name": "orphan"}),
            ],
            grabber_packages: vec![json!({"uuid": 3, "name": "Found", "bytesTotal": 300})],
            grabber_links: vec![
                json!({"uuid": 31, "packageUUID": 3, "name": "x.rar", "availability": "ONLINE", "bytesTotal": 100}),
                json!({"uuid": 32, "packageUUID": 3, "name": "y.rar", "availability": "OFFLINE", "bytesTotal": 200}),
            ],
            collecting: true,
        }
    }

    #[test]
    fn nests_links_in_their_packages() {
        let state = build_state(raw());
        assert_eq!(state.downloads.len(), 2);
        assert_eq!(state.downloads[0].items.len(), 2);
        assert_eq!(state.downloads[0].items[0].state, RowState::Running);
        assert_eq!(state.downloads[0].items[1].state, RowState::Disabled);
        assert_eq!(state.downloads[1].state, RowState::Finished);
        assert_eq!(state.grabber[0].items.len(), 2);
        assert_eq!(state.grabber[0].state, RowState::Unknown);
    }

    #[test]
    fn sums_what_is_left_to_download() {
        let summary = build_state(raw()).summary;
        assert!(summary.active);
        assert_eq!(summary.controller, "running");
        assert_eq!((summary.bytes_loaded, summary.bytes_total), (500, 1000));
        assert_eq!(summary.progress, Some(0.5));
        assert_eq!(summary.eta, Some(0));
        assert_eq!(summary.running, 1);
        assert_eq!((summary.links, summary.grabber_links), (4, 2));
        assert!(summary.collecting);
    }

    #[test]
    fn reads_row_states() {
        assert_eq!(
            download_state(&json!({"enabled": true, "status": "Plugin error"})),
            RowState::Failed
        );
        assert_eq!(download_state(&json!({"enabled": true})), RowState::Waiting);
        assert_eq!(
            grabber_state(&json!({"availability": "ONLINE"})),
            RowState::Online
        );
        assert_eq!(
            grabber_state(&json!({"availability": "TEMP_UNKNOWN"})),
            RowState::Unknown
        );
        assert_eq!(progress(1, 0), None);
        assert_eq!(controller_state("STOPPED_STATE"), "stopped");
        assert_eq!(controller_state("RUNNING"), "running");
        assert_eq!(controller_state("PAUSE"), "paused");
    }

    #[test]
    fn stays_idle_without_work() {
        let state = build_state(Raw {
            controller: "IDLE".into(),
            speed: 0,
            packages: vec![],
            links: vec![],
            grabber_packages: vec![],
            grabber_links: vec![],
            collecting: false,
        });
        assert!(!state.summary.active);
        assert_eq!(state.summary.progress, None);
        let paused = build_state(Raw {
            controller: "PAUSE".into(),
            ..raw()
        });
        assert_eq!(paused.summary.eta, None);
        assert_eq!(state.summary.eta, None);
    }
}
