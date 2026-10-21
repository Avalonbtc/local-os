use crate::App;
use chrono::Utc;
use rig_domain::*;
use serde_json::Value;
use std::time::Duration;
use uuid::Uuid;

impl App {
    pub async fn fleet_summary(&self, machine: Option<Uuid>) -> Result<Vec<Observation>> {
        Ok(summarize(self.repository.latest_summary(machine).await?))
    }
    pub async fn history_summary(
        &self,
        machine: Uuid,
        kind: &str,
        hours: u32,
    ) -> Result<Vec<Observation>> {
        Ok(summarize(self.history(machine, kind, hours).await?))
    }
    pub async fn unread_messages(&self, machine: Uuid, limit: i64) -> Result<Vec<MachineMessage>> {
        self.repository.machine(machine).await?;
        self.repository.unread_messages(machine, limit).await
    }
    pub async fn dismiss_messages(&self, machine: Uuid, input: MessageDismiss) -> Result<()> {
        self.repository.machine(machine).await?;
        if let Some(key) = &input.key
            && (key.is_empty()
                || key.len() > 200
                || !["runtime:", "job:", "audit:"]
                    .iter()
                    .any(|p| key.starts_with(p)))
        {
            return Err(Error::Validation("消息标识无效".into()));
        }
        self.repository
            .dismiss_messages(machine, input.key.as_deref())
            .await
    }
    pub async fn messages(&self, machine: Uuid, limit: i64) -> Result<Vec<MachineMessage>> {
        self.repository.machine(machine).await?;
        self.repository.messages(machine, limit).await
    }
    pub async fn miner_log(&self, machine: Uuid, instance: &str, lines: u32) -> Result<MinerLog> {
        if instance.is_empty()
            || instance.len() > 40
            || !instance
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))
        {
            return Err(Error::Validation("实例名称无效".into()));
        }
        let m = self.repository.machine(machine).await?;
        let credential = self.credential(machine).await?;
        let value = self
            .runtime
            .log_tail(&m, &credential, instance, lines)
            .await?;
        serde_json::from_value(value)
            .map_err(|e| Error::Unavailable(format!("运行层响应无效: {e}")))
    }
    pub async fn run_monitor(self) {
        // One task per machine: a dead endpoint cannot delay healthy machines.
        let mut workers = tokio::task::JoinSet::new();
        let mut handles = std::collections::HashMap::new();
        let maintenance = self.clone();
        workers.spawn(async move {
            loop {
                if let Err(error) = maintenance.repository.maintain().await {
                    tracing::warn!(%error, "retention failed");
                }
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        });
        // All machines feed one writer: one multi-row statement per second instead of
        // one transaction per sample (1 000 machines ≈ 300 samples/s).
        let (sink, samples) = tokio::sync::mpsc::channel::<Observation>(4096);
        let writer = self.clone();
        workers.spawn(async move { writer.write_observations(samples).await });
        loop {
            if let Ok(machines) = self.repository.machines().await {
                let ids: std::collections::HashSet<_> = machines.iter().map(|m| m.id).collect();
                handles.retain(|(id, _), handle: &mut tokio::task::AbortHandle| {
                    if !ids.contains(id) {
                        handle.abort();
                        return false;
                    }
                    !handle.is_finished()
                });
                for machine in machines {
                    handles.entry((machine.id, "ssh")).or_insert_with(|| {
                        let app = self.clone();
                        let id = machine.id;
                        let sink = sink.clone();
                        workers.spawn(async move { app.monitor_machine(id, "ssh", sink).await })
                    });
                }
            }
            while workers.try_join_next().is_some() {}
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    }
    async fn write_observations(&self, mut samples: tokio::sync::mpsc::Receiver<Observation>) {
        let mut batch = Vec::with_capacity(512);
        loop {
            let Some(first) = samples.recv().await else {
                return;
            };
            batch.push(first);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
            while batch.len() < 512 {
                match tokio::time::timeout_at(deadline, samples.recv()).await {
                    Ok(Some(sample)) => batch.push(sample),
                    Ok(None) | Err(_) => break,
                }
            }
            if let Err(error) = self.repository.observe_many(&batch).await {
                tracing::warn!(%error, count = batch.len(), "observations not saved");
            }
            batch.clear();
        }
    }
    async fn monitor_machine(
        &self,
        id: Uuid,
        channel: &str,
        sink: tokio::sync::mpsc::Sender<Observation>,
    ) {
        let mut tick = tokio::time::interval(Duration::from_secs(10));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut cycle = 0u64;
        // Machine row and decrypted credential change rarely; re-read every minute or after errors
        // instead of two database round trips (and an AES decrypt) per 10 s tick.
        let mut cached: Option<(Machine, std::result::Result<Credential, String>)> = None;
        let mut last_event_seq = i64::MIN;
        let mut last_policy_push: Option<tokio::time::Instant> = None;
        let mut last_push_setup: Option<tokio::time::Instant> = None;
        loop {
            tick.tick().await;
            if channel == "ssh"
                && self
                    .push_target(id)
                    .is_some_and(|t| self.push.live(id, &t.digest))
            {
                // The rig's watchdog pushed system, mining, software and hardware itself and got
                // its policy in the reply: nothing to read over SSH.
                cycle += 1;
                continue;
            }
            if cached.is_none() || cycle.is_multiple_of(6) {
                cached = match self.repository.machine(id).await {
                    Ok(m) => {
                        let credential = if channel == "ssh" {
                            self.credential(id).await.map_err(|e| e.to_string())
                        } else {
                            Err(String::new())
                        };
                        Some((m, credential))
                    }
                    Err(Error::NotFound) => return,
                    Err(_) => {
                        cycle += 1;
                        continue;
                    }
                };
            }
            let Some((machine, credential)) = cached.as_ref() else {
                continue;
            };
            let credential = match credential {
                Ok(credential) => credential,
                Err(error) => {
                    send(
                        &sink,
                        observation(id, "system", Err(Error::Unavailable(error.clone()))),
                    )
                    .await;
                    cached = None;
                    cycle += 1;
                    continue;
                }
            };
            let snapshot = tokio::time::timeout(
                Duration::from_secs(25),
                self.runtime.snapshot(machine, credential),
            )
            .await
            .unwrap_or_else(|_| Err(Error::Unavailable("SSH 采集超时".into())));
            let mut legacy = true;
            match snapshot {
                Err(error) => {
                    // The host is unreachable: do not spend further timeouts on it this tick.
                    send(&sink, observation(id, "system", Err(error))).await;
                    cycle += 1;
                    continue;
                }
                Ok(Some(raw)) => {
                    if let Some(parsed) = from_snapshot(&raw, Utc::now()) {
                        legacy = false;
                        let mut system = parsed.system;
                        if let Some(object) = system.as_object_mut() {
                            object.insert("transport".into(), Value::from("ssh"));
                        }
                        for (kind, data) in [("system", system), ("mining", parsed.mining)] {
                            send(
                                &sink,
                                Observation {
                                    machine_id: id,
                                    kind: kind.into(),
                                    observed_at: parsed.observed_at,
                                    data,
                                    error: None,
                                },
                            )
                            .await;
                        }
                        let fresh: Vec<Value> = parsed
                            .events
                            .into_iter()
                            .filter(|e| e["seq"].as_i64().is_some_and(|s| s > last_event_seq))
                            .collect();
                        if !fresh.is_empty() {
                            match self.repository.record_events(id, &fresh).await {
                                Ok(_) => {
                                    last_event_seq = fresh
                                        .iter()
                                        .filter_map(|e| e["seq"].as_i64())
                                        .max()
                                        .unwrap_or(last_event_seq);
                                }
                                Err(error) => tracing::warn!(%error, "machine events not saved"),
                            }
                        }
                        let wanted = policy_digest(&machine.policy);
                        if parsed.policy_digest.as_deref() != Some(wanted.as_str())
                            && last_policy_push
                                .is_none_or(|at| at.elapsed() > Duration::from_secs(60))
                        {
                            last_policy_push = Some(tokio::time::Instant::now());
                            if let Err(error) = self
                                .runtime
                                .set_policy(machine, credential, &wanted, &machine.policy)
                                .await
                            {
                                tracing::warn!(%error, machine = %id, "recovery policy not delivered");
                            }
                        }
                        // Runtime ≥ 0.4.0 reports push_digest (null until configured): point its
                        // watchdog at us so later samples arrive without SSH.
                        if raw["snapshot"].get("push_digest").is_some()
                            && let Some(target) = self.push_target(id)
                            && raw["snapshot"]["push_digest"].as_str()
                                != Some(target.digest.as_str())
                            && last_push_setup
                                .is_none_or(|at| at.elapsed() > Duration::from_secs(300))
                        {
                            last_push_setup = Some(tokio::time::Instant::now());
                            match self
                                .runtime
                                .configure_push(machine, credential, &target)
                                .await
                            {
                                Ok(()) => {
                                    tracing::info!(machine = %id, "telemetry push configured")
                                }
                                Err(error) => {
                                    tracing::warn!(%error, machine = %id, "telemetry push not configured")
                                }
                            }
                        }
                    }
                }
                Ok(None) => {}
            }
            let mut kinds = Vec::new();
            if legacy {
                // Runtime < 0.1.8, or its watchdog stopped publishing: privileged collection.
                kinds.extend(["system", "mining"]);
            }
            if cycle.is_multiple_of(6) {
                kinds.push("software");
            }
            if cycle.is_multiple_of(360) {
                kinds.push("hardware");
            }
            for kind in kinds {
                let result = tokio::time::timeout(
                    Duration::from_secs(25),
                    self.runtime.collect(machine, credential, kind),
                )
                .await
                .unwrap_or_else(|_| Err(Error::Unavailable("SSH 采集超时".into())));
                let failed = result.is_err();
                // Older runtime versions return mining for unknown collection kinds.
                if kind != "hardware" || result.as_ref().is_ok_and(|v| v.get("instances").is_none())
                {
                    send(&sink, observation(id, kind, result)).await;
                }
                if failed {
                    break;
                }
            }
            cycle += 1;
        }
    }
}

pub(crate) async fn send(sink: &tokio::sync::mpsc::Sender<Observation>, observation: Observation) {
    if sink.send(observation).await.is_err() {
        tracing::warn!("telemetry writer stopped");
    }
}

fn observation(id: Uuid, kind: &str, result: Result<Value>) -> Observation {
    let (data, error) = match result {
        Ok(mut v) => {
            // Transfer remote sample age rather than comparing two wall clocks.
            if kind == "mining"
                && let Some(collected) = v["collected_at"].as_f64()
            {
                let offset = Utc::now().timestamp_millis() as f64 / 1000.0 - collected;
                for item in v["instances"].as_array_mut().into_iter().flatten() {
                    for key in ["observed_at", "stats_observed_at"] {
                        if let Some(at) = item[key].as_f64() {
                            item[key] = serde_json::json!(at + offset);
                        }
                    }
                }
            }
            (v, None)
        }
        Err(e) => (Value::Null, Some(e.to_string())),
    };
    Observation {
        machine_id: id,
        kind: kind.into(),
        observed_at: Utc::now(),
        data,
        error,
    }
}

/// Digest of the machine policy as the controller serialises it (keys sorted by serde_json).
pub fn policy_digest(policy: &Value) -> String {
    crate::digest(&serde_json::to_string(policy).unwrap_or_default())
}

pub struct ParsedSnapshot {
    pub observed_at: chrono::DateTime<Utc>,
    pub system: Value,
    pub mining: Value,
    pub events: Vec<Value>,
    pub policy_digest: Option<String>,
}

/// Convert a rig snapshot to controller time using the rig's monotonic uptime, so a skewed
/// rig clock can never make stale data look fresh (or fresh data look stale).
/// Returns `None` when the watchdog has not refreshed the file for 45 s.
pub fn from_snapshot(raw: &Value, now: chrono::DateTime<Utc>) -> Option<ParsedSnapshot> {
    let now_uptime = raw["now_uptime"].as_f64()?;
    let snapshot = &raw["snapshot"];
    let written = snapshot["uptime"].as_f64()?;
    let age = now_uptime - written;
    if !(-1.0..=45.0).contains(&age) {
        return None;
    }
    let boot = snapshot["boot_id"].as_str();
    let at = |uptime: f64| -> chrono::DateTime<Utc> {
        now - chrono::Duration::milliseconds(((now_uptime - uptime).max(0.0) * 1000.0) as i64)
    };
    let epoch = |t: chrono::DateTime<Utc>| t.timestamp_millis() as f64 / 1000.0;
    let mut mining = snapshot["mining"].clone();
    for item in mining["instances"].as_array_mut().into_iter().flatten() {
        let same_boot = item["boot_id"].as_str() == boot;
        match item["sample_uptime"].as_f64() {
            Some(sample) if same_boot => {
                let t = epoch(at(sample));
                item["observed_at"] = serde_json::json!(t);
                if !item["stats_observed_at"].is_null() {
                    // Runtime ≥ 0.3.2 keeps the last good miner numbers through one failed API
                    // read and says when they were taken; older runtimes read them every sample.
                    let read_at = item["stats_uptime"].as_f64().unwrap_or(sample);
                    item["stats_observed_at"] = serde_json::json!(epoch(at(read_at)));
                }
            }
            _ => {
                item["process_alive"] = Value::Bool(false);
                item["stats"] = Value::Null;
                item["stats_observed_at"] = Value::Null;
            }
        }
    }
    let events = snapshot["events"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|event| {
            let mut event = event.clone();
            if event["boot_id"].as_str() == boot
                && let Some(uptime) = event["uptime"].as_f64()
            {
                event["at"] = serde_json::json!(epoch(at(uptime)));
            }
            event
        })
        .collect();
    Some(ParsedSnapshot {
        observed_at: at(written),
        system: snapshot["system"].clone(),
        mining,
        events,
        policy_digest: snapshot["policy_digest"].as_str().map(str::to_owned),
    })
}

fn summarize(observations: Vec<Observation>) -> Vec<Observation> {
    observations
        .into_iter()
        .filter_map(|mut observation| {
            let keys: &[&str] = match observation.kind.as_str() {
                "system" => &[
                    "uptime",
                    "cpu_pct",
                    "memory_pct",
                    "memory_total",
                    "memory_used",
                    "gpus",
                    "logical_cpus",
                    "load",
                    "cpu_temperature",
                    "cpu_power_w",
                    "cpu_power_packages",
                    "cpu_power_source",
                    "addresses",
                    "runtime_version",
                    "transport",
                ],
                "mining" => return Some(observation),
                _ => return None,
            };
            if let Some(object) = observation.data.as_object_mut() {
                object.retain(|key, _| keys.contains(&key.as_str()));
            }
            Some(observation)
        })
        .collect()
}

impl App {
    pub async fn latest(&self, machine: Option<Uuid>) -> Result<Vec<Observation>> {
        self.repository.latest(machine).await
    }
    pub async fn history(&self, machine: Uuid, kind: &str, hours: u32) -> Result<Vec<Observation>> {
        self.repository
            .history(machine, kind, hours.min(2160))
            .await
    }
}

#[cfg(test)]
mod summary_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn summary_excludes_bulk_data_and_unknown_kinds() {
        let observed_at = Utc::now();
        let make = |kind: &str, data| Observation {
            machine_id: Uuid::nil(),
            kind: kind.into(),
            observed_at,
            data,
            error: Some("offline".into()),
        };
        let result = summarize(vec![
            make("events", json!({"items":["large event log"]})),
            make(
                "system",
                json!({"cpu_pct":95,"logical_cpus":256,"gpus":[],"hardware":{"large":true}}),
            ),
            make(
                "mining",
                json!({"instances":[{"stats":{"hashrate_hs":70000}}]}),
            ),
        ]);
        assert_eq!(result.len(), 2);
        assert!(result[0].data.get("hardware").is_none());
        assert_eq!(result[0].data["logical_cpus"], 256);
        assert_eq!(result[0].error.as_deref(), Some("offline"));
        assert_eq!(result[0].observed_at, observed_at);
        assert_eq!(
            result[1].data["instances"][0]["stats"]["hashrate_hs"],
            70000
        );
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use serde_json::json;

    fn raw(now_uptime: f64, written: f64) -> Value {
        json!({"now_uptime": now_uptime, "snapshot": {
            "schema": 1, "boot_id": "b1", "uptime": written, "policy_digest": "d",
            "system": {"cpu_pct": 10},
            "mining": {"instances": [
                {"instance": "a", "boot_id": "b1", "sample_uptime": written - 2.0, "process_alive": true,
                 "stats_observed_at": 1.0, "observed_at": 1.0, "stats": {"hashrate_hs": 5}},
                {"instance": "old", "boot_id": "b0", "sample_uptime": 3.0, "process_alive": true,
                 "stats_observed_at": 1.0, "stats": {"hashrate_hs": 5}}
            ]},
            "events": [
                {"id": "00000000-0000-0000-0000-000000000001", "seq": 1, "boot_id": "b1", "uptime": written - 10.0, "at": 5.0},
                {"id": "00000000-0000-0000-0000-000000000002", "seq": 2, "boot_id": "b0", "uptime": 1.0, "at": 7.0}
            ]
        }})
    }

    #[test]
    fn converts_rig_uptime_to_controller_time_and_ignores_rig_wall_clock() {
        let now = chrono::DateTime::<Utc>::from_timestamp(1_000_000, 0).unwrap();
        let parsed = from_snapshot(&raw(1000.0, 995.0), now).unwrap();
        assert_eq!(parsed.observed_at, now - chrono::Duration::seconds(5));
        let a = &parsed.mining["instances"][0];
        assert_eq!(a["stats_observed_at"], json!(1_000_000.0 - 7.0));
        assert_eq!(a["observed_at"], json!(1_000_000.0 - 7.0));
        // A sample from a previous boot can never count as a live process.
        let old = &parsed.mining["instances"][1];
        assert_eq!(old["process_alive"], json!(false));
        assert!(old["stats"].is_null() && old["stats_observed_at"].is_null());
        assert_eq!(parsed.events[0]["at"], json!(1_000_000.0 - 15.0));
        assert_eq!(parsed.events[1]["at"], json!(7.0));
        assert_eq!(parsed.policy_digest.as_deref(), Some("d"));
    }

    #[test]
    fn kept_miner_numbers_age_from_when_they_were_read() {
        let now = chrono::DateTime::<Utc>::from_timestamp(1_000_000, 0).unwrap();
        let mut snapshot = raw(1000.0, 995.0);
        // Sampled at uptime 993 after a failed API read; the numbers kept are from uptime 983.
        snapshot["snapshot"]["mining"]["instances"][0]["stats_uptime"] = json!(983.0);
        let parsed = from_snapshot(&snapshot, now).unwrap();
        let a = &parsed.mining["instances"][0];
        assert_eq!(a["observed_at"], json!(1_000_000.0 - 7.0));
        assert_eq!(a["stats_observed_at"], json!(1_000_000.0 - 17.0));
    }

    #[test]
    fn stale_or_malformed_snapshots_fall_back_to_privileged_collection() {
        let now = Utc::now();
        assert!(from_snapshot(&raw(1000.0, 950.0), now).is_none());
        assert!(from_snapshot(&json!({"snapshot": {}}), now).is_none());
        assert!(from_snapshot(&raw(1000.0, 970.0), now).is_some());
    }

    #[test]
    fn policy_digest_is_key_order_independent() {
        let a: Value = serde_json::from_str(r#"{"b":1,"a":{"y":2,"x":1}}"#).unwrap();
        let b: Value = serde_json::from_str(r#"{"a":{"x":1,"y":2},"b":1}"#).unwrap();
        assert_eq!(policy_digest(&a), policy_digest(&b));
        assert_ne!(policy_digest(&a), policy_digest(&json!({})));
    }
}
