//! Offline detection: a machine is locally offline when no telemetry (push or SSH) has arrived
//! for `local_offline_minutes`, and pool-offline when it runs fine locally but its pool has not
//! seen the worker hash for `pool_offline_minutes`. Each transition raises one alert.
use crate::App;
use chrono::{DateTime, Utc};
use rig_domain::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use uuid::Uuid;

/// A machine whose newest good sample is younger than this is back online.
const FRESH: chrono::Duration = chrono::Duration::seconds(60);
/// A share this recent counts as hashing even when the pool's rate already reads 0.
const SHARE_WINDOW: chrono::Duration = chrono::Duration::minutes(5);
/// Pool numbers move about once a minute; one request per wallet per interval is plenty.
const POOL_REFRESH: Duration = Duration::from_secs(120);
const DEPLOYMENT_REFRESH: Duration = Duration::from_secs(60);

/// One miner instance as deployed: where it mines and with which login.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub instance: String,
    pub pool_url: String,
    pub coin: String,
    pub login: String,
    pub password: String,
}
/// Rendered instances of a deployment snapshot (`environment` holds the expanded templates).
pub fn targets(deployment: &Value) -> Vec<Target> {
    let text = |v: &Value| v.as_str().unwrap_or_default().to_owned();
    deployment["instances"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            let env = &item["environment"];
            Target {
                instance: text(&item["instance"]),
                pool_url: env["CUSTOM_URL"]
                    .as_str()
                    .and_then(|urls| urls.lines().next())
                    .unwrap_or_default()
                    .trim()
                    .to_owned(),
                coin: text(&item["coin"]),
                login: text(&env["CUSTOM_TEMPLATE"]),
                password: text(&env["CUSTOM_PASS"]),
            }
        })
        .collect()
}
/// Instances the rig says should be mining and whose miner process is alive.
pub fn running(mining: &Value) -> HashSet<String> {
    mining["instances"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|i| i["desired"] == "running" && i["process_alive"] == true)
        .filter_map(|i| i["instance"].as_str().map(str::to_owned))
        .collect()
}
/// Names a pool may list the worker under: the login suffix (`WALLET.rig01`, Kryptex and
/// Nanopool) and the password (SupportXMR and HashVault name workers by it; `x` means none).
pub fn worker_names(login: &str, password: &str) -> Vec<String> {
    let mut names = Vec::new();
    if let Some((_, worker)) = login.split_once(['.', '/']) {
        let worker = worker.split('+').next().unwrap_or_default().trim();
        if !worker.is_empty() {
            names.push(worker.to_owned());
        }
    }
    let password = password.split(':').next().unwrap_or_default().trim();
    if !password.is_empty()
        && !password.eq_ignore_ascii_case("x")
        && !names.iter().any(|n| n.eq_ignore_ascii_case(password))
    {
        names.push(password.to_owned());
    }
    names
}

/// The worker the pool lists this instance under, if any of its names matches.
pub fn find<'a>(target: &Target, workers: &'a PoolWorkers) -> Option<&'a PoolWorker> {
    let names = worker_names(&target.login, &target.password);
    workers
        .workers
        .iter()
        .find(|w| names.iter().any(|n| n.eq_ignore_ascii_case(&w.name)))
}

