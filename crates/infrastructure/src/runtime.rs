use base64::{Engine, engine::general_purpose::STANDARD};
use rig_domain::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

const DEPENDENCIES: &str = include_str!("../../../runtime/dependencies.sh");
pub const RUNTIME: &str = include_str!("../../../runtime/rig-runtime.py");
pub struct ScreenRuntime {
    pub remote: Arc<dyn RemoteExecutor>,
    pub cache: PathBuf,
    client: reqwest::Client,
    sudo_modes: tokio::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, bool)>>,
}
impl ScreenRuntime {
    pub fn new(remote: Arc<dyn RemoteExecutor>, cache: PathBuf) -> Result<Self> {
        Ok(Self {
            remote,
            cache,
            sudo_modes: Default::default(),
            client: reqwest::Client::builder()
                .https_only(true)
                .redirect(reqwest::redirect::Policy::limited(5))
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .map_err(|e| Error::Internal(e.to_string()))?,
        })
    }
    pub async fn store_package(&self, bytes: &[u8]) -> Result<Value> {
        let (tx, rx) = tokio::sync::mpsc::channel(2);
        let producer = async {
            for part in bytes.chunks(65536) {
                tx.send(Ok(part.to_vec()))
                    .await
                    .map_err(|_| Error::Unavailable("安装包接收已停止".into()))?;
            }
            drop(tx);
            Ok::<_, Error>(())
        };
        let (_, value) = tokio::try_join!(producer, self.cache_stream(rx))?;
        Ok(value)
    }
    async fn call(&self, m: &Machine, c: &Credential, args: &str, timeout: u64) -> Result<Value> {
        let output = self
            .run_root(m, c, &format!("/usr/local/bin/rig-miner {args}"), timeout)
            .await?;
        if output.exit_code != Some(0) {
            return Err(Error::Unavailable(format!(
                "运行层调用失败: {} {}",
                output.stderr, output.stdout
            )));
        }
        serde_json::from_str(&output.stdout)
            .map_err(|e| Error::Unavailable(format!("运行层响应无效: {e}")))
    }
    async fn run_root(
        &self,
        m: &Machine,
        c: &Credential,
        command: &str,
        timeout: u64,
    ) -> Result<ExecOutput> {
        if m.username == "root" {
            return self.remote.execute(m, c, command, timeout).await;
        }
        let key = rig_application::digest(&format!(
            "{}:{}:{}:{}:{}",
            m.id,
            m.host,
            m.username,
            m.host_key,
            serde_json::to_string(c).map_err(|e| Error::Internal(e.to_string()))?
        ));
        let cached = self
            .sudo_modes
            .lock()
            .await
            .get(&key)
            .filter(|(at, _)| at.elapsed().as_secs() < 300)
            .map(|(_, v)| *v);
        let no_password = if let Some(value) = cached {
            value
        } else {
            let value = self
                .remote
                .execute(m, c, "sudo -n true", 15)
                .await?
                .exit_code
                == Some(0);
            let mut modes = self.sudo_modes.lock().await;
            modes.retain(|_, (at, _)| at.elapsed().as_secs() < 300);
            modes.insert(key, (std::time::Instant::now(), value));
            value
        };
        if no_password {
            return self
                .remote
                .execute(m, c, &format!("sudo -n {command}"), timeout)
                .await;
        }
        let password = c
            .sudo_password()
            .filter(|password| !password.is_empty())
            .ok_or_else(|| {
                Error::Unavailable(
                    "sudo 需要密码：请在机器设置中填写 sudo 密码，或配置免密码 sudo".into(),
                )
            })?;
        let input = format!("{password}\n").into_bytes();
        let output = self
            .remote
            .execute_input(m, c, &format!("sudo -S -p '' {command}"), &input, timeout)
            .await?;
        if output.exit_code != Some(0) && output.stderr.contains("password") {
            return Err(Error::Unavailable(
                "sudo 认证失败：请在机器设置中填写正确的 sudo 密码，或给该 SSH 用户配置免密码 sudo"
                    .into(),
            ));
        }
        Ok(output)
    }
}
fn io(e: std::io::Error) -> Error {
    Error::Internal(format!("软件包存储: {e}"))
}
fn net(e: reqwest::Error) -> Error {
    Error::Unavailable(format!("下载软件包: {e}"))
}
impl ScreenRuntime {
    async fn send_operation(
        &self,
        m: &Machine,
        c: &Credential,
        id: Uuid,
        action: &Value,
    ) -> Result<()> {
        let payload = STANDARD
            .encode(serde_json::to_vec(action).map_err(|e| Error::Internal(e.to_string()))?);
        self.call(
            m,
            c,
            &format!("operation-start {id} {}", shell_quote(&payload)),
            30,
        )
        .await?;
        Ok(())
    }
    /// Like HiveOS, the rig downloads its own miner from the flight sheet's URL. With a pinned
    /// SHA256 the rig verifies it; without one it reports the digest it stored. Returns that
    /// digest. The controller never stores or forwards the package.
    async fn rig_fetch(&self, m: &Machine, c: &Credential, cfg: &Value) -> Result<String> {
        let pinned = cfg["sha256"]
            .as_str()
            .map(str::to_lowercase)
            .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()));
        let url = cfg["url"].as_str().unwrap_or("");
        if let Some(hash) = &pinned {
            let remote = format!("/var/lib/rigdeck/cache/{hash}.tar");
            let check = self
                .run_root(m, c, &format!("sha256sum {}", shell_quote(&remote)), 20)
                .await?;
            if check.stdout.starts_with(hash.as_str()) {
                return Ok(hash.clone());
            }
        }
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(Error::Validation(
                "这台矿机没有该安装包，而飞行表里的地址不能直接下载；请在飞行表中填写 http(s) 安装链接".into(),
            ));
        }
        let command = format!(
            "/usr/local/bin/rig-miner fetch-package {} {}",
            shell_quote(pinned.as_deref().unwrap_or("-")),
            shell_quote(url)
        );
        let output = self.run_root(m, c, &command, 1800).await?;
        let digest = output.stdout.trim().to_lowercase();
        if output.exit_code != Some(0)
            || digest.len() != 64
            || !digest.bytes().all(|b| b.is_ascii_hexdigit())
            || pinned.as_ref().is_some_and(|h| h != &digest)
        {
            let reason: String = output.stderr.trim().chars().take(600).collect();
            let hint = if reason.contains("Invalid SHA256")
                || reason.contains("fetch-package")
                || reason.is_empty()
            {
                "；如果运行层低于 0.2.0，请先「部署运行层」"
            } else {
                ""
            };
            return Err(Error::Unavailable(format!(
                "矿机下载安装包失败：{}{hint}",
                if reason.is_empty() {
                    "无输出"
                } else {
                    &reason
                }
            )));
        }
        Ok(digest)
    }
}
#[async_trait::async_trait]
impl MinerRuntime for ScreenRuntime {
    async fn import_package(&self, url: &str) -> Result<Value> {
        let mut response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(net)?
            .error_for_status()
            .map_err(net)?;
        if response
            .content_length()
            .is_some_and(|size| size > 512 * 1024 * 1024)
        {
            return Err(Error::Validation("安装包超过 512 MiB".into()));
        }
        let (tx, rx) = tokio::sync::mpsc::channel(2);
        let download = async move {
            while let Some(chunk) = response.chunk().await.map_err(net)? {
                for part in chunk.chunks(65536) {
                    tx.send(Ok(part.to_vec()))
                        .await
                        .map_err(|_| Error::Unavailable("下载接收已停止".into()))?;
                }
            }
            Ok::<_, Error>(())
        };
        let (_, value) = tokio::try_join!(download, self.cache_stream(rx))?;
        Ok(value)
    }
    async fn cache_stream(
        &self,
        mut chunks: tokio::sync::mpsc::Receiver<Result<Vec<u8>>>,
    ) -> Result<Value> {
        tokio::fs::create_dir_all(&self.cache).await.map_err(io)?;
        let temp = TemporaryPackage(self.cache.join(format!("{}.upload", Uuid::new_v4())));
        let mut file = tokio::fs::File::create(&temp.0).await.map_err(io)?;
        let mut size = 0usize;
        while let Some(chunk) = chunks.recv().await {
            let chunk = chunk?;
            size += chunk.len();
            if size > 512 * 1024 * 1024 {
                return Err(Error::Validation("安装包超过 512 MiB".into()));
            }
            file.write_all(&chunk).await.map_err(io)?;
        }
        if size == 0 {
            return Err(Error::Validation("安装包为空".into()));
        }
        file.sync_all().await.map_err(io)?;
        drop(file);
        let hash = hash_file(temp.0.clone()).await?;
        let target = self.cache.join(format!("{hash}.tar"));
        if !target.exists() {
            tokio::fs::rename(&temp.0, &target).await.map_err(io)?;
        }
        Ok(json!({"sha256":hash,"url":format!("cache:{hash}"),"size_bytes":size}))
    }

