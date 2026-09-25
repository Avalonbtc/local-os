use reqwest::{Client, Url};
use rig_domain::*;
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};

pub fn registry() -> HashMap<String, Arc<dyn BmcProvider>> {
    HashMap::from([
        ("redfish".into(), Arc::new(Redfish) as Arc<dyn BmcProvider>),
        ("ipmi".into(), Arc::new(Ipmi) as Arc<dyn BmcProvider>),
    ])
}
fn password(c: &Credential) -> Result<&str> {
    match c {
        Credential::Password { password, .. } => Ok(password),
        _ => Err(Error::Validation("BMC 需要用户名和密码".into())),
    }
}
fn failure(e: impl std::fmt::Display) -> Error {
    Error::Unavailable(format!("BMC: {e}"))
}
struct RedfishSession {
    client: Client,
    base: Url,
    username: String,
    password: String,
}
impl RedfishSession {
    fn new(c: &BmcConfig, credential: &Credential) -> Result<Self> {
        static CLIENTS: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Client>>> =
            std::sync::OnceLock::new();
        let clients = CLIENTS.get_or_init(Default::default);
        let key = rig_application::digest(c.ca_pem.as_deref().unwrap_or("system-ca"));
        let mut clients = clients.lock().map_err(failure)?;
        let client = if let Some(client) = clients.get(&key) {
            client.clone()
        } else {
            let mut builder = Client::builder()
                .timeout(Duration::from_secs(15))
                .redirect(reqwest::redirect::Policy::none());
            if let Some(pem) = &c.ca_pem {
                builder = builder.add_root_certificate(
                    reqwest::Certificate::from_pem(pem.as_bytes()).map_err(failure)?,
                );
            }
            let client = builder.build().map_err(failure)?;
            clients.insert(key, client.clone());
            client
        };
        let base = Url::parse(&c.url).map_err(failure)?;
        if base.scheme() != "https" {
            return Err(Error::Validation("Redfish 只允许 HTTPS".into()));
        }
        Ok(Self {
            client,
            base,
            username: c.username.clone(),
            password: password(credential)?.into(),
        })
    }
    fn url(&self, path: &str) -> Result<Url> {
        let url = self.base.join(path).map_err(failure)?;
        if url.origin() != self.base.origin() {
            return Err(Error::Validation("BMC 返回了其他主机的资源链接".into()));
        }
        Ok(url)
    }
    async fn get(&self, path: &str) -> Result<Value> {
        self.client
            .get(self.url(path)?)
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .await
            .map_err(failure)?
            .error_for_status()
            .map_err(failure)?
            .json()
            .await
            .map_err(failure)
    }
    async fn members(&self, path: &str) -> Result<Vec<Value>> {
        let mut results = Vec::new();
        let mut next = Some(path.to_string());
        let mut pages = 0;
        while let Some(path) = next {
            pages += 1;
            if pages > 100 {
                return Err(Error::Validation("BMC 分页超过限制".into()));
            }
            let collection = self.get(&path).await?;
            for m in collection["Members"].as_array().into_iter().flatten() {
                if m.get("Name").is_some()
                    || m.get("Message").is_some()
                    || m.get("Reading").is_some()
                {
                    results.push(m.clone());
                } else if let Some(path) = m["@odata.id"].as_str() {
                    results.push(self.get(path).await?);
                }
            }
            next = collection["Members@odata.nextLink"]
                .as_str()
                .map(str::to_owned);
        }
        Ok(results)
    }
    async fn system(&self) -> Result<Value> {
        let root = self.get("/redfish/v1/").await?;
        let systems = self
            .members(
                root["Systems"]["@odata.id"]
                    .as_str()
                    .ok_or_else(|| Error::Unavailable("BMC 无 ComputerSystem 能力".into()))?,
            )
            .await?;
        if systems.len() != 1 {
            return Err(Error::Validation(
                "当前 Provider 要求 BMC 恰好包含一个 ComputerSystem".into(),
            ));
        }
        Ok(systems[0].clone())
    }
}
pub struct Redfish;
#[async_trait::async_trait]
impl BmcProvider for Redfish {
    fn id(&self) -> &str {
        "redfish"
    }
    async fn read(&self, c: &BmcConfig, credential: &Credential, kind: &str) -> Result<Value> {
        let s = RedfishSession::new(c, credential)?;
        if kind == "power" {
            let system = s.system().await?;
            return Ok(
                json!({"state":system["PowerState"],"health":system["Status"],"model":system["Model"],"manufacturer":system["Manufacturer"]}),
            );
        }
        let root = s.get("/redfish/v1/").await?;
        let mut items = Vec::new();
        let mut unsupported = Vec::new();
        if kind == "sensors" {
            let path = root["Chassis"]["@odata.id"]
                .as_str()
                .ok_or_else(|| Error::Unavailable("BMC 无 Chassis 能力".into()))?;
            for chassis in s.members(path).await? {
                if let Some(path) = chassis["Sensors"]["@odata.id"].as_str() {
                    for sensor in s.members(path).await? {
                        items.push(json!({"name":sensor["Name"],"reading":sensor["Reading"],"unit":sensor["ReadingUnits"],"health":sensor["Status"]["Health"],"raw":sensor}));
                    }
                }
                for (resource, fields) in [
                    (
                        "Thermal",
                        vec![
                            ("Temperatures", "ReadingCelsius", "°C"),
                            ("Fans", "Reading", "RPM"),
                        ],
                    ),
                    (
                        "Power",
                        vec![
                            ("PowerControl", "PowerConsumedWatts", "W"),
                            ("PowerSupplies", "PowerOutputWatts", "W"),
                            ("Voltages", "ReadingVolts", "V"),
                        ],
                    ),
                ] {
                    if let Some(path) = chassis[resource]["@odata.id"].as_str() {
                        let values = s.get(path).await?;
                        for (field, reading, unit) in &fields {
                            for sensor in values[*field].as_array().into_iter().flatten() {
                                let unit = sensor["ReadingUnits"].as_str().unwrap_or(unit);
                                items.push(json!({"name":sensor["Name"],"reading":sensor[*reading],"unit":unit,"health":sensor["Status"]["Health"],"raw":sensor}));
                            }
                        }
                    } else {
                        unsupported.push(resource);
                    }
                }
            }
        } else if kind == "events" {
            let system = s.system().await?;
            if let Some(path) = system["LogServices"]["@odata.id"].as_str() {
                for log in s.members(path).await? {
                    if let Some(path) = log["Entries"]["@odata.id"].as_str() {
                        items.extend(s.members(path).await?);
                    }
                }
            } else {
                unsupported.push("ComputerSystem.LogServices");
            }
        } else {
            return Err(Error::Validation("BMC 查询类型无效".into()));
        }
        Ok(json!({"items":items,"unsupported":unsupported,"provider":"redfish"}))
    }
    async fn power(&self, c: &BmcConfig, credential: &Credential, action: &str) -> Result<Value> {
        let reset = match action {
            "on" => "On",
            "shutdown" => "GracefulShutdown",
            "reboot" => "GracefulRestart",
            "force_off" => "ForceOff",
            "force_restart" => "ForceRestart",
            _ => return Err(Error::Validation("无效电源动作".into())),
        };
        let s = RedfishSession::new(c, credential)?;
        let system = s.system().await?;
        let control = &system["Actions"]["#ComputerSystem.Reset"];
        if let Some(allowed) = control["ResetType@Redfish.AllowableValues"].as_array()
            && !allowed.iter().any(|v| v == reset)
        {
            return Err(Error::Validation(format!(
                "BMC 不支持 {reset}，未降级为强制动作"
            )));
        }
        let target = control["target"]
            .as_str()
            .ok_or_else(|| Error::Validation("BMC 缺少 Reset action".into()))?;
        let response = s
            .client
            .post(s.url(target)?)
            .basic_auth(&s.username, Some(&s.password))
            .json(&json!({"ResetType":reset}))
            .send()
            .await
            .map_err(failure)?;
        let status = response.status();
        if !status.is_success() {
            return Err(failure(format!("Reset HTTP {status}")));
        }
        Ok(
            json!({"request_accepted":true,"action":action,"previous_state":system["PowerState"],"task_monitor":response.headers().get("location").and_then(|h|h.to_str().ok()),"completion":"Check subsequent power/boot telemetry; acknowledgement does not prove completed reboot"}),
        )
    }
}
pub struct Ipmi;

