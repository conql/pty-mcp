#![allow(dead_code)]

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, ensure};
use rmcp::{
    ClientHandler, ServiceExt,
    model::{CallToolRequestParams, ReadResourceRequestParams},
    service::RunningService,
};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use tokio::{
    io::AsyncReadExt,
    process::{Child, ChildStderr, Command},
    sync::Mutex,
};

use super::fake_bins::{FakeBins, TempSandbox};

#[derive(Debug, Clone, Default)]
pub struct DummyClient;

impl ClientHandler for DummyClient {}

pub struct E2eHarness {
    sandbox: TempSandbox,
    fake_bins: FakeBins,
    child: Child,
    client: Option<RunningService<rmcp::RoleClient, DummyClient>>,
    stderr_buffer: Arc<Mutex<String>>,
    stderr_task: Option<tokio::task::JoinHandle<()>>,
    workspace_root: PathBuf,
    managed_mount_root: PathBuf,
    remote_root: PathBuf,
}

pub fn resolve_binary_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_pty-mcp").map(PathBuf::from) {
        return Ok(path);
    }

    let current_exe = std::env::current_exe().context("failed to resolve current test executable")?;
    let debug_dir = current_exe
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| anyhow!("failed to derive target/debug from {}", current_exe.display()))?;
    let candidate = debug_dir.join(format!("pty-mcp{}", std::env::consts::EXE_SUFFIX));
    ensure!(
        candidate.is_file(),
        "could not find pty-mcp binary at {}; current_exe={}",
        candidate.display(),
        current_exe.display()
    );
    Ok(candidate)
}

#[derive(Debug, Default)]
pub struct E2eHarnessBuilder {
    name: String,
    env_overrides: BTreeMap<String, String>,
}

impl E2eHarnessBuilder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            env_overrides: BTreeMap::new(),
        }
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env_overrides.insert(key.into(), value.into());
        self
    }

    pub async fn start(self) -> Result<E2eHarness> {
        let sandbox = TempSandbox::new(&self.name)?;
        let fake_bins = FakeBins::install(sandbox.root())?;
        let workspace_root = sandbox.path("workspace");
        let managed_mount_root = sandbox.path("managed-mounts");
        let remote_root = sandbox.path("remote");
        std::fs::create_dir_all(&workspace_root)?;
        std::fs::create_dir_all(&managed_mount_root)?;
        std::fs::create_dir_all(&remote_root)?;

        let bin = resolve_binary_path()?;

        let mut command = Command::new(bin);
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        command.env(
            "PTY_MCP_ALLOWED_CWD_ROOTS",
            format!(
                "{}:{}:{}",
                workspace_root.display(),
                managed_mount_root.display(),
                remote_root.display()
            ),
        );
        command.env("PTY_MCP_SSH_BIN_PATH", &fake_bins.ssh_path);
        command.env("PTY_MCP_SSHFS_BIN_PATH", &fake_bins.sshfs_path);
        command.env("PTY_MCP_UMOUNT_BIN_PATH", &fake_bins.umount_path);
        command.env("PTY_MCP_SSH_MANAGED_MOUNT_ROOT", &managed_mount_root);
        command.env("RUST_LOG", "pty_mcp=info");

        for (key, value) in self.env_overrides {
            command.env(key, value);
        }

        let mut child = command.spawn().context("failed to spawn pty-mcp child")?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("child stdout was not piped"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("child stdin was not piped"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("child stderr was not piped"))?;

        let stderr_buffer = Arc::new(Mutex::new(String::new()));
        let stderr_task = Some(spawn_stderr_capture(stderr, stderr_buffer.clone()));

        let client = DummyClient
            .serve((stdout, stdin))
            .await
            .context("failed to initialize MCP client over child stdio")?;

        Ok(E2eHarness {
            sandbox,
            fake_bins,
            child,
            client: Some(client),
            stderr_buffer,
            stderr_task,
            workspace_root,
            managed_mount_root,
            remote_root,
        })
    }
}

impl E2eHarness {
    pub fn builder(name: impl Into<String>) -> E2eHarnessBuilder {
        E2eHarnessBuilder::new(name)
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn managed_mount_root(&self) -> &Path {
        &self.managed_mount_root
    }

    pub fn remote_root(&self) -> &Path {
        &self.remote_root
    }

    pub fn sandbox_root(&self) -> &Path {
        self.sandbox.root()
    }

    pub fn fake_bins(&self) -> &FakeBins {
        &self.fake_bins
    }

    pub async fn list_tool_names(&self) -> Result<Vec<String>> {
        let tools = self
            .client()?
            .peer()
            .list_all_tools()
            .await
            .context("failed to list tools")?;
        Ok(tools.into_iter().map(|tool| tool.name.to_string()).collect())
    }

    pub async fn list_resource_uris(&self) -> Result<Vec<String>> {
        let resources = self
            .client()?
            .peer()
            .list_all_resources()
            .await
            .context("failed to list resources")?;
        Ok(resources
            .into_iter()
            .map(|resource| resource.raw.uri.to_string())
            .collect())
    }

    pub async fn list_resource_template_uris(&self) -> Result<Vec<String>> {
        let templates = self
            .client()?
            .peer()
            .list_all_resource_templates()
            .await
            .context("failed to list resource templates")?;
        Ok(templates
            .into_iter()
            .map(|template| template.raw.uri_template.to_string())
            .collect())
    }

    pub async fn call_tool_typed<T>(&self, name: &str, args: Value) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let result = self.call_tool_raw(name, args).await?;
        let description = self.describe_result(&result);
        if result.is_error == Some(true) {
            return Err(anyhow!("tool {name} returned error result: {description}"));
        }

        result
            .into_typed::<T>()
            .with_context(|| format!("failed to decode tool result for {name}; result={description}"))
    }