    async fn cache_package(&self, bytes: &[u8]) -> Result<Value> {
        self.store_package(bytes).await
    }
    async fn bootstrap(&self, m: &Machine, c: &Credential) -> Result<Value> {
        let hash = hex::encode(Sha256::digest(RUNTIME.as_bytes()));
        let dir = format!("/opt/rigdeck/runtime/{}", &hash[..16]);
        let temp = format!("/tmp/rigdeck-{}.py", Uuid::new_v4());
        self.remote
            .upload(m, c, &temp, RUNTIME.as_bytes(), 0o600)
            .await?;
        let script = format!(
            r#"set -eu
test "$(uname -s)" = Linux
{dependencies}
install -d -m 700 /var/lib/rigdeck /var/lib/rigdeck/cache
install -d -m 755 {dir} /hive /var/log/miner
install -m 755 {temp} {dir}/rig-runtime.py
ln -sfn {dir} /opt/rigdeck/current.next
mv -Tf /opt/rigdeck/current.next /opt/rigdeck/current
printf '%s\n' '#!/bin/sh' 'exec python3 /opt/rigdeck/current/rig-runtime.py "$@"' > /usr/local/bin/rig-miner
chmod 755 /usr/local/bin/rig-miner
rm -f {temp}
printf '%s\n' '[Unit]' 'Description=RigDeck restore desired mining state' 'After=network-online.target' 'Wants=network-online.target' '[Service]' 'Type=oneshot' 'ExecStart=/usr/local/bin/rig-miner recover' 'RemainAfterExit=yes' '[Install]' 'WantedBy=multi-user.target' > /etc/systemd/system/rigdeck-restore.service
printf '%s\n' '[Unit]' 'Description=RigDeck local statistics and recovery watchdog' 'After=rigdeck-restore.service' '[Service]' 'Type=simple' 'ExecStart=/usr/local/bin/rig-miner watchdog' 'Restart=always' 'RestartSec=5' 'KillMode=process' '[Install]' 'WantedBy=multi-user.target' > /etc/systemd/system/rigdeck-watchdog.service
systemctl daemon-reload
systemctl enable rigdeck-restore.service rigdeck-watchdog.service >/dev/null
/usr/local/bin/rig-miner set-reader {reader} >/dev/null
systemctl restart rigdeck-watchdog.service
/usr/local/bin/rig-miner check
"#,
            dependencies = DEPENDENCIES,
            reader = shell_quote(&m.username)
        );
        let script_path = format!("/tmp/rigdeck-install-{}.sh", Uuid::new_v4());
        self.remote
            .upload(m, c, &script_path, script.as_bytes(), 0o600)
            .await?;
        let output = self
            .run_root(m, c, &format!("sh {}", shell_quote(&script_path)), 300)
            .await?;
        let _ = self
            .remote
            .execute(m, c, &format!("rm -f {}", shell_quote(&script_path)), 10)
            .await;
        if output.exit_code != Some(0) {
            return Err(Error::Unavailable(format!(
                "自动部署失败: {} {}",
                output.stderr, output.stdout
            )));
        }
        // apt output may precede the final JSON line.
        let line = output
            .stdout
            .lines()
            .rev()
            .find(|l| l.starts_with('{'))
            .ok_or_else(|| Error::Unavailable("运行层未返回安装结果".into()))?;
        serde_json::from_str(line).map_err(|e| Error::Internal(e.to_string()))
    }
    async fn start_operation(
        &self,
        m: &Machine,
        c: &Credential,
        id: Uuid,
        action: &Value,
    ) -> Result<()> {
        if action["kind"] == "apply" {
            let dependencies = self
                .run_root(m, c, &format!("sh -c {}", shell_quote(DEPENDENCIES)), 250)
                .await?;
            if dependencies.exit_code != Some(0) {
                return Err(Error::Unavailable(format!(
                    "运行依赖安装失败（软件源刷新限时 90 秒，安装限时 120 秒）：{} {}",
                    dependencies.stderr, dependencies.stdout
                )));
            }
            let mut action = action.clone();
            for cfg in action["deployment"]["instances"]
                .as_array_mut()
                .ok_or_else(|| Error::Validation("缺少部署实例".into()))?
            {
                // Nothing has been started on the rig yet, so a failed download is a definite
                // failure (job "failed", lock released), never "unknown".
                let digest = self.rig_fetch(m, c, cfg).await.map_err(|e| match e {
                    Error::Unavailable(message) => Error::Validation(message),
                    other => other,
                })?;
                // The rig's package is addressed by the digest it actually downloaded.
                cfg["sha256"] = json!(digest);
            }
            return self.send_operation(m, c, id, &action).await;
        }
        self.send_operation(m, c, id, action).await
    }
    async fn operation(&self, m: &Machine, c: &Credential, id: Uuid) -> Result<RemoteOperation> {
        serde_json::from_value(
            self.call(m, c, &format!("operation-status {id}"), 20)
                .await?,
        )
        .map_err(|e| Error::Unavailable(e.to_string()))
    }
    async fn cancel(&self, m: &Machine, c: &Credential, id: Uuid) -> Result<()> {
        self.call(m, c, &format!("cancel {id}"), 20).await?;
        Ok(())
    }
    async fn collect(&self, m: &Machine, c: &Credential, kind: &str) -> Result<Value> {
        self.call(m, c, &format!("collect {}", shell_quote(kind)), 20)
            .await
    }
    async fn snapshot(&self, m: &Machine, c: &Credential) -> Result<Option<Value>> {
        // Unprivileged read of a file the watchdog owns; no sudo prompt, no auth.log line,
        // no Python start-up on the rig. The uptime line lets the controller age the sample
        // without trusting the rig's wall clock.
        let output = self
            .remote
            .execute(
                m,
                c,
                "cat /proc/uptime /run/rigdeck/snapshot.json 2>/dev/null",
                15,
            )
            .await?;
        if output.exit_code != Some(0) {
            return Ok(None);
        }
        Ok(parse_snapshot(&output.stdout))
    }
    async fn log_tail(
        &self,
        m: &Machine,
        c: &Credential,
        instance: &str,
        lines: u32,
    ) -> Result<Value> {
        self.call(
            m,
            c,
            &format!(
                "log-tail {} {}",
                shell_quote(instance),
                lines.clamp(1, 2000)
            ),
            20,
        )
        .await
    }
    async fn set_policy(
        &self,
        m: &Machine,
        c: &Credential,
        digest: &str,
        policy: &Value,
    ) -> Result<()> {
        let payload = STANDARD.encode(
            serde_json::to_vec(&json!({"digest":digest,"policy":policy}))
                .map_err(|e| Error::Internal(e.to_string()))?,
        );
        self.call(m, c, &format!("set-policy {}", shell_quote(&payload)), 20)
            .await?;
        Ok(())
    }
}
/// `cat /proc/uptime snapshot.json` → `{now_uptime, snapshot}`; `None` for anything unexpected.
pub fn parse_snapshot(stdout: &str) -> Option<Value> {
    let (first, rest) = stdout.split_once('\n')?;
    let now_uptime: f64 = first.split_whitespace().next()?.parse().ok()?;
    let snapshot: Value = serde_json::from_str(rest.trim()).ok()?;
    if snapshot["schema"] != 1 || !snapshot["uptime"].is_number() {
        return None;
    }
    Some(json!({"now_uptime":now_uptime,"snapshot":snapshot}))
}
pub fn cache_path(root: &Path) -> PathBuf {
    root.join("packages")
}

