//! Rig-initiated telemetry. Every watchdog pushes its 10 s snapshot here (runtime ≥ 0.4.0)
//! instead of the monitor opening an SSH channel to read it. The sender is `Pusher` in
//! `runtime/rig-runtime.py`.
//!
//! Wire format: a zlib-compressed JSON message, HMAC-SHA256 signed with the machine's push key
//! (`X-Rig-Machine`, `X-Rig-Signature`). After one acknowledged full snapshot the rig sends only
//! a patch against it (`base` = the acknowledged `seq`). Replies are 200 with what was stored,
//! or 409 with this controller's `epoch` when the rig must resend a full snapshot.
//!
//! Replay protection: the signature covers the epoch, which is new on every controller start,
//! so nothing captured before a restart is accepted; within one run `seq` must increase per
//! boot and a boot id that has been replaced is never accepted again.
use crate::App;
use crate::telemetry::{from_snapshot, policy_digest};
use chrono::Utc;
use hmac::{Hmac, Mac};
use rig_domain::*;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::Sha256;
use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Largest compressed body accepted (checked before the signature).
pub const MAX_BODY: usize = 256 * 1024;
/// Largest message after decompression (a decompression bomb stops here).
const MAX_INFLATED: u64 = 4 * 1024 * 1024;
/// A push newer than this means the monitor need not read the machine over SSH.
const LIVE: Duration = Duration::from_secs(25);
const MACHINE_TTL: Duration = Duration::from_secs(60);

#[derive(Clone, Debug)]
pub struct PushSettings {
    /// Explicit URL rigs push to; `None` derives it per rig from its SSH connection.
    pub url: Option<String>,
    /// Port of the ingest listener, for derived URLs.
    pub port: u16,
}

pub struct PushHub {
    settings: Option<PushSettings>,
    epoch: String,
    sessions: Mutex<HashMap<Uuid, Session>>,
    machines: Mutex<HashMap<Uuid, (Machine, Instant)>>,
    gates: Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<()>>>>,
}
impl Default for PushHub {
    /// Push disabled: every machine is read over SSH.
    fn default() -> Self {
        Self::new(None)
    }
}
impl PushHub {
    pub fn new(settings: Option<PushSettings>) -> Self {
        Self {
            settings,
            epoch: Uuid::new_v4().simple().to_string(),
            sessions: Mutex::default(),
            machines: Mutex::default(),
            gates: Mutex::default(),
        }
    }
    pub fn enabled(&self) -> bool {
        self.settings.is_some()
    }
    pub(crate) fn live(&self, machine: Uuid, digest: &str) -> bool {
        self.sessions
            .lock()
            .unwrap()
            .get(&machine)
            .filter(|s| s.push_digest.as_deref() == Some(digest))
            .and_then(|s| s.last_push)
            .is_some_and(|at| at.elapsed() < LIVE)
    }
    fn forget(&self, machine: Uuid) {
        self.sessions.lock().unwrap().remove(&machine);
        self.machines.lock().unwrap().remove(&machine);
    }
}

#[derive(Clone)]
struct Session {
    boot_id: String,
    last_seq: i64,
    /// Boot ids this machine has since replaced; a message from one is a replay.
    retired: Vec<String>,
    /// The materialised snapshot (without events) the rig's next patch applies to.
    base: Option<(i64, Value)>,
    events_seq: Option<i64>,
    last_push: Option<Instant>,
    push_digest: Option<String>,
}

pub enum PushReply {
    /// HTTP 200: stored. Carries the acknowledged event sequence and, if the rig's copy is
    /// stale, the machine's recovery policy.
    Accepted(Value),
    /// HTTP 409: resend a full snapshot under the `epoch` in the body.
    Resync(Value),
}

#[derive(Debug, Deserialize)]
struct Message {
    v: u32,
    machine: Uuid,
    epoch: Option<String>,
    seq: i64,
    boot_id: String,
    sent_uptime: f64,
    base: Option<i64>,
    snapshot: Option<Value>,
    patch: Option<Value>,
    #[serde(default)]
    events: Vec<Value>,
    software: Option<Value>,
    hardware: Option<Value>,
}

