//! Two ways to reach JDownloader with the same calls (`/downloadsV2/queryLinks`, params as a JSON
//! list): its local API on the network, or MyJDownloader's encrypted relay.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::crypto::{self, Token};
use crate::error::Error;

/// One JDownloader API call: `path` like `/downloadcontroller/start`, positional params.
pub trait Api {
    fn call(&mut self, path: &str, params: &[Value]) -> Result<Value, Error>;
}

/// Percent-encodes a query component (RFC 3986 unreserved characters stay as they are).
pub fn encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .http_status_as_error(false)
        .build()
        .into()
}

fn read(
    response: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<(u16, String), Error> {
    let mut response = response.map_err(|error| Error::Network(error.to_string()))?;
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|error| Error::Network(error.to_string()))?;
    Ok((status, body))
}

/// An error body: `{"src":"MYJD","type":"AUTH_FAILED"}` or `{"type":"…"}`.
fn error_kind(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    value.get("type")?.as_str().map(str::to_owned)
}

/// JDownloader wraps results as `{"data": …}`; some answers come bare.
fn unwrap_data(value: Value) -> Value {
    match value {
        Value::Object(mut map) if map.contains_key("data") => {
            map.remove("data").unwrap_or(Value::Null)
        }
        other => other,
    }
}

/// The local API ("Deprecated API" in JDownloader's advanced settings): plain HTTP, no account.
pub struct LocalApi {
    base: String,
    agent: ureq::Agent,
}

impl LocalApi {
    pub fn new(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_owned(),
            agent: agent(),
        }
    }
}

/// `/path?param1&param2`, each param its URL-encoded JSON.
pub fn local_url(base: &str, path: &str, params: &[Value]) -> String {
    let query: Vec<String> = params
        .iter()
        .map(|param| encode_component(&param.to_string()))
        .collect();
    if query.is_empty() {
        format!("{base}{path}")
    } else {
        format!("{base}{path}?{}", query.join("&"))
    }
}

impl Api for LocalApi {
    fn call(&mut self, path: &str, params: &[Value]) -> Result<Value, Error> {
        let url = local_url(&self.base, path, params);
        let (status, body) = read(self.agent.get(&url).call())?;
        if status != 200 {
            return Err(Error::Api {
                status,
                kind: error_kind(&body).unwrap_or_else(|| "ERROR".into()),
            });
        }
        if body.trim().is_empty() {
            return Ok(Value::Null);
        }
        let value: Value =
            serde_json::from_str(&body).map_err(|_| Error::Protocol("answer is not JSON"))?;
        Ok(unwrap_data(value))
    }
}

const SERVER: &str = "https://api.jdownloader.org";
const APP_KEY: &str = "jdctl";

/// What a MyJDownloader connection keeps: derived secrets (never the password) and the session,
/// reused across runs until it expires.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CloudAccount {
    pub email: String,
    /// Hex of `sha256(email + password + "server")`.
    pub login_secret: String,
    /// Hex of `sha256(email + password + "device")`.
    pub device_secret: String,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    #[serde(default)]
    pub session: Option<CloudSession>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CloudSession {
    pub session_token: String,
    pub regain_token: String,
    /// Hex of the server encryption token for this session.
    pub server_token: String,
}

fn token_from_hex(value: &str) -> Result<Token, Error> {
    let bytes = hex::decode(value).map_err(|_| Error::Config("saved secret is damaged".into()))?;
    bytes
        .try_into()
        .map_err(|_| Error::Config("saved secret is damaged".into()))
}

pub struct CloudApi {
    pub account: CloudAccount,
    agent: ureq::Agent,
    rid: u64,
    /// Set when the session changed and should be saved.
    pub session_changed: bool,
}

#[derive(Debug, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    #[serde(rename = "type", default)]
    pub kind: String,
}

impl CloudApi {
    pub fn new(account: CloudAccount) -> Self {
        let rid = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(1);
        Self {
            account,
            agent: agent(),
            rid,
            session_changed: false,
        }
    }

    fn next_rid(&mut self) -> u64 {
        self.rid += 1;
        self.rid
    }

    /// A signed account call (`/my/…`), decrypted with `key`.
    fn account_call(
        &mut self,
        path: &str,
        params: &[(&str, &str)],
        key: &Token,
    ) -> Result<Value, Error> {
        let rid = self.next_rid();
        let mut query = format!("{path}?");
        for (name, value) in params {
            query.push_str(&format!("{name}={}&", encode_component(value)));
        }
        query.push_str(&format!("rid={rid}"));
        let signature = crypto::sign(key, &query);
        let url = format!("{SERVER}{query}&signature={signature}");
        let (status, body) = read(self.agent.get(&url).call())?;
        if status != 200 {
            let kind = error_kind(&body)
                .or_else(|| {
                    crypto::decrypt(key, &body)
                        .ok()
                        .and_then(|plain| error_kind(&plain))
                })
                .unwrap_or_else(|| "ERROR".into());
            return Err(Error::Api { status, kind });
        }
        let plain = crypto::decrypt(key, &body)?;
        serde_json::from_str(&plain).map_err(|_| Error::Protocol("account answer is not JSON"))
    }