struct TemporaryPackage(PathBuf);
impl Drop for TemporaryPackage {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
async fn hash_file(path: PathBuf) -> Result<String> {
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let mut file = std::fs::File::open(path).map_err(io)?;
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 65536];
        loop {
            let n = file.read(&mut buffer).map_err(io)?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
        }
        Ok(hex::encode(hash.finalize()))
    })
    .await
    .map_err(|e| Error::Internal(e.to_string()))?
}

#[cfg(test)]
mod package_tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn url_import_calculates_hash_and_stores_bytes() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/fixture.tar.gz", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 2048];
            assert!(stream.read(&mut request).await.unwrap() > 0);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nfixture",
                )
                .await
                .unwrap();
        });
        let cache = std::env::temp_dir().join(format!("rigdeck-import-{}", Uuid::new_v4()));
        let runtime = ScreenRuntime {
            remote: Arc::new(crate::ssh::SshExecutor::default()),
            cache: cache.clone(),
            sudo_modes: Default::default(),
            // Plain HTTP is confined to this loopback fixture. Production is HTTPS-only.
            client: reqwest::Client::new(),
        };
        let artifact = runtime.import_package(&url).await.unwrap();
        server.await.unwrap();
        assert_eq!(artifact["sha256"], hex::encode(Sha256::digest(b"fixture")));
        assert_eq!(artifact["size_bytes"], 7);
        let path = cache.join(format!("{}.tar", artifact["sha256"].as_str().unwrap()));
        assert_eq!(tokio::fs::read(&path).await.unwrap(), b"fixture");
        tokio::fs::remove_dir_all(cache).await.unwrap();
    }
}