impl App {
    /// The push target a rig should be configured with, or `None` while push is disabled.
    pub fn push_target(&self, machine: Uuid) -> Option<PushTarget> {
        let settings = self.push.settings.as_ref()?;
        let key = self.vault.push_key(machine)?;
        let setting = settings
            .url
            .clone()
            .unwrap_or_else(|| format!("auto:{}", settings.port));
        Some(PushTarget {
            url: settings.url.clone(),
            port: settings.port,
            digest: crate::digest(&format!("{key}|{setting}")),
            key,
        })
    }

    pub async fn ingest_push(
        &self,
        machine: &str,
        signature: &str,
        body: &[u8],
    ) -> Result<PushReply> {
        if !self.push.enabled() {
            return Err(Error::NotFound);
        }
        if body.len() > MAX_BODY {
            return Err(Error::Validation("上报数据过大".into()));
        }
        let id: Uuid = machine.trim().parse().map_err(|_| Error::Unauthorized)?;
        let key = self.vault.push_key(id).ok_or(Error::Unauthorized)?;
        verify(&key, signature, body)?;
        let message = inflate(body)?;
        if message.v != 1 || message.machine != id {
            return Err(Error::Validation("上报数据与机器不符".into()));
        }
        let machine = self.push_machine(id).await?;
        // Serialize a machine through validation, durable storage and acknowledgement.
        // Different machines remain independent; never hold the global mutex across await.
        let gate = self
            .push
            .gates
            .lock()
            .unwrap()
            .entry(id)
            .or_default()
            .clone();
        let _guard = gate.lock().await;
        let resync = || PushReply::Resync(json!({"epoch": self.push.epoch, "need": "full"}));
        let (snapshot, mut session) = {
            let sessions = self.push.sessions.lock().unwrap();
            if message.epoch.as_deref() != Some(self.push.epoch.as_str()) {
                return Ok(resync());
            }
            let mut session = sessions.get(&id).cloned().unwrap_or_else(|| Session {
                boot_id: message.boot_id.clone(),
                last_seq: i64::MIN,
                retired: Vec::new(),
                base: None,
                events_seq: None,
                last_push: None,
                push_digest: None,
            });
            if session.boot_id != message.boot_id {
                if session.retired.contains(&message.boot_id) {
                    return Err(Error::Validation("上报来自已结束的开机周期".into()));
                }
                let previous = std::mem::replace(&mut session.boot_id, message.boot_id.clone());
                session.retired.push(previous);
                session.last_seq = i64::MIN;
                session.base = None;
                session.events_seq = None;
                session.last_push = None;
            }
            if message.seq <= session.last_seq {
                return Err(Error::Validation("重复或过期的上报".into()));
            }
            let snapshot = match (message.base, message.snapshot, message.patch) {
                (None, Some(full), None) => full,
                (Some(base), None, Some(patch)) => match &session.base {
                    Some((seq, value)) if *seq == base => {
                        let mut value = value.clone();
                        apply_patch(&mut value, patch);
                        value
                    }
                    _ => return Ok(resync()),
                },
                _ => return Err(Error::Validation("上报数据缺少快照".into())),
            };
            if snapshot["schema"] != 1
                || !snapshot["uptime"].is_number()
                || snapshot["boot_id"].as_str() != Some(message.boot_id.as_str())
            {
                session.base = None;
                return Err(Error::Validation("上报快照格式无效".into()));
            }
            (snapshot, session)
        };

        let events_seq = session.events_seq;
        let next_base = snapshot.clone();
        let mut snapshot = snapshot;
        snapshot["events"] = Value::Array(message.events);
        // The rig sends right after writing, so its uptime at send time stands in for "now".
        let raw = json!({"now_uptime": message.sent_uptime, "snapshot": snapshot});
        let now = Utc::now();
        let parsed =
            from_snapshot(&raw, now).ok_or_else(|| Error::Validation("上报快照已过期".into()))?;
        let mut system = parsed.system;
        if let Some(object) = system.as_object_mut() {
            object.insert("transport".into(), json!("push"));
        }
        let mut observations = vec![
            (parsed.observed_at, "system", system),
            (parsed.observed_at, "mining", parsed.mining),
        ];
        if let Some(software) = message.software {
            observations.push((now, "software", software));
        }
        if let Some(hardware) = message.hardware {
            observations.push((now, "hardware", hardware));
        }
        let observations: Vec<Observation> = observations
            .into_iter()
            .map(|(observed_at, kind, data)| Observation {
                machine_id: id,
                kind: kind.into(),
                observed_at,
                data,
                error: None,
            })
            .collect();
        // One multi-row transaction per push. Never acknowledge queue admission as storage:
        // a failed write must leave the old base and freshness intact for retry/SSH fallback.
        self.repository.observe_many(&observations).await?;
        {
            let mut sessions = self.push.sessions.lock().unwrap();
            session.last_seq = message.seq;
            session.push_digest = next_base["push_digest"].as_str().map(str::to_owned);
            session.base = Some((message.seq, next_base));
            // A delayed but still parseable snapshot is not evidence of fresh telemetry.
            let age = (Utc::now() - parsed.observed_at).to_std().unwrap_or_default();
            session.last_push = Instant::now().checked_sub(age);
            sessions.insert(id, session);
        }

        let fresh: Vec<Value> = parsed
            .events
            .into_iter()
            .filter(|e| {
                e["seq"]
                    .as_i64()
                    .is_some_and(|s| events_seq.is_none_or(|acked| s > acked))
            })
            .collect();
        let mut acknowledged = events_seq;
        if !fresh.is_empty() {
            match self.repository.record_events(id, &fresh).await {
                Ok(_) => {
                    acknowledged = fresh
                        .iter()
                        .filter_map(|e| e["seq"].as_i64())
                        .chain(events_seq)
                        .max();
                    if let Some(session) = self.push.sessions.lock().unwrap().get_mut(&id) {
                        session.events_seq = session.events_seq.max(acknowledged);
                    }
                }
                // Not acknowledged: the rig sends them again with its next push.
                Err(error) => tracing::warn!(%error, machine = %id, "pushed events not saved"),
            }
        }

        let mut reply = json!({"ok": true});
        if let Some(seq) = acknowledged {
            reply["events_seq"] = json!(seq);
        }
        let wanted = policy_digest(&machine.policy);
        if parsed.policy_digest.as_deref() != Some(wanted.as_str()) {
            reply["policy"] = json!({"digest": wanted, "policy": machine.policy});
        }
        Ok(PushReply::Accepted(reply))
    }

