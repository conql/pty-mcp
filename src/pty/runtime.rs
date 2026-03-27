use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Result, anyhow, bail};
use chrono::Utc;
#[cfg(unix)]
use nix::{
    sys::signal::{Signal, kill as kill_process},
    unistd::Pid,
};
use portable_pty::{Child, ChildKiller, CommandBuilder, PtySize, native_pty_system};
use tokio::sync::{mpsc, watch};

use crate::session::{ExitInfo, SignalKind};

const DEFAULT_PTY_ROWS: u16 = 24;
const DEFAULT_PTY_COLS: u16 = 80;
const DEFAULT_OUTPUT_CHANNEL_CAPACITY: usize = 256;

pub type PtyOutputReceiver = mpsc::Receiver<Vec<u8>>;

#[derive(Debug, Clone)]
pub struct PtySpawnRequest {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
    pub rows: u16,
    pub cols: u16,
}

impl PtySpawnRequest {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            rows: DEFAULT_PTY_ROWS,
            cols: DEFAULT_PTY_COLS,
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    pub fn size(mut self, rows: u16, cols: u16) -> Self {
        self.rows = rows;
        self.cols = cols;
        self
    }
}

pub struct PtySpawnResult {
    pub pid: Option<u32>,
    pub output: PtyOutputReceiver,
    pub handle: PtySessionHandle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeExitStatus {
    pub exit_info: ExitInfo,
}

impl RuntimeExitStatus {
    fn from_exit(exit_status: portable_pty::ExitStatus) -> Self {
        Self {
            exit_info: ExitInfo {
                exit_code: Some(exit_status.exit_code() as i32),
                exit_signal: exit_status.signal().map(str::to_string),
                finished_at: Some(Utc::now()),
            },
        }
    }
}

#[derive(Clone)]
pub struct PtySessionHandle {
    inner: Arc<PtySessionInner>,
}

struct PtySessionInner {
    pid: Option<u32>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    killer: Arc<Mutex<Box<dyn ChildKiller + Send + Sync>>>,
    exit_tx: watch::Sender<Option<Result<RuntimeExitStatus, String>>>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct PtyRuntime;

impl PtyRuntime {
    pub async fn spawn(&self, request: PtySpawnRequest) -> Result<PtySpawnResult> {
        let artifacts = tokio::task::spawn_blocking(move || spawn_blocking(request))
            .await
            .map_err(|join_error| anyhow!("pty spawn task failed: {join_error}"))??;

        let SpawnArtifacts {
            pid,
            reader,
            writer,
            killer,
            child,
        } = artifacts;

        let (output_tx, output_rx) = mpsc::channel(DEFAULT_OUTPUT_CHANNEL_CAPACITY);
        std::thread::spawn(move || read_output_loop(reader, output_tx));

        let (exit_tx, _) = watch::channel(None::<Result<RuntimeExitStatus, String>>);
        let wait_tx = exit_tx.clone();
        std::thread::spawn(move || wait_for_exit(child, wait_tx));

        let handle = PtySessionHandle {
            inner: Arc::new(PtySessionInner {
                pid,
                writer: Arc::new(Mutex::new(writer)),
                killer: Arc::new(Mutex::new(killer)),
                exit_tx,
            }),
        };

        Ok(PtySpawnResult {
            pid,
            output: output_rx,
            handle,
        })
    }
}

impl PtySessionHandle {
    pub fn pid(&self) -> Option<u32> {
        self.inner.pid
    }

    pub fn exit_status(&self) -> Option<Result<RuntimeExitStatus, String>> {
        self.inner.exit_tx.borrow().clone()
    }

    pub async fn write_plain(&self, data: impl Into<String>) -> Result<usize> {
        self.write_inner(data.into()).await
    }

    pub async fn write_escaped(&self, data: impl AsRef<str>) -> Result<usize> {
        let decoded = decode_escaped_input(data.as_ref())?;
        self.write_inner(decoded).await
    }

    pub async fn signal(&self, signal: SignalKind) -> Result<()> {
        self.ensure_running()?;

        let pid = self.inner.pid;
        let killer = Arc::clone(&self.inner.killer);

        tokio::task::spawn_blocking(move || signal_blocking(pid, killer, signal))
            .await
            .map_err(|join_error| anyhow!("signal task failed: {join_error}"))?
    }

    pub async fn wait(&self, timeout: Option<Duration>) -> Result<Option<RuntimeExitStatus>> {
        if let Some(result) = self.exit_status() {
            return result.map(Some).map_err(|message| anyhow!(message));
        }

        let mut exit_rx = self.inner.exit_tx.subscribe();
        let changed = async {
            loop {
                exit_rx
                    .changed()
                    .await
                    .map_err(|_| anyhow!("exit watcher unexpectedly closed"))?;

                if let Some(result) = exit_rx.borrow().clone() {
                    return result.map(Some).map_err(|message| anyhow!(message));
                }
            }
        };

        match timeout {
            Some(duration) => match tokio::time::timeout(duration, changed).await {
                Ok(result) => result,
                Err(_) => Ok(None),
            },
            None => changed.await,
        }
    }