    pub async fn call_tool_error(&self, name: &str, args: Value) -> Result<Value> {
        let result = self.call_tool_raw(name, args).await?;
        ensure!(
            result.is_error == Some(true),
            "expected tool {name} to return is_error=true; result={}",
            self.describe_result(&result)
        );
        Ok(result
            .structured_content
            .unwrap_or_else(|| serde_json::json!({ "message": "missing structured error" })))
    }

    pub async fn call_tool_raw(
        &self,
        name: &str,
        args: Value,
    ) -> Result<rmcp::model::CallToolResult> {
        let arguments = as_arguments_map(args)
            .with_context(|| format!("tool {name} requires object arguments"))?;
        self.client()?
            .call_tool(CallToolRequestParams::new(name.to_owned()).with_arguments(arguments))
            .await
            .with_context(|| format!("tool {name} call failed"))
    }

    pub async fn read_resource_json(&self, uri: &str) -> Result<Value> {
        let response = self
            .client()?
            .read_resource(ReadResourceRequestParams::new(uri))
            .await
            .with_context(|| format!("failed to read resource {uri}"))?;
        let text = match &response.contents[0] {
            rmcp::model::ResourceContents::TextResourceContents { text, .. } => text,
            other => {
                return Err(anyhow!(
                    "unexpected resource contents for {uri}: {other:?}; diagnostics={}",
                    self.diagnostics().await
                ));
            }
        };

        serde_json::from_str(text)
            .with_context(|| format!("failed to decode resource {uri} as json: {text}"))
    }

    pub async fn wait_until<F, Fut>(&self, label: &str, mut check: F) -> Result<()>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<bool>>,
    {
        for _ in 0..50 {
            if check().await? {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        Err(anyhow!(
            "timed out waiting for {label}; diagnostics={}",
            self.diagnostics().await
        ))
    }

    pub async fn stderr_text(&self) -> String {
        self.stderr_buffer.lock().await.clone()
    }

    pub async fn diagnostics(&self) -> String {
        format!(
            "sandbox_root={} stderr={:?} ssh_log={:?} sshfs_log={:?} umount_log={:?}",
            self.sandbox.root().display(),
            self.stderr_text().await,
            self.fake_bins.read_ssh_log(),
            self.fake_bins.read_sshfs_log(),
            self.fake_bins.read_umount_log(),
        )
    }

    pub async fn shutdown(mut self) -> Result<()> {
        if let Some(client) = self.client.take() {
            let _ = client.cancel().await;
        }

        let status = tokio::time::timeout(Duration::from_secs(3), self.child.wait())
            .await
            .context("timed out waiting for pty-mcp child to exit")?
            .context("failed while waiting for pty-mcp child")?;

        if !status.success() {
            return Err(anyhow!(
                "pty-mcp child exited unsuccessfully: status={status}; diagnostics={}",
                self.diagnostics().await
            ));
        }

        if let Some(stderr_task) = self.stderr_task.take() {
            let _ = stderr_task.await;
        }

        Ok(())
    }

    fn client(&self) -> Result<&RunningService<rmcp::RoleClient, DummyClient>> {
        self.client
            .as_ref()
            .ok_or_else(|| anyhow!("MCP client is no longer available"))
    }

    fn describe_result(&self, result: &rmcp::model::CallToolResult) -> String {
        serde_json::json!({
            "is_error": result.is_error,
            "structured_content": result.structured_content,
            "content_len": result.content.len(),
        })
        .to_string()
    }
}

impl Drop for E2eHarness {
    fn drop(&mut self) {
        if let Some(client) = self.client.take() {
            tokio::spawn(async move {
                let _ = client.cancel().await;
            });
        }

        let _ = self.child.start_kill();
    }
}

fn spawn_stderr_capture(stderr: ChildStderr, buffer: Arc<Mutex<String>>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut reader = stderr;
        let mut bytes = Vec::new();
        let _ = reader.read_to_end(&mut bytes).await;
        let text = String::from_utf8_lossy(&bytes).to_string();
        *buffer.lock().await = text;
    })
}

fn as_arguments_map(value: Value) -> Result<Map<String, Value>> {
    value
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow!("expected JSON object arguments, got {value}"))
}