#[cfg(test)]
mod snapshot_parse_tests {
    use super::parse_snapshot;

    #[test]
    fn parses_uptime_line_and_snapshot_or_rejects() {
        let parsed =
            parse_snapshot("123.45 999.0\n{\"schema\":1,\"uptime\":120.5,\"system\":{}}\n")
                .unwrap();
        assert_eq!(parsed["now_uptime"], 123.45);
        assert_eq!(parsed["snapshot"]["uptime"], 120.5);
        assert!(parse_snapshot("123.45 999.0\n").is_none());
        assert!(parse_snapshot("123.45 999.0\n{\"schema\":2,\"uptime\":1}").is_none());
        assert!(parse_snapshot("garbage\n{}").is_none());
    }
}

#[cfg(test)]
mod rig_fetch_tests {
    use super::*;
    use std::sync::Mutex;

    /// A rig that answers `sha256sum` and `fetch-package` like the real runtime would.
    struct Rig {
        commands: Mutex<Vec<String>>,
        cached: Option<String>,
        fetched: std::result::Result<String, String>,
    }
    #[async_trait::async_trait]
    impl RemoteExecutor for Rig {
        async fn execute(
            &self,
            _: &Machine,
            _: &Credential,
            command: &str,
            _: u64,
        ) -> Result<ExecOutput> {
            self.commands.lock().unwrap().push(command.to_string());
            let (stdout, stderr, code) = if command.contains("sha256sum") {
                match &self.cached {
                    Some(h) if command.contains(h.as_str()) => {
                        (format!("{h}  file\n"), String::new(), 0)
                    }
                    _ => (String::new(), "No such file".into(), 1),
                }
            } else if command.contains("fetch-package") {
                match &self.fetched {
                    Ok(h) => (format!("{h}\n"), String::new(), 0),
                    Err(e) => (String::new(), e.clone(), 22),
                }
            } else {
                (String::new(), String::new(), 0)
            };
            Ok(ExecOutput {
                stdout,
                stderr,
                exit_code: Some(code),
                truncated: false,
            })
        }
        async fn upload(
            &self,
            _: &Machine,
            _: &Credential,
            _: &str,
            _: &[u8],
            _: u32,
        ) -> Result<()> {
            panic!("the controller must never upload packages")
        }
        async fn terminal(
            &self,
            _: &Machine,
            _: &Credential,
            _: Option<&str>,
            _: tokio::sync::mpsc::Receiver<TerminalInput>,
            _: tokio::sync::mpsc::Sender<TerminalOutput>,
        ) -> Result<()> {
            unreachable!()
        }
    }
    fn runtime(rig: Arc<Rig>) -> ScreenRuntime {
        ScreenRuntime::new(rig, std::env::temp_dir().join("rigdeck-unused")).unwrap()
    }
    fn machine() -> Machine {
        serde_json::from_value(json!({"id":Uuid::nil(),"name":"r","host":"h","port":22,"username":"root","host_key":"k","group":"","tags":[],"is_controller":false,"bmc":null,"policy":{},"created_at":"2026-01-01T00:00:00Z"})).unwrap()
    }
    const CRED: Credential = Credential::Password {
        password: String::new(),
        sudo_password: None,
    };