/// What one instance's pool says, given the pool answer for its account (if supported).
/// `names_line_up`: some machine on the same pool account is listed under its own worker name,
/// so a name the pool does not list really means that worker is gone. Otherwise the names are
/// not how this pool sees the miners (xmrig-proxy aggregating rigs into its own login, DNS
/// redirected to a proxy, workers named by password) and only the wallet total is evidence.
pub fn check(
    target: &Target,
    account: Option<&PoolAccount>,
    answer: Option<&std::result::Result<PoolWorkers, String>>,
    names_line_up: bool,
    now: DateTime<Utc>,
) -> PoolCheck {
    let names = worker_names(&target.login, &target.password);
    let mut result = PoolCheck {
        instance: target.instance.clone(),
        pool_url: Some(target.pool_url.clone()).filter(|u| !u.is_empty()),
        provider: account.map(|a| a.provider.clone()),
        worker: names.first().cloned(),
        result: PoolCheckResult::Unsupported,
        hashrate_hs: None,
        last_share_at: None,
        message: String::new(),
    };
    let Some(account) = account else {
        result.message = if target.pool_url.is_empty() {
            "未配置矿池地址，不做矿池检测".into()
        } else {
            "该矿池没有可用的矿工接口（自建代理或未支持的矿池），不做矿池检测".into()
        };
        return result;
    };
    let workers = match answer {
        None => {
            result.result = PoolCheckResult::Error;
            result.message = "等待矿池接口首次返回".into();
            return result;
        }
        Some(Err(error)) => {
            result.result = PoolCheckResult::Error;
            result.message = format!("读取矿池接口失败：{error}");
            return result;
        }
        Some(Ok(workers)) => workers,
    };
    let hashing = |rate: f64, share: Option<DateTime<Utc>>| {
        rate > 0.0 || share.is_some_and(|s| now - s <= SHARE_WINDOW)
    };
    if names.is_empty() {
        // No worker name: the pool can only tell us about the wallet as a whole.
        result.hashrate_hs = workers.total_hs;
        (result.result, result.message) = match workers.total_hs {
            Some(rate) if rate > 0.0 => (
                PoolCheckResult::Ok,
                "未设置 worker 名，按整个钱包的算力判断".into(),
            ),
            Some(_) => (
                PoolCheckResult::Zero,
                format!("{} 上这个钱包的算力为 0", account.provider),
            ),
            None => (PoolCheckResult::Error, "矿池没有返回钱包算力".into()),
        };
        return result;
    }
    let listed = names.join("」或「");
    match find(target, workers) {
        None if names_line_up => {
            result.result = PoolCheckResult::Missing;
            result.message = format!("{} 上看不到 worker「{listed}」", account.provider);
        }
        None => {
            result.hashrate_hs = workers.total_hs;
            (result.result, result.message) = match workers.total_hs {
                Some(rate) if rate > 0.0 => (
                    PoolCheckResult::Ambiguous,
                    format!(
                        "{} 上没有按 worker 名「{listed}」列出这台矿机，但钱包整体有算力：可能经过 xmrig-proxy 等代理汇总，或矿池不按这个名字区分 worker，无法单机确认",
                        account.provider
                    ),
                ),
                Some(_) => (
                    PoolCheckResult::Zero,
                    format!(
                        "{} 上找不到 worker「{listed}」，这个钱包的总算力也为 0",
                        account.provider
                    ),
                ),
                None => (PoolCheckResult::Error, "矿池没有返回钱包算力".into()),
            };
        }
        Some(worker) => {
            result.worker = Some(worker.name.clone());
            result.hashrate_hs = Some(worker.hashrate_hs);
            result.last_share_at = worker.last_share_at;
            (result.result, result.message) = if hashing(worker.hashrate_hs, worker.last_share_at) {
                (
                    PoolCheckResult::Ok,
                    format!("{} 已看到算力", account.provider),
                )
            } else {
                (
                    PoolCheckResult::Zero,
                    format!(
                        "{} 上 worker「{}」算力为 0，最近没有提交份额",
                        account.provider, worker.name
                    ),
                )
            };
        }
    }
    result
}

/// Inputs for one machine's next status.
pub struct Evidence<'a> {
    pub name: &'a str,
    pub previous: Option<&'a MachineStatus>,
    /// Newest error-free telemetry sample, if any.
    pub sample_at: Option<DateTime<Utc>>,
    /// Silence before this moment is not held against the machine: when it was added, or when
    /// this controller started (it could not have received anything while it was down).
    pub baseline: DateTime<Utc>,
    /// `None` keeps the previous pool verdict (machine offline locally, or nothing checked).
    pub pool_checks: Option<Vec<PoolCheck>>,
    pub settings: &'a AlertSettings,
    pub now: DateTime<Utc>,
}

