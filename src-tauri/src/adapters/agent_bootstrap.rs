use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::adapters::adb;
use crate::services::device_service::{AdbRunOutput, AdbRunner};

pub const AGENT_ABSTRACT_SOCKET: &str = "app_reverse_tools_agent_v1";
pub const AGENT_REMOTE_BINARY: &str = "/data/local/tmp/app_reverse_tools_agent";
const AGENT_REMOTE_PID: &str = "/data/local/tmp/app_reverse_tools_agent.pid";
const AGENT_REMOTE_LOG: &str = "/data/local/tmp/app_reverse_tools_agent.log";
const AGENT_REMOTE_ROLLBACK: &str = "/data/local/tmp/app_reverse_tools_agent.rollback";
const AGENT_REMOTE_SESSION_PREFIX: &str = "/data/local/tmp/.app_reverse_tools_agent";
const SHORT_TIMEOUT: Duration = Duration::from_secs(10);
const PUSH_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceAbi {
    Arm64V8a,
    ArmeabiV7a,
    X86_64,
    X86,
}

impl DeviceAbi {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "arm64-v8a" => Some(Self::Arm64V8a),
            "armeabi-v7a" => Some(Self::ArmeabiV7a),
            "x86_64" => Some(Self::X86_64),
            "x86" => Some(Self::X86),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Arm64V8a => "arm64-v8a",
            Self::ArmeabiV7a => "armeabi-v7a",
            Self::X86_64 => "x86_64",
            Self::X86 => "x86",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AgentBootstrapError {
    #[error("adb is unavailable: {0}")]
    AdbUnavailable(String),
    #[error("device {serial} is not online: {detail}")]
    DeviceOffline { serial: String, detail: String },
    #[error("device {serial} reported unsupported ABI: {abi}")]
    UnsupportedAbi { serial: String, abi: String },
    #[error("agent binary is unavailable: {0}")]
    BinaryUnavailable(String),
    #[error("agent bootstrap step {step} failed: {detail}")]
    CommandFailed { step: &'static str, detail: String },
    #[error("failed to generate session credentials: {0}")]
    Credential(String),
    #[error("adb returned an invalid dynamic forward port: {0}")]
    InvalidForwardPort(String),
    #[error("Agent artifact integrity check failed: expected {expected}, got {actual}")]
    Integrity { expected: String, actual: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentInstallOutcome {
    pub changed: bool,
    pub rollback_available: bool,
    pub previous_sha256: Option<String>,
    pub installed_sha256: String,
}

pub struct AgentLaunch {
    pub auth_token: String,
    remote_session_dir: String,
    remote_token_path: String,
}

pub struct AgentBootstrap {
    runner: Arc<dyn AdbRunner>,
}

impl AgentBootstrap {
    pub fn new(runner: Arc<dyn AdbRunner>) -> Self {
        Self { runner }
    }

    pub async fn ensure_online(&self, serial: &str) -> Result<(), AgentBootstrapError> {
        let output = self
            .run(serial, &adb::cmd_get_state(), SHORT_TIMEOUT)
            .await?;
        if output.exit_code == Some(0) && output.stdout.trim() == "device" {
            return Ok(());
        }
        Err(AgentBootstrapError::DeviceOffline {
            serial: serial.to_owned(),
            detail: command_detail(&output),
        })
    }

    pub async fn device_abi(&self, serial: &str) -> Result<DeviceAbi, AgentBootstrapError> {
        let output = self
            .run(
                serial,
                &adb::cmd_shell("getprop ro.product.cpu.abi"),
                SHORT_TIMEOUT,
            )
            .await?;
        if output.exit_code != Some(0) {
            return Err(command_error("read_abi", &output));
        }
        DeviceAbi::parse(&output.stdout).ok_or_else(|| AgentBootstrapError::UnsupportedAbi {
            serial: serial.to_owned(),
            abi: output.stdout.trim().to_owned(),
        })
    }

    pub async fn install_artifact(
        &self,
        serial: &str,
        binary: &Path,
        expected_sha256: &str,
    ) -> Result<AgentInstallOutcome, AgentBootstrapError> {
        if !binary.is_file() {
            return Err(AgentBootstrapError::BinaryUnavailable(
                binary.display().to_string(),
            ));
        }
        let binary = binary.to_str().ok_or_else(|| {
            AgentBootstrapError::BinaryUnavailable(format!(
                "path is not valid UTF-8: {}",
                binary.display()
            ))
        })?;
        let expected_sha256 = expected_sha256.to_ascii_lowercase();
        let previous_sha256 = self.remote_sha256(serial, AGENT_REMOTE_BINARY).await?;
        if previous_sha256.as_deref() == Some(expected_sha256.as_str()) {
            let rollback_sha256 = self.remote_sha256(serial, AGENT_REMOTE_ROLLBACK).await?;
            return Ok(AgentInstallOutcome {
                changed: false,
                rollback_available: rollback_sha256.is_some(),
                previous_sha256: rollback_sha256.or(previous_sha256),
                installed_sha256: expected_sha256,
            });
        }

        let staging = format!("{AGENT_REMOTE_BINARY}.new.{}", random_hex(12)?);
        if let Err(error) = self
            .run_checked(
                serial,
                &adb::cmd_push(binary, &staging),
                PUSH_TIMEOUT,
                "push_binary",
            )
            .await
        {
            let _ = self.remove_remote_file(serial, &staging).await;
            return Err(error);
        }
        if let Err(error) = self
            .run_checked(
                serial,
                &adb::cmd_shell(&format!("chmod 700 {staging}")),
                SHORT_TIMEOUT,
                "chmod_binary",
            )
            .await
        {
            let _ = self.remove_remote_file(serial, &staging).await;
            return Err(error);
        }
        let staged_sha256 = match self.remote_sha256(serial, &staging).await {
            Ok(sha256) => sha256,
            Err(error) => {
                let _ = self.remove_remote_file(serial, &staging).await;
                return Err(error);
            }
        };
        if staged_sha256.as_deref() != Some(expected_sha256.as_str()) {
            let actual = staged_sha256.unwrap_or_else(|| "missing".into());
            let _ = self.remove_remote_file(serial, &staging).await;
            return Err(AgentBootstrapError::Integrity {
                expected: expected_sha256,
                actual,
            });
        }

        let promote = format!(
            "rm -f {AGENT_REMOTE_ROLLBACK}; had_old=0; if [ -f {AGENT_REMOTE_BINARY} ]; then mv {AGENT_REMOTE_BINARY} {AGENT_REMOTE_ROLLBACK} || exit 1; had_old=1; fi; if mv {staging} {AGENT_REMOTE_BINARY} && chmod 700 {AGENT_REMOTE_BINARY}; then :; else rm -f {AGENT_REMOTE_BINARY}; if [ \"$had_old\" = 1 ]; then mv {AGENT_REMOTE_ROLLBACK} {AGENT_REMOTE_BINARY}; fi; exit 1; fi"
        );
        if let Err(error) = self
            .run_checked(
                serial,
                &adb::cmd_shell(&promote),
                SHORT_TIMEOUT,
                "promote_binary",
            )
            .await
        {
            let _ = self.remove_remote_file(serial, &staging).await;
            return Err(error);
        }
        Ok(AgentInstallOutcome {
            changed: true,
            rollback_available: previous_sha256.is_some(),
            previous_sha256,
            installed_sha256: expected_sha256,
        })
    }

    pub async fn commit_install(&self, serial: &str) -> Result<(), AgentBootstrapError> {
        self.remove_remote_file(serial, AGENT_REMOTE_ROLLBACK).await
    }

    pub async fn rollback_install(&self, serial: &str) -> Result<bool, AgentBootstrapError> {
        let command = format!(
            "if [ -f {AGENT_REMOTE_ROLLBACK} ]; then rm -f {AGENT_REMOTE_BINARY}; mv {AGENT_REMOTE_ROLLBACK} {AGENT_REMOTE_BINARY} && chmod 700 {AGENT_REMOTE_BINARY}; echo restored; fi"
        );
        let output = self
            .run_checked(
                serial,
                &adb::cmd_shell(&command),
                SHORT_TIMEOUT,
                "rollback_binary",
            )
            .await?;
        Ok(output.stdout.lines().any(|line| line.trim() == "restored"))
    }

    pub async fn start(&self, serial: &str) -> Result<AgentLaunch, AgentBootstrapError> {
        let auth_token = random_hex(32)?;
        let token_suffix = random_hex(16)?;
        let remote_session_dir = format!("{AGENT_REMOTE_SESSION_PREFIX}.{token_suffix}");
        let remote_token_path = format!("{remote_session_dir}/token");
        let local_token = LocalTokenFile::create(&auth_token)?;

        self.run_checked(
            serial,
            &adb::cmd_shell(&format!(
                "umask 077; mkdir {remote_session_dir} && chmod 700 {remote_session_dir}"
            )),
            SHORT_TIMEOUT,
            "create_token_directory",
        )
        .await?;
        if let Err(error) = self
            .run_checked(
                serial,
                &adb::cmd_push(local_token.path_str()?, &remote_token_path),
                SHORT_TIMEOUT,
                "push_token",
            )
            .await
        {
            let _ = self
                .remove_remote_session(serial, &remote_token_path, &remote_session_dir)
                .await;
            return Err(error);
        }
        if let Err(error) = self
            .run_checked(
                serial,
                &adb::cmd_shell(&format!("chmod 600 {remote_token_path}")),
                SHORT_TIMEOUT,
                "chmod_token",
            )
            .await
        {
            let _ = self
                .remove_remote_session(serial, &remote_token_path, &remote_session_dir)
                .await;
            return Err(error);
        }

        let start_command = format!(
            "nohup {AGENT_REMOTE_BINARY} --localabstract {AGENT_ABSTRACT_SOCKET} --auth-token-file {remote_token_path} >{AGENT_REMOTE_LOG} 2>&1 </dev/null & echo $! >{AGENT_REMOTE_PID}; chmod 600 {AGENT_REMOTE_LOG} {AGENT_REMOTE_PID}"
        );
        if let Err(error) = self
            .run_checked(
                serial,
                &adb::cmd_shell(&start_command),
                SHORT_TIMEOUT,
                "start_agent",
            )
            .await
        {
            let _ = self
                .remove_remote_session(serial, &remote_token_path, &remote_session_dir)
                .await;
            return Err(error);
        }

        Ok(AgentLaunch {
            auth_token,
            remote_session_dir,
            remote_token_path,
        })
    }

    pub async fn forward(&self, serial: &str) -> Result<(String, u16), AgentBootstrapError> {
        let remote = format!("localabstract:{AGENT_ABSTRACT_SOCKET}");
        let output = self
            .run(serial, &adb::cmd_forward("tcp:0", &remote), SHORT_TIMEOUT)
            .await?;
        if output.exit_code != Some(0) {
            return Err(command_error("forward", &output));
        }
        let port = adb::parse_dynamic_forward_port(&output.stdout).ok_or_else(|| {
            AgentBootstrapError::InvalidForwardPort(output.stdout.trim().to_owned())
        })?;
        Ok((format!("tcp:{port}"), port))
    }

    pub async fn remove_forward(
        &self,
        serial: &str,
        local: &str,
    ) -> Result<(), AgentBootstrapError> {
        self.run_checked(
            serial,
            &adb::cmd_forward_remove(Some(local)),
            SHORT_TIMEOUT,
            "remove_forward",
        )
        .await?;
        Ok(())
    }

    pub async fn stop(&self, serial: &str) -> Result<(), AgentBootstrapError> {
        let command = format!(
            "if [ -f {AGENT_REMOTE_PID} ]; then pid=$(cat {AGENT_REMOTE_PID}); case \"$pid\" in ''|*[!0-9]*) ;; *) kill \"$pid\" 2>/dev/null || true; attempt=0; while kill -0 \"$pid\" 2>/dev/null && [ \"$attempt\" -lt 20 ]; do sleep 0.05; attempt=$((attempt + 1)); done; if kill -0 \"$pid\" 2>/dev/null; then kill -9 \"$pid\" 2>/dev/null || true; sleep 0.05; fi ;; esac; fi; rm -f {AGENT_REMOTE_PID}"
        );
        self.run_checked(
            serial,
            &adb::cmd_shell(&command),
            SHORT_TIMEOUT,
            "stop_agent",
        )
        .await?;
        Ok(())
    }

    pub async fn cleanup_launch(&self, serial: &str, launch: &AgentLaunch) {
        let _ = self
            .remove_remote_session(
                serial,
                &launch.remote_token_path,
                &launch.remote_session_dir,
            )
            .await;
    }

    async fn remove_remote_session(
        &self,
        serial: &str,
        token_path: &str,
        session_dir: &str,
    ) -> Result<(), AgentBootstrapError> {
        self.run_checked(
            serial,
            &adb::cmd_shell(&format!(
                "rm -f {token_path}; rmdir {session_dir} 2>/dev/null || true"
            )),
            SHORT_TIMEOUT,
            "remove_token_directory",
        )
        .await?;
        Ok(())
    }

    async fn remote_sha256(
        &self,
        serial: &str,
        path: &str,
    ) -> Result<Option<String>, AgentBootstrapError> {
        let command = format!(
            "if [ -f {path} ]; then if command -v sha256sum >/dev/null 2>&1; then sha256sum {path}; else toybox sha256sum {path}; fi; fi"
        );
        let output = self
            .run_checked(
                serial,
                &adb::cmd_shell(&command),
                SHORT_TIMEOUT,
                "sha256_binary",
            )
            .await?;
        if output.stdout.trim().is_empty() {
            return Ok(None);
        }
        adb::parse_sha256(&output.stdout)
            .map(Some)
            .ok_or_else(|| command_error("parse_sha256_binary", &output))
    }

    async fn remove_remote_file(
        &self,
        serial: &str,
        path: &str,
    ) -> Result<(), AgentBootstrapError> {
        self.run_checked(
            serial,
            &adb::cmd_shell(&format!("rm -f {path}")),
            SHORT_TIMEOUT,
            "remove_remote_file",
        )
        .await?;
        Ok(())
    }

    async fn run_checked(
        &self,
        serial: &str,
        command: &[String],
        timeout: Duration,
        step: &'static str,
    ) -> Result<AdbRunOutput, AgentBootstrapError> {
        let output = self.run(serial, command, timeout).await?;
        if output.exit_code != Some(0) {
            return Err(command_error(step, &output));
        }
        Ok(output)
    }

    async fn run(
        &self,
        serial: &str,
        command: &[String],
        timeout: Duration,
    ) -> Result<AdbRunOutput, AgentBootstrapError> {
        let environment = self.runner.environment().await;
        let adb_path = environment.path.ok_or_else(|| {
            AgentBootstrapError::AdbUnavailable(
                environment
                    .probe_error
                    .or(environment.hint)
                    .unwrap_or_else(|| "adb path is unavailable".into()),
            )
        })?;
        let arguments = adb::build_args(Some(serial), command);
        self.runner
            .run(&adb_path, &arguments, timeout)
            .await
            .map_err(|error| AgentBootstrapError::CommandFailed {
                step: "run_adb",
                detail: error.to_string(),
            })
    }
}

fn random_hex(byte_count: usize) -> Result<String, AgentBootstrapError> {
    let mut bytes = vec![0_u8; byte_count];
    getrandom::fill(&mut bytes)
        .map_err(|error| AgentBootstrapError::Credential(error.to_string()))?;
    let mut encoded = String::with_capacity(byte_count * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(encoded)
}

fn command_detail(output: &AdbRunOutput) -> String {
    let detail = if output.stderr.trim().is_empty() {
        output.stdout.trim()
    } else {
        output.stderr.trim()
    };
    format!("exit {:?}: {detail}", output.exit_code)
}

fn command_error(step: &'static str, output: &AdbRunOutput) -> AgentBootstrapError {
    AgentBootstrapError::CommandFailed {
        step,
        detail: command_detail(output),
    }
}

struct LocalTokenFile {
    path: PathBuf,
}

impl LocalTokenFile {
    fn create(auth_token: &str) -> Result<Self, AgentBootstrapError> {
        let suffix = random_hex(16)?;
        let path = std::env::temp_dir().join(format!("app-reverse-tools-agent-{suffix}.token"));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&path)
            .map_err(|error| AgentBootstrapError::Credential(error.to_string()))?;
        file.write_all(auth_token.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|error| AgentBootstrapError::Credential(error.to_string()))?;
        Ok(Self { path })
    }

    fn path_str(&self) -> Result<&str, AgentBootstrapError> {
        self.path.to_str().ok_or_else(|| {
            AgentBootstrapError::Credential("temporary path is not valid UTF-8".into())
        })
    }
}

impl Drop for LocalTokenFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;

    use crate::core::error::CoreResult;
    use crate::services::device_service::{AdbEnvironment, AdbRunOutput};

    use super::*;

    struct RecordingRunner {
        responses: Mutex<Vec<AdbRunOutput>>,
        commands: Mutex<Vec<Vec<String>>>,
    }

    impl RecordingRunner {
        fn new(mut responses: Vec<AdbRunOutput>) -> Self {
            responses.reverse();
            Self {
                responses: Mutex::new(responses),
                commands: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl AdbRunner for RecordingRunner {
        async fn run(
            &self,
            _adb_path: &str,
            args: &[String],
            _timeout: Duration,
        ) -> CoreResult<AdbRunOutput> {
            self.commands.lock().unwrap().push(args.to_vec());
            Ok(self.responses.lock().unwrap().pop().unwrap())
        }

        async fn environment(&self) -> AdbEnvironment {
            AdbEnvironment {
                installed: true,
                path: Some("/mock/adb".into()),
                source: Some("mock".into()),
                version: None,
                hint: None,
                probe_error: None,
            }
        }

        fn invalidate_cache(&self) {}
    }

    fn ok(stdout: &str) -> AdbRunOutput {
        AdbRunOutput {
            stdout: stdout.into(),
            stderr: String::new(),
            exit_code: Some(0),
        }
    }

    #[tokio::test]
    async fn bootstrap_uses_serial_scoped_commands_and_never_places_token_in_arguments() {
        let runner = Arc::new(RecordingRunner::new(vec![
            ok("device\n"),
            ok("arm64-v8a\n"),
            ok(""),
            ok("1 file pushed\n"),
            ok(""),
            ok("9a3a45d01531a20e89ac6ae10b0b0beb0492acd7216a368aa062d1a5fecaf9cd  staged\n"),
            ok(""),
            ok(""),
            ok("1 file pushed\n"),
            ok(""),
            ok(""),
            ok("43210\n"),
        ]));
        let bootstrap = AgentBootstrap::new(runner.clone());
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("android-agent");
        fs::write(&binary, b"binary").unwrap();

        bootstrap.ensure_online("SERIAL-1").await.unwrap();
        assert_eq!(
            bootstrap.device_abi("SERIAL-1").await.unwrap(),
            DeviceAbi::Arm64V8a
        );
        let install = bootstrap
            .install_artifact(
                "SERIAL-1",
                &binary,
                "9a3a45d01531a20e89ac6ae10b0b0beb0492acd7216a368aa062d1a5fecaf9cd",
            )
            .await
            .unwrap();
        let launch = bootstrap.start("SERIAL-1").await.unwrap();
        let (local, port) = bootstrap.forward("SERIAL-1").await.unwrap();

        assert_eq!(launch.auth_token.len(), 64);
        assert!(
            launch
                .auth_token
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
        assert_eq!((local.as_str(), port), ("tcp:43210", 43210));
        assert!(install.changed);
        assert!(!install.rollback_available);
        let commands = runner.commands.lock().unwrap();
        assert!(
            commands
                .iter()
                .all(|args| args.starts_with(&["-s".to_string(), "SERIAL-1".to_string()]))
        );
        assert!(
            commands
                .iter()
                .flatten()
                .all(|argument| !argument.contains(&launch.auth_token))
        );
        let token_push = &commands[8];
        assert!(!Path::new(&token_push[3]).exists());
    }

    #[tokio::test]
    async fn install_reuses_matching_remote_binary_without_push() {
        let digest = "9a3a45d01531a20e89ac6ae10b0b0beb0492acd7216a368aa062d1a5fecaf9cd";
        let runner = Arc::new(RecordingRunner::new(vec![
            ok(&format!("{digest}  current\n")),
            ok(""),
        ]));
        let bootstrap = AgentBootstrap::new(runner.clone());
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("android-agent");
        fs::write(&binary, b"binary").unwrap();

        let install = bootstrap
            .install_artifact("SERIAL-1", &binary, digest)
            .await
            .unwrap();
        assert!(!install.changed);
        assert!(!install.rollback_available);
        assert!(
            runner
                .commands
                .lock()
                .unwrap()
                .iter()
                .all(|args| !args.iter().any(|argument| argument == "push"))
        );
    }

    #[tokio::test]
    async fn upgrade_keeps_previous_binary_until_commit_or_rollback() {
        let old = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let new = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let runner = Arc::new(RecordingRunner::new(vec![
            ok(&format!("{old}  current\n")),
            ok("1 file pushed\n"),
            ok(""),
            ok(&format!("{new}  staged\n")),
            ok(""),
            ok("restored\n"),
        ]));
        let bootstrap = AgentBootstrap::new(runner.clone());
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("android-agent");
        fs::write(&binary, b"new binary").unwrap();

        let install = bootstrap
            .install_artifact("SERIAL-1", &binary, new)
            .await
            .unwrap();
        assert!(install.changed);
        assert!(install.rollback_available);
        assert_eq!(install.previous_sha256.as_deref(), Some(old));
        assert!(bootstrap.rollback_install("SERIAL-1").await.unwrap());
        let commands = runner.commands.lock().unwrap();
        assert!(
            commands[4]
                .iter()
                .any(|argument| argument.contains("had_old"))
        );
        assert!(
            commands[5]
                .iter()
                .any(|argument| argument.contains("echo restored"))
        );
    }

    #[tokio::test]
    async fn staging_hash_mismatch_is_removed_before_promotion() {
        let old = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let expected = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let actual = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
        let runner = Arc::new(RecordingRunner::new(vec![
            ok(&format!("{old}  current\n")),
            ok("1 file pushed\n"),
            ok(""),
            ok(&format!("{actual}  staged\n")),
            ok(""),
        ]));
        let bootstrap = AgentBootstrap::new(runner.clone());
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("android-agent");
        fs::write(&binary, b"new binary").unwrap();

        let error = bootstrap
            .install_artifact("SERIAL-1", &binary, expected)
            .await
            .unwrap_err();
        assert!(matches!(error, AgentBootstrapError::Integrity { .. }));
        let commands = runner.commands.lock().unwrap();
        assert_eq!(commands.len(), 5);
        assert!(
            commands[4]
                .iter()
                .any(|argument| argument.starts_with("rm -f "))
        );
    }

    #[tokio::test]
    async fn stop_waits_for_process_exit_before_returning() {
        let runner = Arc::new(RecordingRunner::new(vec![ok("")]));
        let bootstrap = AgentBootstrap::new(runner.clone());
        bootstrap.stop("SERIAL-1").await.unwrap();
        let commands = runner.commands.lock().unwrap();
        let command = commands[0].last().unwrap();
        assert!(command.contains("while kill -0"));
        assert!(command.contains("kill -9"));
        assert!(command.contains("attempt"));
    }

    #[test]
    fn abi_parser_is_explicit_and_rejects_unknown_values() {
        assert_eq!(DeviceAbi::parse("arm64-v8a\n"), Some(DeviceAbi::Arm64V8a));
        assert_eq!(DeviceAbi::parse("riscv64"), None);
    }
}