fn dcmi_power_w(output: &str) -> Option<u32> {
    output.lines().find_map(|line| {
        let (label, reading) = line.split_once(':')?;
        if !label
            .trim()
            .eq_ignore_ascii_case("Instantaneous power reading")
        {
            return None;
        }
        let mut parts = reading.split_whitespace();
        let watts = parts.next()?.parse::<u32>().ok()?;
        (watts > 0 && parts.next()?.eq_ignore_ascii_case("Watts")).then_some(watts)
    })
}

struct IpmiTarget {
    host: String,
    port: Option<u16>,
}

fn ipmi_target(address: &str) -> Result<IpmiTarget> {
    let address = address.trim();
    let url = if address.contains("://") {
        Url::parse(address)
    } else {
        Url::parse(&format!("ipmi://{address}"))
    }
    .map_err(|_| Error::Validation("IPMI 地址无效：请填写 BMC IP 或主机名".into()))?;
    if !matches!(url.scheme(), "ipmi" | "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "" && url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::Validation(
            "IPMI 地址无效：只填写 BMC IP/主机名，不要附带路径或账号".into(),
        ));
    }
    let host = url
        .host_str()
        .filter(|host| !host.starts_with('-'))
        .ok_or_else(|| Error::Validation("IPMI 地址无效：缺少 BMC 主机名".into()))?
        .to_owned();
    let port = (url.scheme() == "ipmi").then(|| url.port()).flatten();
    Ok(IpmiTarget { host, port })
}

impl Ipmi {
    async fn run(&self, c: &BmcConfig, credential: &Credential, args: &[&str]) -> Result<String> {
        self.run_with_timeout(c, credential, args, Duration::from_secs(30))
            .await
    }