    fn adopt_session(&mut self, answer: &Value, previous: &Token) -> Result<(), Error> {
        let session_token = answer["sessiontoken"]
            .as_str()
            .ok_or(Error::Protocol("no session token"))?;
        let regain_token = answer["regaintoken"]
            .as_str()
            .ok_or(Error::Protocol("no regain token"))?;
        let server = crypto::session_token(previous, session_token)?;
        self.account.session = Some(CloudSession {
            session_token: session_token.to_owned(),
            regain_token: regain_token.to_owned(),
            server_token: hex::encode(server),
        });
        self.session_changed = true;
        Ok(())
    }

    /// Signs in with the derived secrets (`/my/connect`).
    pub fn connect(&mut self) -> Result<(), Error> {
        let login = token_from_hex(&self.account.login_secret)?;
        let email = self.account.email.clone();
        let answer = self.account_call(
            "/my/connect",
            &[("email", &email), ("appkey", APP_KEY)],
            &login,
        )?;
        self.adopt_session(&answer, &login)
    }

    /// Renews an expired session (`/my/reconnect`), or signs in again when that fails.
    fn reconnect(&mut self) -> Result<(), Error> {
        let Some(session) = self.account.session.clone() else {
            return self.connect();
        };
        let server = token_from_hex(&session.server_token)?;
        match self.account_call(
            "/my/reconnect",
            &[
                ("sessiontoken", &session.session_token),
                ("regaintoken", &session.regain_token),
            ],
            &server,
        ) {
            Ok(answer) => self.adopt_session(&answer, &server),
            Err(_) => self.connect(),
        }
    }

    fn ensure_session(&mut self) -> Result<CloudSession, Error> {
        if self.account.session.is_none() {
            self.connect()?;
        }
        self.account
            .session
            .clone()
            .ok_or(Error::Protocol("no session"))
    }

    pub fn list_devices(&mut self) -> Result<Vec<Device>, Error> {
        let session = self.ensure_session()?;
        let server = token_from_hex(&session.server_token)?;
        let answer = self.account_call(
            "/my/listdevices",
            &[("sessiontoken", &session.session_token)],
            &server,
        )?;
        serde_json::from_value(answer["list"].clone())
            .map_err(|_| Error::Protocol("bad device list"))
    }

    fn device_call(&mut self, path: &str, params: &[Value]) -> Result<Value, Error> {
        let session = self.ensure_session()?;
        let device_id = self.account.device_id.clone().ok_or_else(|| {
            Error::Config("no JDownloader device chosen for this connection".into())
        })?;
        let device_secret = token_from_hex(&self.account.device_secret)?;
        let device_token = crypto::session_token(&device_secret, &session.session_token)?;
        let rid = self.next_rid();
        // Params travel as JSON strings inside the JSON request.
        let encoded: Vec<String> = params.iter().map(Value::to_string).collect();
        let request = json!({ "url": path, "params": encoded, "rid": rid, "apiVer": 1 });
        let body = crypto::encrypt(&device_token, &request.to_string());
        let url = format!(
            "{SERVER}/t_{}_{}{path}",
            session.session_token,
            encode_component(&device_id)
        );
        let (status, answer) = read(
            self.agent
                .post(&url)
                .header("Content-Type", "application/aesjson-jd; charset=utf-8")
                .send(body),
        )?;
        if status != 200 {
            let kind = error_kind(&answer)
                .or_else(|| {
                    crypto::decrypt(&device_token, &answer)
                        .ok()
                        .and_then(|plain| error_kind(&plain))
                })
                .unwrap_or_else(|| "ERROR".into());
            return Err(Error::Api { status, kind });
        }
        let plain = crypto::decrypt(&device_token, &answer)?;
        let value: Value = serde_json::from_str(&plain)
            .map_err(|_| Error::Protocol("device answer is not JSON"))?;
        Ok(unwrap_data(value))
    }
}

impl Api for CloudApi {
    fn call(&mut self, path: &str, params: &[Value]) -> Result<Value, Error> {
        match self.device_call(path, params) {
            Err(error)
                if error.is_session_lost() || matches!(error, Error::Api { status: 403, .. }) =>
            {
                self.reconnect()?;
                self.device_call(path, params)
            }
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_query_components() {
        assert_eq!(encode_component("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(encode_component("ada@example.org"), "ada%40example.org");
        assert_eq!(encode_component(r#"{"a":1}"#), "%7B%22a%22%3A1%7D");
        assert_eq!(encode_component("é"), "%C3%A9");
    }

    #[test]
    fn builds_local_urls_with_json_params() {
        assert_eq!(
            local_url("http://jd:3128", "/downloadcontroller/start", &[]),
            "http://jd:3128/downloadcontroller/start"
        );
        assert_eq!(
            local_url(
                "http://jd:3128",
                "/downloadcontroller/pause",
                &[json!(true)]
            ),
            "http://jd:3128/downloadcontroller/pause?true"
        );
        assert_eq!(
            local_url(
                "http://jd:3128",
                "/downloadsV2/queryLinks",
                &[json!({"name": true})]
            ),
            "http://jd:3128/downloadsV2/queryLinks?%7B%22name%22%3Atrue%7D"
        );
    }

    #[test]
    fn unwraps_data_and_reads_error_kinds() {
        assert_eq!(unwrap_data(json!({"data": [1, 2]})), json!([1, 2]));
        assert_eq!(unwrap_data(json!(true)), json!(true));
        assert_eq!(
            error_kind(r#"{"src":"MYJD","type":"AUTH_FAILED"}"#).as_deref(),
            Some("AUTH_FAILED")
        );
        assert_eq!(error_kind("oops"), None);
    }
}