    /// Bind the reply (including policy) to this exact signed request, even over LAN HTTP.
    pub fn sign_push_reply(
        &self,
        machine: &str,
        request: &str,
        status: u16,
        body: &[u8],
    ) -> Result<String> {
        let id = machine.parse().map_err(|_| Error::Unauthorized)?;
        let key = self.vault.push_key(id).ok_or(Error::Unauthorized)?;
        let key = hex::decode(key).map_err(|_| Error::Unauthorized)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&key).map_err(|_| Error::Unauthorized)?;
        mac.update(format!("rigdeck-push-reply-v1\n{request}\n{status}\n").as_bytes());
        mac.update(body);
        Ok(hex::encode(mac.finalize().into_bytes()))
    }

    async fn push_machine(&self, id: Uuid) -> Result<Machine> {
        if let Some((machine, at)) = self.push.machines.lock().unwrap().get(&id)
            && at.elapsed() < MACHINE_TTL
        {
            return Ok(machine.clone());
        }
        match self.repository.machine(id).await {
            Ok(machine) => {
                self.push
                    .machines
                    .lock()
                    .unwrap()
                    .insert(id, (machine.clone(), Instant::now()));
                Ok(machine)
            }
            Err(Error::NotFound) => {
                self.push.forget(id);
                Err(Error::NotFound)
            }
            Err(error) => Err(error),
        }
    }
}

fn verify(key: &str, signature: &str, body: &[u8]) -> Result<()> {
    let key = hex::decode(key).map_err(|_| Error::Internal("上报密钥格式错误".into()))?;
    let signature = hex::decode(signature.trim()).map_err(|_| Error::Unauthorized)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(&key)
        .map_err(|_| Error::Internal("上报密钥格式错误".into()))?;
    mac.update(body);
    mac.verify_slice(&signature)
        .map_err(|_| Error::Unauthorized)
}

fn inflate(body: &[u8]) -> Result<Message> {
    let mut text = Vec::new();
    flate2::read::ZlibDecoder::new(body)
        .take(MAX_INFLATED + 1)
        .read_to_end(&mut text)
        .map_err(|_| Error::Validation("上报数据无法解压".into()))?;
    if text.len() as u64 > MAX_INFLATED {
        return Err(Error::Validation("上报数据解压后过大".into()));
    }
    serde_json::from_slice(&text).map_err(|e| Error::Validation(format!("上报数据无效: {e}")))
}

