use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BiosSetting {
    pub id: String,
    pub name: String,
    pub group: String,
    pub value: String,
    pub kind: String,
    pub options: Vec<String>,
    pub minimum: Option<i64>,
    pub maximum: Option<i64>,
    pub step: Option<i64>,
    pub help: String,
    pub condition: String,
    pub license: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BiosSnapshot {
    pub machine_id: Uuid,
    pub settings: Vec<BiosSetting>,
    pub licenses: Vec<String>,
    pub revision: Option<String>,
    pub pending: std::collections::BTreeMap<String, String>,
    pub observed_at: Option<String>,
    pub last_operation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Actor {
    pub id: Uuid,
    pub name: String,
    pub token_id: Option<Uuid>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Session {
    pub actor: Actor,
    pub csrf: String,
    pub expires_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Login {
    pub username: String,
    pub password: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TokenInfo {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshProbeInput {
    pub host: String,
    pub port: u16,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshHostKey {
    pub fingerprint: String,
    pub algorithm: String,
}
/// Up to 256 endpoints whose SSH host keys are fetched in parallel for one review screen.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshProbeBatch {
    pub targets: Vec<SshProbeInput>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SshProbeResult {
    pub host: String,
    pub port: u16,
    pub fingerprint: Option<String>,
    pub algorithm: Option<String>,
    pub error: Option<String>,
}
/// Batch onboarding: every row carries its own verified host key; credentials are usually shared.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MachineBatchInput {
    pub machines: Vec<MachineInput>,
    /// Queue one "deploy runtime" job for every machine that was created.
    #[serde(default)]
    pub bootstrap: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MachineBatchRow {
    pub name: String,
    pub host: String,
    pub machine_id: Option<Uuid>,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MachineBatchResult {
    pub results: Vec<MachineBatchRow>,
    pub bootstrap_job_id: Option<Uuid>,
    /// The machines were created, but queueing the runtime deployment failed.
    pub bootstrap_error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Machine {
    pub id: Uuid,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub host_key: String,
    pub group: String,
    pub tags: Vec<String>,
    pub is_controller: bool,
    pub bmc: Option<BmcConfig>,
    pub policy: Value,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MachineInput {
    pub name: String,
    pub host: String,
    #[serde(default = "ssh_port")]
    pub port: u16,
    #[serde(default = "ssh_user")]
    pub username: String,
    pub host_key: String,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub is_controller: bool,
    pub credential: Option<Credential>,
    /// Optional privilege password. Omitted on edit to retain the saved value.
    pub sudo_password: Option<String>,
    pub bmc: Option<BmcConfig>,
    pub bmc_credential: Option<Credential>,
    #[serde(default = "empty_object")]
    pub policy: Value,
}
fn ssh_port() -> u16 {
    22
}
fn ssh_user() -> String {
    "root".into()
}
pub fn empty_object() -> Value {
    serde_json::json!({})
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Credential {
    Password {
        password: String,
        #[serde(default)]
        sudo_password: Option<String>,
    },
    PrivateKey {
        private_key: String,
        passphrase: Option<String>,
        #[serde(default)]
        sudo_password: Option<String>,
    },
}
impl Credential {
    pub fn explicit_sudo_password(&self) -> Option<&str> {
        match self {
            Self::Password { sudo_password, .. } | Self::PrivateKey { sudo_password, .. } => {
                sudo_password.as_deref()
            }
        }
    }
    pub fn sudo_password(&self) -> Option<&str> {
        match self {
            Self::Password {
                password,
                sudo_password,
            } => sudo_password.as_deref().or(Some(password)),
            Self::PrivateKey { sudo_password, .. } => sudo_password.as_deref(),
        }
    }
    pub fn set_sudo_password(&mut self, value: Option<String>) {
        match self {
            Self::Password { sudo_password, .. } | Self::PrivateKey { sudo_password, .. } => {
                *sudo_password = value
            }
        }
    }
}
impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credential([redacted])")
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BmcConfig {
    pub url: String,
    pub username: String,
    #[serde(default = "bmc_provider")]
    pub provider: String,
    pub ca_pem: Option<String>,
}
fn bmc_provider() -> String {
    "redfish".into()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CatalogKind {
    Coin,
    Wallet,
    Pool,
    Miner,
    /// HiveOS-style overclocking template; `data` is a [`GpuOcConfig`].
    OcProfile,
}
impl CatalogKind {
    pub fn parse(s: &str) -> crate::Result<Self> {
        match s {
            "coins" | "coin" => Ok(Self::Coin),
            "wallets" | "wallet" => Ok(Self::Wallet),
            "pools" | "pool" => Ok(Self::Pool),
            "miners" | "miner" => Ok(Self::Miner),
            "oc-profiles" | "oc_profiles" | "oc_profile" => Ok(Self::OcProfile),
            _ => Err(crate::Error::NotFound),
        }
    }
    pub fn table(self) -> &'static str {
        match self {
            Self::Coin => "coins",
            Self::Wallet => "wallets",
            Self::Pool => "pools",
            Self::Miner => "miners",
            Self::OcProfile => "oc_profiles",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CatalogItem {
    pub id: Uuid,
    pub kind: CatalogKind,
    pub name: String,
    pub data: Value,
    pub revision: i32,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CatalogInput {
    pub name: String,
    pub data: Value,
    pub expected_revision: Option<i32>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FlightTask {
    pub instance: String,
    pub wallet_id: Uuid,
    /// Like HiveOS: which miner and where the rig downloads it from. `adapter`, `name`,
    /// `version`, `url`, optional `sha256`, `executable` (native adapters). Rigs fetch the
    /// package themselves; the controller never stores or forwards it.
    pub miner: Value,
    /// Pool URLs (`urls`, first is primary), wallet template, password, algorithm, extra args.
    #[serde(default = "empty_object")]
    pub config: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FlightSheet {
    #[serde(default)]
    pub apply_job_id: Option<Uuid>,
    pub id: Uuid,
    pub name: String,
    pub version: i32,
    pub tasks: Vec<FlightTask>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FlightInput {
    pub name: String,
    pub tasks: Vec<FlightTask>,
    pub expected_version: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    BiosRead {
        /// After a reboot, accept the read-back values and drop the pending marker.
        #[serde(default)]
        discard_pending: bool,
    },
    BiosWrite {
        revision: String,
        changes: std::collections::BTreeMap<String, String>,
    },
    Bootstrap,
    Command {
        script: String,
        #[serde(default = "command_timeout")]
        timeout_seconds: u32,
    },
    Apply {
        sheet_id: Uuid,
    },
    Miner {
        operation: String,
        instances: Vec<String>,
    },
    Power {
        operation: String,
    },
    /// Applies overclocking on the rig and keeps it for re-application after reboot.
    GpuOc {
        #[serde(default)]
        profile_name: Option<String>,
        config: Box<GpuOcConfig>,
    },
    /// Restores driver defaults and forgets the saved overclock.
    GpuOcReset,
    Adopt {
        name: String,
        pid: u32,
        start_identity: String,
        start_script: String,
        stop_script: String,
        pid_script: String,
    },
}
fn command_timeout() -> u32 {
    300
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct JobInput {
    pub machine_ids: Vec<Uuid>,
    pub action: Action,
    pub idempotency_key: String,
    #[serde(default = "job_concurrency")]
    pub concurrency: u8,
    #[serde(default = "yes")]
    pub canary: bool,
    #[serde(default)]
    pub include_controller: bool,
}
fn job_concurrency() -> u8 {
    4
}
fn yes() -> bool {
    true
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Job {
    pub id: Uuid,
    pub action: Value,
    pub status: String,
    pub concurrency: i32,
    pub canary: bool,
    pub created_at: DateTime<Utc>,
    pub actor: String,
    pub targets: Vec<JobTarget>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct JobTarget {
    pub id: Uuid,
    pub job_id: Uuid,
    pub machine_id: Uuid,
    pub machine_name: String,
    pub status: String,
    pub ordinal: i32,
    pub operation_id: Uuid,
    pub output: String,
    pub output_truncated: bool,
    pub result: Option<Value>,
    pub error: Option<String>,
}
#[derive(Debug, Clone)]
pub struct ClaimedTask {
    pub target: JobTarget,
    pub action: Value,
    pub lease: Uuid,
    pub reconcile: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Observation {
    pub machine_id: Uuid,
    pub kind: String,
    pub observed_at: DateTime<Utc>,
    pub data: Value,
    pub error: Option<String>,
}
/// One line of the HiveOS-style per-machine message feed: runtime events, job results and audit.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MessageDismiss {
    /// `source:id` of one message; omit to clear every chip for the machine.
    pub key: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MachineMessage {
    pub id: String,
    pub at: DateTime<Utc>,
    /// info | success | warning | error
    pub level: String,
    /// runtime | job | audit
    pub source: String,
    pub kind: String,
    pub instance: Option<String>,
    pub message: String,
    pub detail: Option<Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MinerLog {
    pub instance: String,
    pub text: String,
    pub size: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AuditEvent {
    pub id: i64,
    pub actor: String,
    pub action: String,
    pub target: String,
    pub detail: Value,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AdapterDescription {
    pub id: String,
    pub name: String,
    pub schema_version: u32,
    pub parameters: Value,
    pub capabilities: Value,
    /// Official Linux builds offered in the flight sheet, newest first. Empty for adapters
    /// that only run a custom download URL.
    pub releases: Vec<MinerRelease>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MinerRelease {
    pub version: String,
    pub url: String,
    /// Path inside the archive (after its single top-level directory is stripped).
    pub executable: String,
    pub sha256: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResolveTarget {
    pub decision: String,
    pub note: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<u32>,
    pub truncated: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteOperation {
    pub status: String,
    pub output: String,
    #[serde(default)]
    pub truncated: bool,
    pub result: Option<Value>,
    pub error: Option<String>,
}
#[derive(Debug, Clone)]
pub enum TerminalInput {
    Data(Vec<u8>),
    Resize(u32, u32),
    Close,
}
#[derive(Debug, Clone)]
pub enum TerminalOutput {
    Data(Vec<u8>),
    Closed(Option<u32>),
    Error(String),
}

#[derive(Debug, Clone)]
pub struct FlightPropagation {
    pub job: JobInput,
    pub snapshot: Value,
}

/// Overclocking in HiveOS's own terms (`nvidia-oc.conf` / `amd-oc.conf`). Every list holds one
/// value per card of that vendor in PCI bus order; a shorter list repeats its last value, so a
/// single value applies to all cards, and an empty list leaves the setting untouched.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct GpuOcConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nvidia: Option<NvidiaOc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amd: Option<AmdOc>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct NvidiaOc {
    /// CLOCK: core offset in MHz; a value above 500 is a locked core clock.
    #[serde(default)]
    pub clock: Vec<i32>,
    /// MEM: memory offset in MHz, Linux/HiveOS scale (twice the Windows Afterburner value).
    #[serde(default)]
    pub mem: Vec<i32>,
    /// PLIMIT: power limit in W, 0 = driver default.
    #[serde(default)]
    pub plimit: Vec<i32>,
    /// FAN: percent, 0 = automatic.
    #[serde(default)]
    pub fan: Vec<i32>,
    /// RUNNING_DELAY: seconds to wait after boot before re-applying, like HiveOS.
    #[serde(default)]
    pub running_delay: u32,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct AmdOc {
    /// CORE_CLOCK, MHz.
    #[serde(default)]
    pub core_clock: Vec<i32>,
    /// CORE_STATE: DPM state to set and pin, 0 = auto.
    #[serde(default)]
    pub core_state: Vec<i32>,
    /// CORE_VDDC, mV.
    #[serde(default)]
    pub core_vddc: Vec<i32>,
    /// MEM_CLOCK, MHz.
    #[serde(default)]
    pub mem_clock: Vec<i32>,
    /// MEM_STATE, 0 = auto.
    #[serde(default)]
    pub mem_state: Vec<i32>,
    /// MVDD, mV (PowerPlay table; accepted for HiveOS import, not written in this release).
    #[serde(default)]
    pub mvdd: Vec<i32>,
    /// VDDCI, mV (PowerPlay table; not written in this release).
    #[serde(default)]
    pub vddci: Vec<i32>,
    /// SOCCLK, MHz (PowerPlay table; not written in this release).
    #[serde(default)]
    pub soc_clk: Vec<i32>,
    /// SOCVDDMAX, mV (PowerPlay table; not written in this release).
    #[serde(default)]
    pub soc_vdd_max: Vec<i32>,
    /// REF: memory refresh via amdmemtweak when it is installed on the rig.
    #[serde(default, rename = "ref")]
    pub ref_: Vec<i32>,
    /// FAN: percent, 0 = automatic.
    #[serde(default)]
    pub fan: Vec<i32>,
    /// PL: power limit in W, 0 = driver default.
    #[serde(default)]
    pub pl: Vec<i32>,
}

pub const MAX_OC_CARDS: usize = 32;

fn oc_field(label: &str, values: &[i32], low: i32, high: i32, zero: bool) -> crate::Result<()> {
    if values.len() > MAX_OC_CARDS {
        return Err(crate::Error::Validation(format!(
            "{label} 最多 {MAX_OC_CARDS} 张卡"
        )));
    }
    if let Some(v) = values
        .iter()
        .find(|&&v| !((low..=high).contains(&v) || zero && v == 0))
    {
        return Err(crate::Error::Validation(format!(
            "{label} = {v} 超出允许范围 {low}–{high}{}",
            if zero {
                "（0 表示默认/不修改）"
            } else {
                ""
            }
        )));
    }
    Ok(())
}

impl GpuOcConfig {
    /// Rejects typing mistakes. The bounds are input limits, not recommended settings.
    pub fn validate(&self) -> crate::Result<()> {
        if self.nvidia.is_none() && self.amd.is_none() {
            return Err(crate::Error::Validation(
                "至少填写 NVIDIA 或 AMD 一组超频参数".into(),
            ));
        }
        if let Some(n) = &self.nvidia {
            oc_field("NVIDIA 核心", &n.clock, -1000, 4000, false)?;
            oc_field("NVIDIA 显存", &n.mem, -5000, 5000, false)?;
            oc_field("NVIDIA 功耗墙", &n.plimit, 1, 1000, true)?;
            oc_field("NVIDIA 风扇", &n.fan, 0, 100, false)?;
            if n.running_delay > 300 {
                return Err(crate::Error::Validation("延迟最多 300 秒".into()));
            }
        }
        if let Some(a) = &self.amd {
            oc_field("AMD 核心频率", &a.core_clock, 300, 4000, true)?;
            oc_field("AMD 核心状态", &a.core_state, 0, 15, false)?;
            oc_field("AMD 核心电压", &a.core_vddc, 500, 1300, true)?;
            oc_field("AMD 显存频率", &a.mem_clock, 90, 3000, true)?;
            oc_field("AMD 显存状态", &a.mem_state, 0, 15, false)?;
            oc_field("AMD MVDD", &a.mvdd, 1000, 1500, true)?;
            oc_field("AMD VDDCI", &a.vddci, 500, 1000, true)?;
            oc_field("AMD SOC 频率", &a.soc_clk, 300, 2000, true)?;
            oc_field("AMD SOC 电压", &a.soc_vdd_max, 500, 1300, true)?;
            oc_field("AMD REF", &a.ref_, 0, 65535, false)?;
            oc_field("AMD 风扇", &a.fan, 0, 100, false)?;
            oc_field("AMD 功耗墙", &a.pl, 1, 1000, true)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod oc_tests {
    use super::*;

    #[test]
    fn hiveos_values_validate_and_mistakes_are_rejected() {
        let config: GpuOcConfig = serde_json::from_value(serde_json::json!({
            "nvidia": {"clock": [1500, -200], "mem": [2400], "plimit": [0, 220], "fan": [0]},
            "amd": {"core_clock": [1150], "core_vddc": [850], "ref": [30]}
        }))
        .unwrap();
        config.validate().unwrap();
        assert_eq!(config.amd.as_ref().unwrap().ref_, vec![30]);
        let action: Action = serde_json::from_value(
            serde_json::json!({"kind":"gpu_oc","config":{"nvidia":{"fan":[101]}}}),
        )
        .unwrap();
        let Action::GpuOc { config, .. } = action else {
            panic!("wrong action")
        };
        assert!(config.validate().is_err());
        assert!(GpuOcConfig::default().validate().is_err());
        let many = GpuOcConfig {
            nvidia: Some(NvidiaOc {
                clock: vec![0; MAX_OC_CARDS + 1],
                ..Default::default()
            }),
            amd: None,
        };
        assert!(many.validate().is_err());
    }
}
