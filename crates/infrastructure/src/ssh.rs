use rig_domain::*;
use russh::{
    ChannelMsg, client,
    keys::{PrivateKeyWithHashAlg, decode_secret_key, ssh_key},
};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{Mutex, mpsc};

pub struct KeyVerifier {
    expected: String,
}
struct KeyProbe {
    observed: Arc<Mutex<Option<SshHostKey>>>,
}
impl client::Handler for KeyProbe {
    type Error = russh::Error;
    async fn check_server_key(
        &mut self,
        key: &ssh_key::PublicKey,
    ) -> std::result::Result<bool, Self::Error> {
        *self.observed.lock().await = Some(SshHostKey {
            fingerprint: key.fingerprint(ssh_key::HashAlg::Sha256).to_string(),
            algorithm: key.algorithm().to_string(),
        });
        // Discovery ends at host-key exchange; it never authenticates or trusts the key.
        Ok(false)
    }
}
impl client::Handler for KeyVerifier {
    type Error = russh::Error;
    async fn check_server_key(
        &mut self,
        key: &ssh_key::PublicKey,
    ) -> std::result::Result<bool, Self::Error> {
        Ok(key.fingerprint(ssh_key::HashAlg::Sha256).to_string() == self.expected)
    }
}
#[derive(Default)]
pub struct SshExecutor {
    pool: Mutex<HashMap<String, Arc<client::Handle<KeyVerifier>>>>,
}
fn remote_error(error: impl std::fmt::Display) -> Error {
    Error::Unavailable(format!("SSH: {error}"))
}
impl SshExecutor {
    async fn connect(
        &self,
        m: &Machine,
        c: &Credential,
    ) -> Result<Arc<client::Handle<KeyVerifier>>> {
        let credential_hash =
            rig_application::digest(&serde_json::to_string(c).map_err(remote_error)?);
        let key = format!(
            "{}:{}:{}:{}:{}:{}",
            m.id, m.host, m.port, m.username, m.host_key, credential_hash
        );
        if let Some(handle) = self.pool.lock().await.get(&key).cloned()
            && !handle.is_closed()
        {
            return Ok(handle);
        }
        let config = Arc::new(client::Config {
            inactivity_timeout: Some(Duration::from_secs(600)),
            keepalive_interval: Some(Duration::from_secs(30)),
            keepalive_max: 3,
            ..Default::default()
        });
        let mut handle = tokio::time::timeout(
            Duration::from_secs(15),
            client::connect(
                config,
                (m.host.as_str(), m.port),
                KeyVerifier {
                    expected: m.host_key.clone(),
                },
            ),
        )
        .await
        .map_err(|_| Error::Unavailable("SSH 连接超时".into()))?
        .map_err(remote_error)?;
        let auth = tokio::time::timeout(Duration::from_secs(15), async {
            Ok::<_, Error>(match c {
                Credential::Password { password, .. } => handle
                    .authenticate_password(&m.username, password)
                    .await
                    .map_err(remote_error)?,
                Credential::PrivateKey {
                    private_key,
                    passphrase,
                    ..
                } => {
                    let key = decode_secret_key(private_key, passphrase.as_deref())
                        .map_err(remote_error)?;
                    let hash = handle
                        .best_supported_rsa_hash()
                        .await
                        .map_err(remote_error)?
                        .flatten();
                    handle
                        .authenticate_publickey(
                            &m.username,
                            PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                        )
                        .await
                        .map_err(remote_error)?
                }
            })
        })
        .await
        .map_err(|_| Error::Unavailable("SSH 认证超时".into()))??;
        if !auth.success() {
            return Err(Error::Unavailable("SSH 认证失败".into()));
        }
        let handle = Arc::new(handle);
        let mut pool = self.pool.lock().await;
        pool.retain(|_, v| !v.is_closed());
        // Retain live connections regardless of fleet size; closed handles are removed above.
        pool.insert(key, handle.clone());
        Ok(handle)
    }
}
#[async_trait::async_trait]
impl RemoteExecutor for SshExecutor {
    async fn probe_host_key(&self, host: &str, port: u16) -> Result<SshHostKey> {
        let observed = Arc::new(Mutex::new(None));
        let result = tokio::time::timeout(
            Duration::from_secs(15),
            client::connect(
                Arc::new(client::Config::default()),
                (host, port),
                KeyProbe {
                    observed: observed.clone(),
                },
            ),
        )
        .await;
        let key = observed.lock().await.take();
        if let Some(key) = key {
            return Ok(key);
        }
        match result {
            Err(_) => Err(Error::Unavailable(
                "SSH 指纹获取超时，请检查地址、端口和主控网络".into(),
            )),
            Ok(Err(error)) => Err(remote_error(error)),
            Ok(Ok(_)) => Err(Error::Unavailable("SSH 服务未提供主机指纹".into())),
        }
    }
    async fn execute(
        &self,
        m: &Machine,
        c: &Credential,
        command: &str,
        timeout: u64,
    ) -> Result<ExecOutput> {
        self.execute_input(m, c, command, &[], timeout).await
    }
    async fn execute_input(
        &self,
        m: &Machine,
        c: &Credential,
        command: &str,
        input: &[u8],
        timeout: u64,
    ) -> Result<ExecOutput> {
        let handle = self.connect(m, c).await?;
        let mut channel =
            tokio::time::timeout(Duration::from_secs(15), handle.channel_open_session())
                .await
                .map_err(|_| Error::Unavailable("SSH 通道超时".into()))?
                .map_err(remote_error)?;
        channel.exec(true, command).await.map_err(remote_error)?;
        for block in input.chunks(65536) {
            channel.data(block).await.map_err(remote_error)?;
        }
        channel.eof().await.map_err(remote_error)?;
        let result = tokio::time::timeout(Duration::from_secs(timeout), async {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let mut code = None;
            let mut truncated = false;
            while let Some(message) = channel.wait().await {
                match message {
                    ChannelMsg::Data { data } => {
                        let left = (2 * 1024 * 1024usize).saturating_sub(stdout.len());
                        truncated |= data.len() > left;
                        stdout.extend_from_slice(&data[..data.len().min(left)]);
                    }
                    ChannelMsg::ExtendedData { data, .. } => {
                        let left = (64 * 1024usize).saturating_sub(stderr.len());
                        truncated |= data.len() > left;
                        stderr.extend_from_slice(&data[..data.len().min(left)]);
                    }
                    ChannelMsg::ExitStatus { exit_status } => code = Some(exit_status),
                    _ => {}
                }
            }
            ExecOutput {
                stdout: String::from_utf8_lossy(&stdout).into_owned(),
                stderr: String::from_utf8_lossy(&stderr).into_owned(),
                exit_code: code,
                truncated,
            }
        })
        .await;
        if result.is_err() {
            let _ = channel.close().await;
        }
        result.map_err(|_| Error::Unavailable("SSH 操作超时；远端是否完成需对账".into()))
    }
    async fn upload(
        &self,
        m: &Machine,
        c: &Credential,
        path: &str,
        content: &[u8],
        mode: u32,
    ) -> Result<()> {
        let handle = self.connect(m, c).await?;
        let mut channel =
            tokio::time::timeout(Duration::from_secs(15), handle.channel_open_session())
                .await
                .map_err(|_| Error::Unavailable("SSH 通道超时".into()))?
                .map_err(remote_error)?;
        let script = "import os,sys,tempfile; p=sys.argv[1]; f=tempfile.NamedTemporaryFile(dir=os.path.dirname(p),delete=False); f.write(sys.stdin.buffer.read()); f.flush(); os.fsync(f.fileno()); f.close(); os.chmod(f.name,int(sys.argv[2])); os.replace(f.name,p)";
        channel
            .exec(
                true,
                format!(
                    "python3 -c {} {} {}",
                    shell_quote(script),
                    shell_quote(path),
                    mode
                ),
            )
            .await
            .map_err(remote_error)?;
        for block in content.chunks(65536) {
            channel.data(block).await.map_err(remote_error)?;
        }
        channel.eof().await.map_err(remote_error)?;
        let code = tokio::time::timeout(Duration::from_secs(120), async {
            let mut code = None;
            while let Some(message) = channel.wait().await {
                if let ChannelMsg::ExitStatus { exit_status } = message {
                    code = Some(exit_status);
                }
            }
            code
        })
        .await
        .map_err(|_| Error::Unavailable("SSH 上传确认超时".into()))?;
        if code != Some(0) {
            return Err(Error::Unavailable("SSH 上传失败".into()));
        }
        Ok(())
    }
    async fn terminal(
        &self,
        m: &Machine,
        c: &Credential,
        command: Option<&str>,
        input: mpsc::Receiver<TerminalInput>,
        output: mpsc::Sender<TerminalOutput>,
    ) -> Result<()> {
        self.terminal_impl(m, c, command, false, input, output)
            .await
    }
    async fn terminal_root(
        &self,
        m: &Machine,
        c: &Credential,
        command: &str,
        input: mpsc::Receiver<TerminalInput>,
        output: mpsc::Sender<TerminalOutput>,
    ) -> Result<()> {
        self.terminal_impl(m, c, Some(command), true, input, output)
            .await
    }
    async fn upload_file(
        &self,
        m: &Machine,
        c: &Credential,
        path: &str,
        local: &std::path::Path,
        mode: u32,
    ) -> Result<()> {
        use tokio::io::AsyncReadExt;
        // Large packages over slow links take as long as they take; only a stalled transfer
        // (no chunk accepted for a minute) or a remote that never confirms is an error.
        let stalled = || Error::Unavailable("SSH 上传停滞超过 60 秒".into());
        let handle = self.connect(m, c).await?;
        let mut channel =
            tokio::time::timeout(Duration::from_secs(15), handle.channel_open_session())
                .await
                .map_err(|_| Error::Unavailable("SSH 通道超时".into()))?
                .map_err(remote_error)?;
        let script = "import os,sys,tempfile,shutil; p=sys.argv[1]; f=tempfile.NamedTemporaryFile(dir=os.path.dirname(p),delete=False); shutil.copyfileobj(sys.stdin.buffer,f,65536); f.flush(); os.fsync(f.fileno()); f.close(); os.chmod(f.name,int(sys.argv[2])); os.replace(f.name,p)";
        channel
            .exec(
                true,
                format!(
                    "python3 -c {} {} {}",
                    shell_quote(script),
                    shell_quote(path),
                    mode
                ),
            )
            .await
            .map_err(remote_error)?;
        let mut file = tokio::fs::File::open(local).await.map_err(remote_error)?;
        let mut buffer = vec![0u8; 65536];
        loop {
            let n = file.read(&mut buffer).await.map_err(remote_error)?;
            if n == 0 {
                break;
            }
            tokio::time::timeout(Duration::from_secs(60), channel.data(&buffer[..n]))
                .await
                .map_err(|_| stalled())?
                .map_err(remote_error)?;
        }
        channel.eof().await.map_err(remote_error)?;
        tokio::time::timeout(Duration::from_secs(120), async {
            while let Some(message) = channel.wait().await {
                if let ChannelMsg::ExitStatus { exit_status } = message {
                    return if exit_status == 0 {
                        Ok(())
                    } else {
                        Err(Error::Unavailable("SSH 上传失败".into()))
                    };
                }
            }
            Err(Error::Unavailable("SSH 上传未返回退出码".into()))
        })
        .await
        .map_err(|_| Error::Unavailable("SSH 上传确认超时".into()))?
    }
}