/// Inverse of the runtime's `snapshot_diff`: objects are patched key by key (`$del` removes),
/// a list patched with `$items` is patched index by index, anything else is replaced.
pub fn apply_patch(target: &mut Value, patch: Value) {
    match (target, patch) {
        (Value::Object(object), Value::Object(mut changes)) => {
            if let Some(Value::Array(removed)) = changes.remove("$del") {
                for key in removed {
                    if let Some(key) = key.as_str() {
                        object.remove(key);
                    }
                }
            }
            for (key, value) in changes {
                match object.get_mut(&key) {
                    Some(existing) => apply_patch(existing, value),
                    None => {
                        object.insert(key, value);
                    }
                }
            }
        }
        (Value::Array(items), Value::Object(mut changes)) if changes.contains_key("$items") => {
            if let Some(Value::Object(by_index)) = changes.remove("$items") {
                for (index, value) in by_index {
                    if let Some(item) = index.parse::<usize>().ok().and_then(|i| items.get_mut(i)) {
                        apply_patch(item, value);
                    }
                }
            }
        }
        (target, value) => *target = value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches_objects_lists_and_removals_like_the_runtime_diff() {
        let mut base = json!({
            "system": {"cpu_pct": 10.0, "load": [1.0, 2.0, 3.0], "gpus": [{"t": 50}, {"t": 51}], "gone": 1},
            "mining": {"instances": [{"stats": {"hashrate_hs": 5}}]},
            "policy_digest": null
        });
        apply_patch(
            &mut base,
            json!({
                "system": {"cpu_pct": 12.5, "load": {"$items": {"0": 1.5}}, "gpus": {"$items": {"1": {"t": 60}}},
                           "$del": ["gone"], "new": "x"},
                "mining": {"instances": [{"stats": null}, {"stats": {}}]},
                "policy_digest": "d"
            }),
        );
        assert_eq!(
            base,
            json!({
                "system": {"cpu_pct": 12.5, "load": [1.5, 2.0, 3.0], "gpus": [{"t": 50}, {"t": 60}], "new": "x"},
                "mining": {"instances": [{"stats": null}, {"stats": {}}]},
                "policy_digest": "d"
            })
        );
        // A non-object replacing an object, and an object replacing a scalar.
        let mut value = json!({"a": {"b": 1}, "c": 1});
        apply_patch(&mut value, json!({"a": 2, "c": {"d": 3}}));
        assert_eq!(value, json!({"a": 2, "c": {"d": 3}}));
    }

    #[test]
    fn patches_written_by_the_runtime_rebuild_the_new_snapshot() {
        // Generated by tests/test_push.py from the runtime's snapshot_diff.
        let cases: Vec<Value> = serde_json::from_str(include_str!(
            "../../../tests/fixtures/snapshot-patches.json"
        ))
        .unwrap();
        assert!(cases.len() >= 7);
        for case in cases {
            let mut value = case["old"].clone();
            apply_patch(&mut value, case["patch"].clone());
            assert_eq!(value, case["new"], "{}", case["patch"]);
        }
    }

    #[test]
    fn signature_must_match_the_exact_body() {
        let key = "ab".repeat(32);
        let mut mac = Hmac::<Sha256>::new_from_slice(&hex::decode(&key).unwrap()).unwrap();
        mac.update(b"body");
        let signature = hex::encode(mac.finalize().into_bytes());
        assert!(verify(&key, &signature, b"body").is_ok());
        assert!(matches!(
            verify(&key, &signature, b"bodY"),
            Err(Error::Unauthorized)
        ));
        assert!(matches!(
            verify(&key, "zz", b"body"),
            Err(Error::Unauthorized)
        ));
    }

    #[test]
    fn decompression_is_bounded() {
        use flate2::{Compression, write::ZlibEncoder};
        use std::io::Write;
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(b"[").unwrap();
        encoder
            .write_all(&vec![b' '; MAX_INFLATED as usize + 10])
            .unwrap();
        let bomb = encoder.finish().unwrap();
        assert!(bomb.len() < MAX_BODY);
        assert!(inflate(&bomb).unwrap_err().to_string().contains("过大"));
        assert!(inflate(b"not zlib").is_err());
    }
}