pub fn evaluate(e: Evidence) -> (MachineStatus, Vec<NewAlert>) {
    let now = e.now;
    let previous = e.previous;
    let last_seen_at = match (previous.and_then(|p| p.last_seen_at), e.sample_at) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    let silent_since = last_seen_at.map_or(e.baseline, |t| t.max(e.baseline));
    let local_limit = chrono::Duration::minutes(e.settings.local_offline_minutes.into());
    let local = if last_seen_at.is_some_and(|t| now - t <= FRESH) {
        LocalState::Online
    } else if now - silent_since > local_limit {
        LocalState::Offline
    } else {
        // Inside the grace period nothing changes; a new machine starts online.
        previous.map_or(LocalState::Online, |p| p.local)
    };

    let pool_limit = chrono::Duration::minutes(e.settings.pool_offline_minutes.into());
    let kept = (
        previous.map_or(PoolState::Unknown, |p| p.pool),
        previous.and_then(|p| p.pool_bad_since),
        previous.map(|p| p.pool_checks.clone()).unwrap_or_default(),
    );
    let (pool, pool_bad_since, pool_checks) = match e.pool_checks {
        // Locally offline: the pool cannot tell us anything new about why.
        _ if local == LocalState::Offline => kept,
        None => kept,
        Some(checks) => {
            let bad = checks
                .iter()
                .any(|c| matches!(c.result, PoolCheckResult::Missing | PoolCheckResult::Zero));
            let ok = checks.iter().any(|c| c.result == PoolCheckResult::Ok);
            let settled = |state: PoolState| match state {
                PoolState::Online | PoolState::Offline => state,
                _ => PoolState::Unknown,
            };
            if checks.is_empty() {
                (PoolState::Unknown, None, checks)
            } else if bad {
                let since = kept.1.unwrap_or(now);
                let state = if now - since >= pool_limit {
                    PoolState::Offline
                } else {
                    settled(kept.0)
                };
                (state, Some(since), checks)
            } else if ok {
                (PoolState::Online, None, checks)
            } else if checks.iter().all(|c| {
                matches!(
                    c.result,
                    PoolCheckResult::Unsupported | PoolCheckResult::Ambiguous
                )
            }) {
                // Nothing here can be checked per machine: shown as not supported.
                (PoolState::Unsupported, None, checks)
            } else {
                // Only API errors (and unsupported pools): no evidence either way.
                (settled(kept.0), kept.1, checks)
            }
        }
    };
    let state = match (local, pool) {
        (LocalState::Offline, _) => MachineState::LocalOffline,
        (_, PoolState::Offline) => MachineState::PoolOffline,
        _ => MachineState::Online,
    };
    let status = MachineStatus {
        machine_id: previous.map_or(Uuid::nil(), |p| p.machine_id),
        state,
        local,
        last_seen_at,
        local_since: match previous {
            Some(p) if p.local == local => p.local_since,
            _ => now,
        },
        pool,
        pool_since: match previous {
            Some(p) if p.pool == pool => p.pool_since,
            _ => now,
        },
        pool_bad_since,
        pool_checks,
        checked_at: now,
    };

    let mut alerts = Vec::new();
    let alert = |kind, event, message: String| NewAlert {
        machine_id: status.machine_id,
        kind,
        event,
        at: now,
        message,
    };
    let was_local = previous.map(|p| p.local);
    if local == LocalState::Offline && was_local != Some(LocalState::Offline) {
        let minutes = (now - silent_since).num_minutes();
        alerts.push(alert(
            AlertKind::Local,
            AlertEvent::Offline,
            format!(
                "{} 已离线：{minutes} 分钟没有收到遥测数据（主动上报和 SSH 读取都没有结果）",
                e.name
            ),
        ));
    }
    if local == LocalState::Online
        && let Some(p) = previous.filter(|p| p.local == LocalState::Offline)
    {
        alerts.push(alert(
            AlertKind::Local,
            AlertEvent::Recovered,
            format!(
                "{} 已恢复在线（离线约 {} 分钟）",
                e.name,
                (now - p.local_since).num_minutes()
            ),
        ));
    }
    let was_pool = previous.map(|p| p.pool);
    if pool == PoolState::Offline && was_pool != Some(PoolState::Offline) {
        let reason = status
            .pool_checks
            .iter()
            .filter(|c| matches!(c.result, PoolCheckResult::Missing | PoolCheckResult::Zero))
            .map(|c| format!("{}：{}", c.instance, c.message))
            .collect::<Vec<_>>()
            .join("；");
        let minutes = pool_bad_since.map_or(0, |s| (now - s).num_minutes());
        alerts.push(alert(
            AlertKind::Pool,
            AlertEvent::Offline,
            format!(
                "{} 矿池离线：本机在运行，但矿池已 {minutes} 分钟没有看到算力（{reason}）",
                e.name
            ),
        ));
    }
    if pool == PoolState::Online
        && let Some(p) = previous.filter(|p| p.pool == PoolState::Offline)
    {
        alerts.push(alert(
            AlertKind::Pool,
            AlertEvent::Recovered,
            format!(
                "{} 矿池恢复：矿池已重新看到算力（离线约 {} 分钟）",
                e.name,
                (now - p.pool_since).num_minutes()
            ),
        ));
    }
    (status, alerts)
}

type PoolAnswer = (
    tokio::time::Instant,
    std::result::Result<PoolWorkers, String>,
);