    async fn run_with_timeout(
        &self,
        c: &BmcConfig,
        credential: &Credential,
        args: &[&str],
        timeout: Duration,
    ) -> Result<String> {
        let target = ipmi_target(&c.url)?;
        let mut command = tokio::process::Command::new("ipmitool");
        command.args(["-I", "lanplus", "-H", &target.host, "-U", &c.username, "-E"]);
        if let Some(port) = target.port {
            command.args(["-p", &port.to_string()]);
        }
        command
            .args(args)
            .env("IPMI_PASSWORD", password(credential)?)
            .kill_on_drop(true);
        let result = tokio::time::timeout(timeout, command.output())
            .await
            .map_err(|_| failure("IPMI 超时，电源结果需对账"))?
            .map_err(failure)?;
        if !result.status.success() {
            return Err(failure(String::from_utf8_lossy(&result.stderr)));
        }
        Ok(String::from_utf8_lossy(&result.stdout).into_owned())
    }
}
#[async_trait::async_trait]
impl BmcProvider for Ipmi {
    fn id(&self) -> &str {
        "ipmi"
    }
    async fn read(&self, c: &BmcConfig, credential: &Credential, kind: &str) -> Result<Value> {
        let args = match kind {
            "power" => vec!["chassis", "power", "status"],
            "sensors" => vec!["sensor", "list"],
            "events" => vec!["sel", "elist"],
            _ => return Err(Error::Validation("无效 IPMI 查询".into())),
        };
        let raw = self.run(c, credential, &args).await?;
        if kind == "power" {
            let reading = self
                .run_with_timeout(
                    c,
                    credential,
                    &["dcmi", "power", "reading"],
                    Duration::from_secs(8),
                )
                .await;
            let (watts, power_error, power_raw) = match reading {
                Ok(output) => {
                    let watts = dcmi_power_w(&output);
                    (
                        watts,
                        watts
                            .is_none()
                            .then_some("DCMI 未返回有效功耗读数".to_string()),
                        watts.is_none().then_some(output),
                    )
                }
                Err(error) => (None, Some(error.to_string()), None),
            };
            return Ok(
                json!({"state":if raw.to_lowercase().contains("is on"){"On"}else if raw.to_lowercase().contains("is off"){"Off"}else{"Unknown"},"power_w":watts,"power_source":watts.map(|_| "IPMI DCMI"),"power_error":power_error,"power_raw":power_raw,"raw":raw}),
            );
        }
        let items:Vec<Value>=raw.lines().map(|line|{let fields:Vec<_>=line.split('|').map(str::trim).collect();json!({"name":fields.first(),"reading":fields.get(1),"unit":fields.get(2),"health":fields.get(3),"raw":line})}).collect();
        Ok(json!({"items":items,"provider":"ipmi"}))
    }
    async fn power(&self, c: &BmcConfig, credential: &Credential, action: &str) -> Result<Value> {
        let verb = match action {
            "on" => "on",
            "shutdown" => "soft",
            "force_off" => "off",
            "force_restart" => "reset",
            "reboot" => {
                return Err(Error::Validation(
                    "IPMI 不提供可靠的优雅重启；请使用系统重启命令或显式强制重启".into(),
                ));
            }
            _ => return Err(Error::Validation("无效电源动作".into())),
        };
        let raw = self.run(c, credential, &["chassis", "power", verb]).await?;
        Ok(json!({"request_accepted":true,"raw":raw,"action":action}))
    }
}

#[cfg(test)]
mod ipmi_address_tests {
    use super::{dcmi_power_w, ipmi_target};

    #[test]
    fn accepts_bmc_web_address_as_host() {
        let target = ipmi_target("https://10.168.2.201/").unwrap();
        assert_eq!(target.host, "10.168.2.201");
        assert_eq!(target.port, None);
    }

    #[test]
    fn accepts_bare_host_and_explicit_ipmi_port() {
        assert_eq!(ipmi_target("10.168.2.201").unwrap().host, "10.168.2.201");
        let target = ipmi_target("ipmi://bmc.example.test:6623").unwrap();
        assert_eq!(target.host, "bmc.example.test");
        assert_eq!(target.port, Some(6623));
    }

    #[test]
    fn rejects_paths_credentials_and_option_like_hosts() {
        for address in [
            "https://10.168.2.201/redfish/v1",
            "ipmi://user:pass@10.168.2.201",
            "-x",
            "",
        ] {
            assert!(ipmi_target(address).is_err(), "{address}");
        }
    }

    #[test]
    fn parses_only_measured_dcmi_instantaneous_power() {
        assert_eq!(
            dcmi_power_w(
                "Instantaneous power reading: 407 Watts\nAverage power reading over sample period: 1256 Watts"
            ),
            Some(407)
        );
        assert_eq!(dcmi_power_w("Instantaneous power reading: 0 Watts"), None);
        assert_eq!(
            dcmi_power_w("Average power reading over sample period: 1256 Watts"),
            None
        );
    }
}
