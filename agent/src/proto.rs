//! The BitBOINC side of the protocol: one JSON POST per action to <server>/thermopsis_app.php (login, report, poll, status, logout).
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Debug)]
pub enum ApiError {
    /// the server answered with an error: (HTTP status, error code such as "wrong_key", "bad_token", "too_fast")
    Http(u16, String),
    /// no answer (network down, TLS problem, timeout)
    Net(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Http(s, c) => write!(f, "server said {} ({})", s, c),
            ApiError::Net(e) => write!(f, "no connection: {}", e),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Command {
    pub id: u64,
    pub kind: String,
    pub params: Value,
}

pub struct Client {
    url: String,
    agent: ureq::Agent,
}

impl Client {
    pub fn new(server: &str) -> Client {
        let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(15)).timeout(Duration::from_secs(40)).user_agent(&format!("thermopsis-agent/{}", crate::VERSION)).build();
        Client { url: format!("{}/thermopsis_app.php", server.trim_end_matches('/')), agent }
    }

    fn call(&self, body: Value) -> Result<Value, ApiError> {
        match self.agent.post(&self.url).send_json(body) {
            Ok(r) => r.into_json::<Value>().map_err(|e| ApiError::Net(format!("unreadable answer: {}", e))),
            Err(ureq::Error::Status(code, r)) => {
                let j: Value = r.into_json().unwrap_or_else(|_| json!({}));
                Err(ApiError::Http(code, j["error"].as_str().unwrap_or("").to_string()))
            }
            Err(e) => Err(ApiError::Net(e.to_string())),
        }
    }

    /// Sign in with the weak account key or the account key; returns (token, public user name). The key itself is not kept anywhere.
    pub fn login(&self, key: &str, device: &str, name: &str) -> Result<(String, String), ApiError> {
        let r = self.call(json!({"action": "login", "key": key.trim(), "device": device, "name": name, "os": crate::os_name(), "app": crate::VERSION}))?;
        Ok((r["token"].as_str().unwrap_or("").to_string(), r["user"].as_str().unwrap_or("").to_string()))
    }

    pub fn report(&self, token: &str, report: &Value) -> Result<(), ApiError> {
        self.call(json!({"action": "report", "token": token, "report": report})).map(|_| ())
    }

    pub fn poll(&self, token: &str) -> Result<Vec<Command>, ApiError> {
        let r = self.call(json!({"action": "poll", "token": token}))?;
        let mut out = Vec::new();
        if let Some(list) = r["commands"].as_array() {
            for c in list {
                if let (Some(id), Some(kind)) = (c["id"].as_u64(), c["kind"].as_str()) {
                    out.push(Command { id, kind: kind.to_string(), params: c["params"].clone() });
                }
            }
        }
        Ok(out)
    }

    pub fn status(&self, token: &str) -> Result<Value, ApiError> {
        self.call(json!({"action": "status", "token": token}))
    }

    pub fn logout(&self, token: &str) -> Result<(), ApiError> {
        self.call(json!({"action": "logout", "token": token})).map(|_| ())
    }
}