impl App {
    pub async fn machine_statuses(&self) -> Result<Vec<MachineStatus>> {
        self.repository.machine_statuses().await
    }
    pub async fn status_alerts(&self, after: Option<i64>, limit: i64) -> Result<Vec<StatusAlert>> {
        self.repository.alerts(after, limit).await
    }
    /// Re-evaluate every machine each 10 s and persist statuses and alerts together.
    pub async fn run_status(self) {
        let started = Utc::now();
        let mut statuses: HashMap<Uuid, MachineStatus> = loop {
            match self.repository.machine_statuses().await {
                Ok(rows) => break rows.into_iter().map(|s| (s.machine_id, s)).collect(),
                Err(error) => {
                    tracing::warn!(%error, "machine status not loaded");
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
            }
        };
        let mut answers: HashMap<PoolAccount, PoolAnswer> = HashMap::new();
        let mut deployments: Option<(tokio::time::Instant, HashMap<Uuid, Vec<Target>>)> = None;
        let mut tick = tokio::time::interval(Duration::from_secs(10));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let settings = self.farm_settings().await.unwrap_or_default().alerts;
            let (Ok(machines), Ok(latest)) = (
                self.repository.machines().await,
                self.repository.latest_summary(None).await,
            ) else {
                continue;
            };
            let mut system = HashMap::new();
            let mut mining = HashMap::new();
            for o in latest {
                match o.kind.as_str() {
                    "system" => {
                        system.insert(o.machine_id, o);
                    }
                    "mining" => {
                        mining.insert(o.machine_id, o);
                    }
                    _ => {}
                }
            }
            let pools = self.pools.as_ref().filter(|_| settings.pool_check);
            if pools.is_some()
                && deployments
                    .as_ref()
                    .is_none_or(|(at, _)| at.elapsed() >= DEPLOYMENT_REFRESH)
            {
                match self.repository.current_deployments().await {
                    Ok(rows) => {
                        let map = rows.into_iter().map(|(id, d)| (id, targets(&d))).collect();
                        deployments = Some((tokio::time::Instant::now(), map));
                    }
                    Err(error) => tracing::warn!(%error, "deployments not read for pool checks"),
                }
            }
            // Instances to ask pools about: deployed and running on a machine that is reporting.
            let now = Utc::now();
            let mut wanted: HashMap<Uuid, Vec<(Target, Option<PoolAccount>)>> = HashMap::new();
            let mut line_up: HashSet<PoolAccount> = HashSet::new();
            if let (Some(pools), Some((_, deployed))) = (pools, &deployments) {
                for machine in &machines {
                    let (Some(sample), Some(targets)) =
                        (system.get(&machine.id), deployed.get(&machine.id))
                    else {
                        continue;
                    };
                    if sample.error.is_some() || now - sample.observed_at > FRESH {
                        continue;
                    }
                    let alive = mining
                        .get(&machine.id)
                        .filter(|m| m.error.is_none() && now - m.observed_at <= FRESH)
                        .map(|m| running(&m.data))
                        .unwrap_or_default();
                    let list = targets
                        .iter()
                        .filter(|t| alive.contains(&t.instance))
                        .map(|t| (t.clone(), pools.account(&t.pool_url, &t.coin, &t.login)))
                        .collect();
                    wanted.insert(machine.id, list);
                }
                let due: HashSet<PoolAccount> = wanted
                    .values()
                    .flatten()
                    .filter_map(|(_, a)| a.clone())
                    .filter(|a| {
                        answers
                            .get(a)
                            .is_none_or(|(at, _)| at.elapsed() >= POOL_REFRESH)
                    })
                    .collect();
                let fetched =
                    futures_util::future::join_all(due.into_iter().map(|account| async move {
                        let result =
                            tokio::time::timeout(Duration::from_secs(20), pools.workers(&account))
                                .await
                                .unwrap_or_else(|_| Err(Error::Unavailable("矿池接口超时".into())));
                        (account, result.map_err(|e| e.to_string()))
                    }))
                    .await;
                for (account, result) in fetched {
                    if let Err(error) = &result {
                        tracing::warn!(%error, pool = %account.provider, "pool worker stats not read");
                    }
                    answers.insert(account, (tokio::time::Instant::now(), result));
                }
                let used: HashSet<&PoolAccount> = wanted
                    .values()
                    .flatten()
                    .filter_map(|(_, a)| a.as_ref())
                    .collect();
                answers.retain(|a, _| used.contains(a));
                // Accounts where at least one machine is listed under its own worker name.
                for (target, account) in wanted.values().flatten() {
                    if let Some(account) = account
                        && let Some((_, Ok(workers))) = answers.get(account)
                        && find(target, workers).is_some()
                    {
                        line_up.insert(account.clone());
                    }
                }
            }

            let ids: HashSet<Uuid> = machines.iter().map(|m| m.id).collect();
            statuses.retain(|id, _| ids.contains(id));
            let mut saved = Vec::with_capacity(machines.len());
            let mut alerts = Vec::new();
            for machine in &machines {
                let previous = statuses.get(&machine.id);
                let checks = if pools.is_none() {
                    Some(Vec::new())
                } else {
                    wanted.get(&machine.id).map(|list| {
                        list.iter()
                            .map(|(t, a)| {
                                check(
                                    t,
                                    a.as_ref(),
                                    a.as_ref().and_then(|a| answers.get(a)).map(|(_, r)| r),
                                    a.as_ref().is_some_and(|a| line_up.contains(a)),
                                    now,
                                )
                            })
                            .collect()
                    })
                };
                let (mut status, raised) = evaluate(Evidence {
                    name: &machine.name,
                    previous,
                    sample_at: system
                        .get(&machine.id)
                        .filter(|o| o.error.is_none() && !o.data.is_null())
                        .map(|o| o.observed_at),
                    baseline: machine.created_at.max(started),
                    pool_checks: checks,
                    settings: &settings,
                    now,
                });
                status.machine_id = machine.id;
                alerts.extend(raised.into_iter().map(|a| NewAlert {
                    machine_id: machine.id,
                    ..a
                }));
                saved.push(status);
            }
            match self.repository.save_statuses(&saved, &alerts).await {
                Ok(()) => {
                    for alert in &alerts {
                        tracing::info!(machine = %alert.machine_id, message = %alert.message, "status alert");
                    }
                    statuses = saved.into_iter().map(|s| (s.machine_id, s)).collect();
                }
                // Keep the old statuses so the same transitions (and alerts) are retried.
                Err(error) => tracing::warn!(%error, "machine status not saved"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as D;
    use serde_json::json;

    const WALLET: &str = "44AFFq5kSiGBoZ4NMDwYtN18obc8AemS33DBLWs3H7otXft3XjrpDtQGv7SqSsaBYBb98uNbr2VBBEt7f2wfn3RVGQBEP3A";

    fn t0() -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).unwrap()
    }
    fn target(login: &str, password: &str) -> Target {
        Target {
            instance: "xmr".into(),
            pool_url: "stratum+ssl://pool.supportxmr.com:443".into(),
            coin: "XMR".into(),
            login: login.into(),
            password: password.into(),
        }
    }
    fn account() -> PoolAccount {
        PoolAccount {
            provider: "supportxmr".into(),
            coin: "xmr".into(),
            wallet: WALLET.into(),
        }
    }
    fn worker(name: &str, rate: f64, share: Option<DateTime<Utc>>) -> PoolWorker {
        PoolWorker {
            name: name.into(),
            hashrate_hs: rate,
            last_share_at: share,
        }
    }
    fn pool_check(result: PoolCheckResult) -> PoolCheck {
        PoolCheck {
            instance: "xmr".into(),
            pool_url: None,
            provider: Some("supportxmr".into()),
            worker: Some("rig01".into()),
            result,
            hashrate_hs: None,
            last_share_at: None,
            message: "m".into(),
        }
    }

    /// Drive `evaluate` like the loop does: one call per step, carrying the status forward.
    struct Sim {
        settings: AlertSettings,
        status: Option<MachineStatus>,
        alerts: Vec<NewAlert>,
    }
    impl Sim {
        fn new() -> Self {
            Self {
                settings: AlertSettings::default(),
                status: None,
                alerts: Vec::new(),
            }
        }
        fn step(
            &mut self,
            at: D,
            seen: Option<D>,
            checks: Option<Vec<PoolCheck>>,
        ) -> &MachineStatus {
            let (status, alerts) = evaluate(Evidence {
                name: "rig01",
                previous: self.status.as_ref(),
                sample_at: seen.map(|s| t0() + s),
                baseline: t0(),
                pool_checks: checks,
                settings: &self.settings,
                now: t0() + at,
            });
            self.alerts.extend(alerts);
            self.status = Some(status);
            self.status.as_ref().unwrap()
        }
        fn events(&self) -> Vec<(AlertKind, AlertEvent)> {
            self.alerts.iter().map(|a| (a.kind, a.event)).collect()
        }
    }

    #[test]
    fn local_offline_waits_for_the_threshold_and_alerts_once_each_way() {
        let mut sim = Sim::new();
        assert_eq!(
            sim.step(D::seconds(0), Some(D::seconds(0)), None).local,
            LocalState::Online
        );
        // Silent for 1–2 minutes: still within the 2-minute threshold, no flapping.
        for s in [30, 60, 90, 120] {
            assert_eq!(
                sim.step(D::seconds(s), None, None).local,
                LocalState::Online,
                "{s}s"
            );
        }
        let status = sim.step(D::seconds(130), None, None);
        assert_eq!(status.state, MachineState::LocalOffline);
        assert_eq!(status.local_since, t0() + D::seconds(130));
        for s in [140, 300, 3600] {
            sim.step(D::seconds(s), None, None);
        }
        assert_eq!(sim.events(), [(AlertKind::Local, AlertEvent::Offline)]);
        // An old sample arriving late is not a recovery; a fresh one is.
        assert_eq!(
            sim.step(D::seconds(3610), Some(D::seconds(10)), None).local,
            LocalState::Offline
        );
        assert_eq!(
            sim.step(D::seconds(3620), Some(D::seconds(3615)), None)
                .local,
            LocalState::Online
        );
        sim.step(D::seconds(3630), Some(D::seconds(3625)), None);
        assert_eq!(
            sim.events(),
            [
                (AlertKind::Local, AlertEvent::Offline),
                (AlertKind::Local, AlertEvent::Recovered)
            ]
        );
        assert!(
            sim.alerts[1].message.contains("离线约 58 分钟"),
            "{}",
            sim.alerts[1].message
        );
    }

    #[test]
    fn silence_from_before_the_baseline_does_not_count() {
        // The controller was down for an hour: the stored last_seen is old, but the machine
        // gets the full threshold from the controller start before it is declared offline.
        let mut sim = Sim::new();
        sim.step(D::seconds(-3600), Some(D::seconds(-3600)), None);
        assert_eq!(
            sim.step(D::seconds(60), None, None).local,
            LocalState::Online
        );
        assert_eq!(
            sim.step(D::seconds(121), None, None).local,
            LocalState::Offline
        );
        // A machine never heard from at all is reported once the threshold passes.
        let mut never = Sim::new();
        assert_eq!(
            never.step(D::seconds(10), None, None).local,
            LocalState::Online
        );
        assert_eq!(
            never.step(D::seconds(130), None, None).local,
            LocalState::Offline
        );
        assert_eq!(never.events(), [(AlertKind::Local, AlertEvent::Offline)]);
    }

    #[test]
    fn pool_offline_needs_a_sustained_bad_reading_and_recovers_once() {
        let mut sim = Sim::new();
        let ok = || Some(vec![pool_check(PoolCheckResult::Ok)]);
        let zero = || Some(vec![pool_check(PoolCheckResult::Zero)]);
        let error = || Some(vec![pool_check(PoolCheckResult::Error)]);
        assert_eq!(
            sim.step(D::minutes(0), Some(D::minutes(0)), ok()).pool,
            PoolState::Online
        );
        // Bad for 9 minutes (with an API error in between): not yet offline.
        for m in 1..=9 {
            let checks = if m == 5 { error() } else { zero() };
            let status = sim.step(D::minutes(m), Some(D::minutes(m)), checks);
            assert_eq!(status.pool, PoolState::Online, "{m} min");
        }
        assert_eq!(
            sim.status.as_ref().unwrap().pool_bad_since,
            Some(t0() + D::minutes(1))
        );
        let status = sim.step(D::minutes(11), Some(D::minutes(11)), zero());
        assert_eq!(
            (status.pool, status.state),
            (PoolState::Offline, MachineState::PoolOffline)
        );
        sim.step(D::minutes(12), Some(D::minutes(12)), zero());
        // API errors never recover (or re-raise) anything.
        assert_eq!(
            sim.step(D::minutes(13), Some(D::minutes(13)), error()).pool,
            PoolState::Offline
        );
        assert_eq!(
            sim.step(D::minutes(14), Some(D::minutes(14)), ok()).pool,
            PoolState::Online
        );
        assert_eq!(
            sim.events(),
            [
                (AlertKind::Pool, AlertEvent::Offline),
                (AlertKind::Pool, AlertEvent::Recovered)
            ]
        );
        assert!(
            sim.alerts[0].message.contains("已 10 分钟没有看到算力"),
            "{}",
            sim.alerts[0].message
        );
        // A single good reading resets the debounce.
        sim.step(D::minutes(20), Some(D::minutes(20)), zero());
        sim.step(D::minutes(25), Some(D::minutes(25)), ok());
        assert_eq!(
            sim.step(D::minutes(31), Some(D::minutes(31)), zero()).pool,
            PoolState::Online
        );
        assert_eq!(sim.events().len(), 2);
    }

    #[test]
    fn unsupported_pools_never_alert_and_local_offline_freezes_the_pool_verdict() {
        let mut sim = Sim::new();
        let unsupported = || Some(vec![pool_check(PoolCheckResult::Unsupported)]);
        for m in 0..60 {
            let status = sim.step(D::minutes(m), Some(D::minutes(m)), unsupported());
            assert_eq!(
                (status.pool, status.state),
                (PoolState::Unsupported, MachineState::Online)
            );
        }
        assert!(sim.alerts.is_empty());

        let mut sim = Sim::new();
        sim.step(
            D::minutes(0),
            Some(D::minutes(0)),
            Some(vec![pool_check(PoolCheckResult::Ok)]),
        );
        // The rig goes dark: one local alert, no pool alert, pool verdict kept as it was.
        let status = sim.step(D::minutes(30), None, None);
        assert_eq!(
            (status.state, status.pool),
            (MachineState::LocalOffline, PoolState::Online)
        );
        let status = sim.step(
            D::minutes(31),
            Some(D::minutes(31)),
            Some(vec![pool_check(PoolCheckResult::Ok)]),
        );
        assert_eq!(status.state, MachineState::Online);
        assert_eq!(
            sim.events(),
            [
                (AlertKind::Local, AlertEvent::Offline),
                (AlertKind::Local, AlertEvent::Recovered)
            ]
        );
        // Nothing running: the pool verdict is unknown, not offline.
        assert_eq!(
            sim.step(D::minutes(32), Some(D::minutes(32)), Some(vec![]))
                .pool,
            PoolState::Unknown
        );
    }

    #[test]
    fn pool_check_matches_login_suffix_or_password_worker_names() {
        let now = t0();
        let answer = |workers| {
            Some(Ok(PoolWorkers {
                workers,
                total_hs: Some(7000.0),
            }))
        };
        let hashing = answer(vec![worker("RIG01", 7000.0, None)]);
        let login = format!("{WALLET}.rig01");
        let result = check(
            &target(&login, "x"),
            Some(&account()),
            hashing.as_ref(),
            true,
            now,
        );
        assert_eq!(
            (result.result, result.worker.as_deref()),
            (PoolCheckResult::Ok, Some("RIG01"))
        );
        // SupportXMR/HashVault style: worker in the password field.
        let result = check(
            &target(WALLET, "rig01:me@example.com"),
            Some(&account()),
            hashing.as_ref(),
            true,
            now,
        );
        assert_eq!(result.result, PoolCheckResult::Ok);
        // Rate 0 but a share 2 minutes ago still counts; 10 minutes ago does not.
        let recent = answer(vec![worker("rig01", 0.0, Some(now - D::minutes(2)))]);
        assert_eq!(
            check(
                &target(&login, "x"),
                Some(&account()),
                recent.as_ref(),
                true,
                now
            )
            .result,
            PoolCheckResult::Ok
        );
        let stale = answer(vec![worker("rig01", 0.0, Some(now - D::minutes(10)))]);
        assert_eq!(
            check(
                &target(&login, "x"),
                Some(&account()),
                stale.as_ref(),
                true,
                now
            )
            .result,
            PoolCheckResult::Zero
        );
        let other = answer(vec![worker("rig02", 9000.0, None)]);
        let missing = check(
            &target(&login, "x"),
            Some(&account()),
            other.as_ref(),
            true,
            now,
        );
        assert_eq!(missing.result, PoolCheckResult::Missing);
        assert!(missing.message.contains("rig01"), "{}", missing.message);
        // No worker name at all: judged by the wallet total.
        let total = |rate| {
            Some(Ok(PoolWorkers {
                workers: vec![],
                total_hs: Some(rate),
            }))
        };
        assert_eq!(
            check(
                &target(WALLET, "x"),
                Some(&account()),
                total(10.0).as_ref(),
                true,
                now
            )
            .result,
            PoolCheckResult::Ok
        );
        assert_eq!(
            check(
                &target(WALLET, "x"),
                Some(&account()),
                total(0.0).as_ref(),
                true,
                now
            )
            .result,
            PoolCheckResult::Zero
        );
        // Errors and unsupported pools are neutral.
        let failed = Some(Err("HTTP 502".to_owned()));
        assert_eq!(
            check(
                &target(&login, "x"),
                Some(&account()),
                failed.as_ref(),
                true,
                now
            )
            .result,
            PoolCheckResult::Error
        );
        assert_eq!(
            check(&target(&login, "x"), Some(&account()), None, true, now).result,
            PoolCheckResult::Error
        );
        let proxy = Target {
            pool_url: "mac.icuicu.de:6666".into(),
            ..target(&login, "x")
        };
        let result = check(&proxy, None, None, true, now);
        assert_eq!(result.result, PoolCheckResult::Unsupported);
        assert!(result.message.contains("不做矿池检测"));
    }

    #[test]
    fn unlisted_names_only_count_when_the_account_lists_its_machines_by_name() {
        // xmrig-proxy in front of the pool (or DNS pointing the pool host at it): the pool
        // sees one worker named after the proxy's login, never rig01.
        let now = t0();
        let login = format!("{WALLET}.rig01");
        let proxied = |total| {
            Some(Ok(PoolWorkers {
                workers: vec![worker("proxy", total, None)],
                total_hs: Some(total),
            }))
        };
        let result = check(
            &target(&login, "x"),
            Some(&account()),
            proxied(52000.0).as_ref(),
            false,
            now,
        );
        assert_eq!(result.result, PoolCheckResult::Ambiguous);
        assert!(result.message.contains("xmrig-proxy"), "{}", result.message);
        // The whole wallet stopped hashing: the rig's work is not reaching the pool either way.
        let result = check(
            &target(&login, "x"),
            Some(&account()),
            proxied(0.0).as_ref(),
            false,
            now,
        );
        assert_eq!(result.result, PoolCheckResult::Zero);
        assert!(
            result.message.contains("总算力也为 0"),
            "{}",
            result.message
        );
        // Once another machine on the account is listed by name, an unlisted name is missing.
        let result = check(
            &target(&login, "x"),
            Some(&account()),
            proxied(52000.0).as_ref(),
            true,
            now,
        );
        assert_eq!(result.result, PoolCheckResult::Missing);

        // A machine that can only be judged ambiguously never goes pool-offline.
        let mut sim = Sim::new();
        for m in 0..60 {
            let checks = Some(vec![pool_check(PoolCheckResult::Ambiguous)]);
            let status = sim.step(D::minutes(m), Some(D::minutes(m)), checks);
            assert_eq!(
                (status.pool, status.state),
                (PoolState::Unsupported, MachineState::Online)
            );
        }
        assert!(sim.alerts.is_empty());
    }

    #[test]
    fn worker_names_skip_placeholder_passwords_and_difficulty() {
        assert_eq!(
            worker_names(&format!("{WALLET}.rig01+50000"), "x"),
            ["rig01"]
        );
        assert_eq!(worker_names(&format!("{WALLET}/rig01"), "X"), ["rig01"]);
        assert_eq!(worker_names(WALLET, "rig02:mail@example.com"), ["rig02"]);
        assert_eq!(worker_names(&format!("{WALLET}.rig01"), "rig01"), ["rig01"]);
        assert!(worker_names(WALLET, "").is_empty());
    }

    #[test]
    fn targets_come_from_the_rendered_deployment_and_running_from_the_rig() {
        let deployment = json!({"sheet_id":"s","instances":[
            {"instance":"xmr","coin":"XMR","pool_urls":["stratum+ssl://pool.supportxmr.com:443","b:1"],
             "environment":{"CUSTOM_TEMPLATE":format!("{WALLET}.rig01"),"CUSTOM_PASS":"x","CUSTOM_URL":"stratum+ssl://pool.supportxmr.com:443\nb:1"}},
            {"instance":"custom","coin":"XMR","environment":{"CUSTOM_TEMPLATE":"","CUSTOM_PASS":"x","CUSTOM_URL":""}}
        ]});
        let list = targets(&deployment);
        assert_eq!(list[0].pool_url, "stratum+ssl://pool.supportxmr.com:443");
        assert_eq!(list[0].login, format!("{WALLET}.rig01"));
        assert_eq!(list[1].pool_url, "");
        let mining = json!({"instances":[
            {"instance":"xmr","desired":"running","process_alive":true},
            {"instance":"custom","desired":"stopped","process_alive":false},
            {"instance":"warm","desired":"running","process_alive":false}
        ]});
        assert_eq!(running(&mining), HashSet::from(["xmr".to_owned()]));
    }
}