    #[tokio::test]
    async fn unpinned_url_is_downloaded_by_the_rig_and_its_digest_used() {
        let digest = "c".repeat(64);
        let rig = Arc::new(Rig {
            commands: Mutex::default(),
            cached: None,
            fetched: Ok(digest.clone()),
        });
        let cfg = json!({"url":"https://github.com/xmrig/x.tar.gz"});
        assert_eq!(
            runtime(rig.clone())
                .rig_fetch(&machine(), &CRED, &cfg)
                .await
                .unwrap(),
            digest
        );
        let commands = rig.commands.lock().unwrap().clone();
        assert_eq!(commands.len(), 1);
        assert!(commands[0].contains("fetch-package '-' 'https://github.com/xmrig/x.tar.gz'"));
    }

    #[tokio::test]
    async fn pinned_digest_already_on_rig_skips_download_and_mismatch_fails() {
        let digest = "a".repeat(64);
        let rig = Arc::new(Rig {
            commands: Mutex::default(),
            cached: Some(digest.clone()),
            fetched: Err("unused".into()),
        });
        let cfg = json!({"url":"cache:legacy","sha256":digest});
        assert_eq!(
            runtime(rig.clone())
                .rig_fetch(&machine(), &CRED, &cfg)
                .await
                .unwrap(),
            digest
        );
        assert_eq!(rig.commands.lock().unwrap().len(), 1);

        let rig = Arc::new(Rig {
            commands: Mutex::default(),
            cached: None,
            fetched: Ok("b".repeat(64)),
        });
        let cfg = json!({"url":"https://x/p.tar.gz","sha256":"a".repeat(64)});
        assert!(
            runtime(rig)
                .rig_fetch(&machine(), &CRED, &cfg)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn failed_download_fails_the_apply_instead_of_leaving_it_unknown() {
        let rig = Arc::new(Rig {
            commands: Mutex::default(),
            cached: None,
            fetched: Err("curl: (22) 404".into()),
        });
        let action = json!({"kind":"apply","deployment":{"instances":[{"url":"https://github.com/x.tar.gz"}]}});
        let error = runtime(rig.clone())
            .start_operation(&machine(), &CRED, Uuid::new_v4(), &action)
            .await
            .unwrap_err();
        // Validation maps to "failed" and releases the machine lock; nothing ran on the rig.
        assert!(matches!(error, Error::Validation(_)), "{error:?}");
        assert!(
            !rig.commands
                .lock()
                .unwrap()
                .iter()
                .any(|c| c.contains("operation-start"))
        );
    }

    #[tokio::test]
    async fn legacy_cache_package_missing_on_rig_and_download_errors_are_reported() {
        let rig = Arc::new(Rig {
            commands: Mutex::default(),
            cached: None,
            fetched: Ok("a".repeat(64)),
        });
        let cfg = json!({"url":"cache:abc","sha256":"a".repeat(64)});
        let error = runtime(rig)
            .rig_fetch(&machine(), &CRED, &cfg)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("安装链接"));

        let rig = Arc::new(Rig {
            commands: Mutex::default(),
            cached: None,
            fetched: Err("curl: (6) Could not resolve host: github.com".into()),
        });
        let error = runtime(rig)
            .rig_fetch(&machine(), &CRED, &json!({"url":"https://github.com/x"}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Could not resolve host"));
    }
}