impl SshExecutor {
    async fn terminal_impl(
        &self,
        m: &Machine,
        c: &Credential,
        command: Option<&str>,
        as_root: bool,
        mut input: mpsc::Receiver<TerminalInput>,
        output: mpsc::Sender<TerminalOutput>,
    ) -> Result<()> {
        let handle = self.connect(m, c).await?;
        let mut channel =
            tokio::time::timeout(Duration::from_secs(15), handle.channel_open_session())
                .await
                .map_err(|_| Error::Unavailable("SSH 通道超时".into()))?
                .map_err(remote_error)?;
        channel
            .request_pty(true, "xterm-256color", 120, 32, 0, 0, &[])
            .await
            .map_err(remote_error)?;
        if as_root && m.username != "root" {
            let marker = format!("rigdeck-sudo-{}:", uuid::Uuid::new_v4());
            let ready = format!("rigdeck-ready-{}:", uuid::Uuid::new_v4());
            let root_command = format!(
                "stty echo; printf %s {}; exec {}",
                shell_quote(&ready),
                command.unwrap_or("sh")
            );
            // Disable echo BEFORE requesting a password. No secret in argv or output.
            let setup = format!(
                "stty -echo; exec sudo -S -p {} -- sh -c {}",
                shell_quote(&marker),
                shell_quote(&root_command)
            );
            channel.exec(true, setup).await.map_err(remote_error)?;
            tokio::time::timeout(Duration::from_secs(30), async {
                let mut pending = Vec::new();
                let mut sent = false;
                while let Some(message) = channel.wait().await {
                    match message {
                        ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                            pending.extend_from_slice(&data);
                            if pending.len() > 65536 {
                                return Err(Error::Unavailable("sudo 终端响应异常".into()));
                            }
                            if let Some(index) = pending
                                .windows(ready.len())
                                .position(|v| v == ready.as_bytes())
                            {
                                let tail = pending[index + ready.len()..].to_vec();
                                if !tail.is_empty() {
                                    let _ = output.send(TerminalOutput::Data(tail)).await;
                                }
                                return Ok(());
                            }
                            if pending
                                .windows(marker.len())
                                .any(|v| v == marker.as_bytes())
                            {
                                if sent {
                                    return Err(Error::Unavailable("sudo 密码错误".into()));
                                }
                                let password = c
                                    .sudo_password()
                                    .filter(|p| !p.is_empty())
                                    .ok_or_else(|| {
                                        Error::Unavailable("请在机器设置中填写 sudo 密码".into())
                                    })?;
                                channel
                                    .data(format!("{password}\n").as_bytes())
                                    .await
                                    .map_err(remote_error)?;
                                sent = true;
                                pending.clear();
                            }
                        }
                        ChannelMsg::ExitStatus { .. } => break,
                        _ => {}
                    }
                }
                Err(Error::Unavailable("sudo 终端认证失败".into()))
            })
            .await
            .map_err(|_| Error::Unavailable("sudo 终端认证超时".into()))??;
        } else if let Some(command) = command {
            channel.exec(true, command).await.map_err(remote_error)?;
        } else {
            channel.request_shell(true).await.map_err(remote_error)?;
        }
        loop {
            tokio::select! {
                message=input.recv()=>{match message{Some(TerminalInput::Data(bytes))=>channel.data(bytes.as_slice()).await.map_err(remote_error)?,Some(TerminalInput::Resize(cols,rows))=>channel.window_change(cols.clamp(1,500),rows.clamp(1,200),0,0).await.map_err(remote_error)?,_=>{let _=channel.close().await;break;}}},
                message=channel.wait()=>{match message{Some(ChannelMsg::Data{data})|Some(ChannelMsg::ExtendedData{data,..})=>{if output.send(TerminalOutput::Data(data.to_vec())).await.is_err(){break;}},Some(ChannelMsg::ExitStatus{exit_status})=>{let _=output.send(TerminalOutput::Closed(Some(exit_status))).await;break;},None=>{let _=output.send(TerminalOutput::Closed(None)).await;break;},_=>{}}}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod host_key_tests {
    use super::*;
    use russh::client::Handler;

    fn public_key() -> ssh_key::PublicKey {
        ssh_key::PublicKey::from_openssh(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        )
        .unwrap()
    }

    #[tokio::test]
    async fn discovery_records_key_but_rejects_authentication() {
        let observed = Arc::new(Mutex::new(None));
        let mut probe = KeyProbe {
            observed: observed.clone(),
        };
        let key = public_key();
        assert!(!probe.check_server_key(&key).await.unwrap());
        let result = observed.lock().await.clone().unwrap();
        assert_eq!(
            result.fingerprint,
            key.fingerprint(ssh_key::HashAlg::Sha256).to_string()
        );
        assert_eq!(result.algorithm, "ssh-ed25519");
    }

    #[tokio::test]
    async fn saved_key_is_required_on_subsequent_connections() {
        let key = public_key();
        let mut verifier = KeyVerifier {
            expected: key.fingerprint(ssh_key::HashAlg::Sha256).to_string(),
        };
        assert!(verifier.check_server_key(&key).await.unwrap());
        verifier.expected = "SHA256:changed-host-key".into();
        assert!(!verifier.check_server_key(&key).await.unwrap());
    }

    #[tokio::test]
    async fn closed_endpoint_returns_error_not_an_empty_fingerprint() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        assert!(
            SshExecutor::default()
                .probe_host_key("127.0.0.1", port)
                .await
                .is_err()
        );
    }
}