    async fn write_inner(&self, data: String) -> Result<usize> {
        self.ensure_running()?;

        let bytes = data.into_bytes();
        let byte_count = bytes.len();
        let writer = Arc::clone(&self.inner.writer);

        tokio::task::spawn_blocking(move || -> Result<usize> {
            let mut writer = writer.lock().expect("pty writer mutex poisoned");
            writer
                .write_all(&bytes)
                .map_err(|source| anyhow!("failed to write to PTY: {source}"))?;
            writer
                .flush()
                .map_err(|source| anyhow!("failed to flush PTY writer: {source}"))?;
            Ok(byte_count)
        })
        .await
        .map_err(|join_error| anyhow!("write task failed: {join_error}"))?
    }

    fn ensure_running(&self) -> Result<()> {
        if self.exit_status().is_some() {
            bail!("PTY session is no longer running");
        }

        Ok(())
    }
}

struct SpawnArtifacts {
    pid: Option<u32>,
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    child: Box<dyn Child + Send + Sync>,
}

impl std::fmt::Debug for PtySpawnResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtySpawnResult")
            .field("pid", &self.pid)
            .field("handle", &self.handle)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for PtySessionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtySessionHandle")
            .field("pid", &self.pid())
            .field("has_exit_status", &self.exit_status().is_some())
            .finish()
    }
}

fn spawn_blocking(request: PtySpawnRequest) -> Result<SpawnArtifacts> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: request.rows,
            cols: request.cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|source| anyhow!("failed to open PTY pair: {source}"))?;

    let mut command = CommandBuilder::new(&request.command);
    command.args(&request.args);

    if let Some(cwd) = &request.cwd {
        command.cwd(cwd);
    }

    for (key, value) in &request.env {
        command.env(key, value);
    }

    let child = pair.slave.spawn_command(command).map_err(|source| {
        anyhow!(
            "failed to spawn PTY command '{}': {source}",
            request.command
        )
    })?;

    drop(pair.slave);

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|source| anyhow!("failed to clone PTY reader: {source}"))?;

    let writer = pair
        .master
        .take_writer()
        .map_err(|source| anyhow!("failed to acquire PTY writer: {source}"))?;

    let pid = child.process_id();
    let killer = child.clone_killer();

    Ok(SpawnArtifacts {
        pid,
        reader,
        writer,
        killer,
        child,
    })
}

fn read_output_loop(mut reader: Box<dyn Read + Send>, output_tx: mpsc::Sender<Vec<u8>>) {
    let mut buffer = vec![0_u8; 4096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read_bytes) => {
                if output_tx
                    .blocking_send(buffer[..read_bytes].to_vec())
                    .is_err()
                {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

fn wait_for_exit(
    mut child: Box<dyn Child + Send + Sync>,
    exit_tx: watch::Sender<Option<Result<RuntimeExitStatus, String>>>,
) {
    let result = child
        .wait()
        .map(RuntimeExitStatus::from_exit)
        .map_err(|source| format!("failed while waiting for PTY child: {source}"));
    let _ = exit_tx.send(Some(result));
}

fn signal_blocking(
    pid: Option<u32>,
    killer: Arc<Mutex<Box<dyn ChildKiller + Send + Sync>>>,
    signal: SignalKind,
) -> Result<()> {
    #[cfg(unix)]
    {
        if let Some(pid) = pid {
            if signal != SignalKind::Sigkill {
                let os_signal = match signal {
                    SignalKind::Sigint => Signal::SIGINT,
                    SignalKind::Sigterm => Signal::SIGTERM,
                    SignalKind::Sigkill => Signal::SIGKILL,
                };

                kill_process(Pid::from_raw(pid as i32), os_signal).map_err(|source| {
                    anyhow!("failed to send {signal:?} to PTY process {pid}: {source}")
                })?;
                return Ok(());
            }
        }
    }

    let mut killer = killer.lock().expect("pty killer mutex poisoned");
    killer
        .kill()
        .map_err(|source| anyhow!("failed to terminate PTY process: {source}"))
}

fn decode_escaped_input(input: &str) -> Result<String> {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars();

    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }

        let escaped = chars
            .next()
            .ok_or_else(|| anyhow!("input ended with a dangling escape sequence"))?;

        match escaped {
            '\\' => output.push('\\'),
            '0' => output.push('\0'),
            'n' => output.push('\n'),
            'r' => output.push('\r'),
            't' => output.push('\t'),
            'x' => {
                let high = chars.next().ok_or_else(|| invalid_hex_escape(input))?;
                let low = chars.next().ok_or_else(|| invalid_hex_escape(input))?;
                let mut hex = String::with_capacity(2);
                hex.push(high);
                hex.push(low);
                let value = u8::from_str_radix(&hex, 16).map_err(|_| invalid_hex_escape(input))?;
                output.push(value as char);
            }
            other => {
                return Err(anyhow!("unsupported escape sequence: \\{other}"));
            }
        }
    }

    Ok(output)
}

fn invalid_hex_escape(input: &str) -> anyhow::Error {
    anyhow!("invalid hex escape in input: {input}")
}
