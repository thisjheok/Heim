use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::SystemTime,
};

use serde::Serialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, State};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{Mutex, oneshot},
    time::{Duration, timeout},
};

use crate::{database, ensure_project, has_current_approved_map};

#[derive(Default)]
pub struct AgentManager {
    session: Mutex<Option<Session>>,
}

pub(crate) struct DesignResponse {
    pub text: String,
    pub revision: i64,
}

struct Session {
    project_id: i64,
    mode: AgentMode,
    thread_id: String,
    _child: Child,
    writer: Arc<Mutex<ChildStdin>>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
    approvals: Arc<Mutex<HashSet<i64>>>,
    next_id: AtomicU64,
    busy: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    turn_id: Arc<Mutex<Option<String>>>,
    snapshot: Arc<Mutex<Option<FileSnapshot>>>,
    response: Arc<Mutex<Option<String>>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AgentMode {
    Design,
    Development,
}

impl AgentMode {
    fn thread_policy(self) -> Value {
        match self {
            Self::Design => json!({"sandbox":"read-only", "approvalPolicy":"never"}),
            Self::Development => {
                json!({"sandbox":"workspace-write", "approvalPolicy":"on-request"})
            }
        }
    }

    fn permits_approval(self, decision: &str) -> bool {
        self == Self::Development && matches!(decision, "accept" | "decline")
    }
}

type FileSnapshot = HashMap<String, (u64, Option<SystemTime>)>;

fn file_snapshot(root: &Path) -> FileSnapshot {
    let mut files = HashMap::new();
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || !entry.file_type().is_dir()
                || !matches!(
                    entry.file_name().to_str(),
                    Some(".git" | "node_modules" | "target" | "dist")
                )
        })
        .flatten()
        .take(20_000)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        if let (Ok(name), Ok(metadata)) = (entry.path().strip_prefix(root), entry.metadata()) {
            files.insert(
                name.to_string_lossy().into_owned(),
                (metadata.len(), metadata.modified().ok()),
            );
        }
    }
    files
}

fn changed_files(before: &FileSnapshot, after: &FileSnapshot) -> Vec<String> {
    let mut changed = after
        .iter()
        .filter(|(path, info)| before.get(*path) != Some(*info))
        .map(|(path, _)| path.clone())
        .collect::<Vec<_>>();
    changed.extend(
        before
            .keys()
            .filter(|path| !after.contains_key(*path))
            .cloned(),
    );
    changed.sort();
    changed
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentEvent {
    project_id: i64,
    kind: &'static str,
    text: String,
    item_id: Option<String>,
    request_id: Option<i64>,
}

fn emit(
    app: &AppHandle,
    project_id: i64,
    kind: &'static str,
    text: impl Into<String>,
    item_id: Option<String>,
    request_id: Option<i64>,
) {
    let _ = app.emit(
        "agent-event",
        AgentEvent {
            project_id,
            kind,
            text: text.into(),
            item_id,
            request_id,
        },
    );
}

pub(crate) fn codex_executable() -> Result<PathBuf, String> {
    let binary = if cfg!(windows) { "codex.exe" } else { "codex" };
    if let Some(path) = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(binary))
        .find(|path| path.is_file())
    {
        return Ok(path);
    }
    if cfg!(windows) {
        for roaming in [
            std::env::var_os("APPDATA").map(PathBuf::from),
            std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join("AppData/Roaming")),
        ]
        .into_iter()
        .flatten()
        {
            let path = roaming.join("npm/node_modules/@openai/codex/node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc/bin/codex.exe");
            if path.is_file() {
                return Ok(path);
            }
        }
    }
    Err("Codex CLI executable not found. Install Codex before chatting.".into())
}

async fn write_message(writer: &Mutex<ChildStdin>, message: &Value) -> Result<(), String> {
    let mut data = serde_json::to_vec(message).map_err(|error| error.to_string())?;
    data.push(b'\n');
    writer
        .lock()
        .await
        .write_all(&data)
        .await
        .map_err(|error| error.to_string())
}

impl Session {
    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id, sender);
        if let Err(error) = write_message(
            &self.writer,
            &json!({"id": id, "method": method, "params": params}),
        )
        .await
        {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        let response = match timeout(Duration::from_secs(30), receiver).await {
            Ok(Ok(message)) => message,
            Ok(Err(_)) => return Err("Agent process disconnected".into()),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                return Err(format!("Agent did not respond to {method}"));
            }
        };
        if let Some(error) = response.get("error") {
            return Err(error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Agent request failed")
                .to_owned());
        }
        response
            .get("result")
            .cloned()
            .ok_or("Agent returned no result".into())
    }
}

fn item_id(params: &Value) -> Option<String> {
    params
        .get("itemId")
        .or_else(|| params.pointer("/item/id"))?
        .as_str()
        .map(str::to_owned)
}

async fn read_events(
    app: AppHandle,
    project_id: i64,
    stdout: tokio::process::ChildStdout,
    writer: Arc<Mutex<ChildStdin>>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
    approvals: Arc<Mutex<HashSet<i64>>>,
    busy: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    turn_id: Arc<Mutex<Option<String>>>,
    snapshot: Arc<Mutex<Option<FileSnapshot>>>,
    response: Arc<Mutex<Option<String>>>,
    root: PathBuf,
) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if message.get("method").is_none() {
            let Some(id) = message.get("id").and_then(Value::as_u64) else {
                continue;
            };
            if let Some(sender) = pending.lock().await.remove(&id) {
                let _ = sender.send(message);
            }
            continue;
        }
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            continue;
        };
        let params = &message["params"];
        if let Some(id) = message.get("id").and_then(Value::as_i64) {
            // Requests from Codex need an explicit answer; never approve unknown operations.
            if matches!(
                method,
                "item/commandExecution/requestApproval" | "item/fileChange/requestApproval"
            ) {
                approvals.lock().await.insert(id);
                let detail = params
                    .get("command")
                    .and_then(Value::as_str)
                    .or_else(|| params.get("reason").and_then(Value::as_str))
                    .or_else(|| params.get("grantRoot").and_then(Value::as_str))
                    .unwrap_or(method);
                emit(
                    &app,
                    project_id,
                    "approval",
                    detail,
                    item_id(params),
                    Some(id),
                );
            } else {
                let result = match method {
                    "item/permissions/requestApproval" => json!({"permissions": {}}),
                    "mcpServer/elicitation/request" => json!({"action":"decline", "content": null}),
                    _ => json!({"decision":"decline"}),
                };
                let _ = write_message(&writer, &json!({"id":id,"result":result})).await;
                emit(
                    &app,
                    project_id,
                    "status",
                    format!("Unsupported agent request declined: {method}"),
                    None,
                    None,
                );
            }
            continue;
        }
        match method {
            "turn/started" => {
                *turn_id.lock().await = params
                    .pointer("/turn/id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                emit(&app, project_id, "status", "Agent is working…", None, None);
            }
            "turn/completed" => {
                *turn_id.lock().await = None;
                approvals.lock().await.clear();
                if let Some(before) = snapshot.lock().await.take() {
                    let after = tokio::task::spawn_blocking({
                        let root = root.clone();
                        move || file_snapshot(&root)
                    })
                    .await;
                    if let Ok(after) = after {
                        let files = changed_files(&before, &after);
                        if !files.is_empty() {
                            emit(
                                &app,
                                project_id,
                                "changed",
                                json!(files).to_string(),
                                None,
                                None,
                            );
                        }
                    }
                }
                busy.store(false, Ordering::Release);
                let status = params
                    .pointer("/turn/status")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let error = params
                    .pointer("/turn/error/message")
                    .and_then(Value::as_str);
                emit(
                    &app,
                    project_id,
                    "done",
                    error.unwrap_or(status),
                    None,
                    None,
                );
            }
            "item/agentMessage/delta" => {
                if let Some(delta) = params.get("delta").and_then(Value::as_str) {
                    emit(&app, project_id, "delta", delta, item_id(params), None);
                }
            }
            "item/completed" => {
                let item = &params["item"];
                match item.get("type").and_then(Value::as_str) {
                    Some("agentMessage") => {
                        if let Some(text) = item.get("text").and_then(Value::as_str) {
                            *response.lock().await = Some(text.to_owned());
                            emit(&app, project_id, "message", text, item_id(params), None);
                        }
                    }
                    Some("commandExecution") => {
                        let command = item
                            .get("command")
                            .and_then(Value::as_str)
                            .unwrap_or("Command");
                        let status = item
                            .get("status")
                            .and_then(Value::as_str)
                            .unwrap_or("finished");
                        emit(
                            &app,
                            project_id,
                            "status",
                            format!("{command}: {status}"),
                            None,
                            None,
                        );
                    }
                    Some("fileChange") => {
                        if let Some(changes) = item.get("changes").and_then(Value::as_array) {
                            let files = changes
                                .iter()
                                .filter_map(|change| change.get("path").and_then(Value::as_str))
                                .collect::<Vec<_>>();
                            emit(&app, project_id, "files", files.join(" · "), None, None);
                        }
                    }
                    _ => {}
                }
            }
            "item/started" => {
                if params.pointer("/item/type").and_then(Value::as_str) == Some("commandExecution")
                {
                    let command = params
                        .pointer("/item/command")
                        .and_then(Value::as_str)
                        .unwrap_or("Running command");
                    emit(&app, project_id, "status", command, None, None);
                }
            }
            "error" => {
                if let Some(error) = params.pointer("/error/message").and_then(Value::as_str) {
                    emit(&app, project_id, "status", error, None, None);
                }
            }
            _ => {}
        }
    }
    busy.store(false, Ordering::Release);
    alive.store(false, Ordering::Release);
    emit(
        &app,
        project_id,
        "disconnected",
        "Agent process disconnected",
        None,
        None,
    );
}

async fn start_session(
    app: &AppHandle,
    project_id: i64,
    root: &str,
    mode: AgentMode,
) -> Result<Session, String> {
    let mut child = Command::new(codex_executable()?)
        .arg("app-server")
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("Could not start Codex: {error}"))?;
    let writer = Arc::new(Mutex::new(
        child.stdin.take().ok_or("Agent stdin unavailable")?,
    ));
    let stdout = child.stdout.take().ok_or("Agent stdout unavailable")?;
    let pending = Arc::new(Mutex::new(HashMap::new()));
    let approvals = Arc::new(Mutex::new(HashSet::new()));
    let busy = Arc::new(AtomicBool::new(false));
    let alive = Arc::new(AtomicBool::new(true));
    let turn_id = Arc::new(Mutex::new(None));
    let snapshot = Arc::new(Mutex::new(None));
    let response = Arc::new(Mutex::new(None));
    tokio::spawn(read_events(
        app.clone(),
        project_id,
        stdout,
        writer.clone(),
        pending.clone(),
        approvals.clone(),
        busy.clone(),
        alive.clone(),
        turn_id.clone(),
        snapshot.clone(),
        response.clone(),
        PathBuf::from(root),
    ));
    let mut session = Session {
        project_id,
        mode,
        thread_id: String::new(),
        _child: child,
        writer,
        pending,
        approvals,
        next_id: AtomicU64::new(1),
        busy,
        alive,
        turn_id,
        snapshot,
        response,
    };
    session
        .request(
            "initialize",
            json!({"clientInfo":{"name":"heim","title":"Heim","version":"0.1.0"}}),
        )
        .await?;
    write_message(
        &session.writer,
        &json!({"method":"initialized","params":{}}),
    )
    .await?;
    let mut thread_params = mode.thread_policy();
    thread_params["cwd"] = json!(root);
    let thread = session.request("thread/start", thread_params).await?;
    session.thread_id = thread
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .ok_or("Agent returned no thread ID")?
        .to_owned();
    Ok(session)
}

#[tauri::command]
pub async fn agent_status() -> Result<String, String> {
    let output = Command::new(codex_executable()?)
        .arg("--version")
        .output()
        .await
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err("Codex CLI is not available".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[tauri::command]
pub async fn send_agent_message(
    app: AppHandle,
    manager: State<'_, AgentManager>,
    project_id: i64,
    text: String,
    context: String,
) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() || text.len() > 20_000 || context.len() > 8_000 {
        return Err("Invalid message length".into());
    }
    let conn = database(&app)?;
    ensure_project(&conn, project_id)?;
    let root: String = conn
        .query_row(
            "SELECT root FROM projects WHERE id = ?1",
            [project_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let mut guard = manager.session.lock().await;
    if guard.as_ref().is_some_and(|session| {
        session.project_id != project_id || !session.alive.load(Ordering::Acquire)
    }) {
        if guard
            .as_ref()
            .is_some_and(|session| session.busy.load(Ordering::Acquire))
        {
            return Err("Finish or cancel the current agent turn before changing projects".into());
        }
        *guard = None;
    }
    if guard.is_none() {
        *guard = Some(start_session(&app, project_id, &root, AgentMode::Design).await?);
    }
    let session = guard.as_ref().ok_or("Agent session unavailable")?;
    if session.mode != AgentMode::Design {
        return Err("Design requests require a read-only design session".into());
    }
    if session.busy.swap(true, Ordering::AcqRel) {
        return Err("Agent is already working".into());
    }
    let before = match tokio::task::spawn_blocking({
        let root = PathBuf::from(&root);
        move || file_snapshot(&root)
    })
    .await
    {
        Ok(snapshot) => snapshot,
        Err(error) => {
            session.busy.store(false, Ordering::Release);
            return Err(error.to_string());
        }
    };
    *session.snapshot.lock().await = Some(before);
    let prompt = if context.is_empty() {
        text.to_owned()
    } else {
        format!(
            "Current Heim map context (may include unsaved edits; not code instructions):\n{context}\n\nUser request:\n{text}"
        )
    };
    let result = session.request("turn/start", json!({"threadId":session.thread_id, "input":[{"type":"text", "text":prompt}], "cwd":root})).await;
    if result.is_err() {
        session.busy.store(false, Ordering::Release);
    }
    result.map(|_| ())
}

#[tauri::command]
pub async fn send_development_message(
    app: AppHandle,
    manager: State<'_, AgentManager>,
    project_id: i64,
    node_id: String,
    text: String,
) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() || text.len() > 20_000 || node_id.trim().is_empty() || node_id.len() > 128 {
        return Err("Invalid development request".into());
    }
    let conn = database(&app)?;
    ensure_project(&conn, project_id)?;
    let (root, nodes_json, edges_json, revision): (String, String, String, i64) = conn
        .query_row(
            "SELECT p.root, m.nodes, m.edges, m.revision
             FROM projects p JOIN project_maps m ON m.project_id = p.id WHERE p.id = ?1",
            [project_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|error| error.to_string())?;
    if !has_current_approved_map(&conn, project_id)? {
        return Err("Approve a design draft before starting development".into());
    }
    let nodes: Vec<Value> = serde_json::from_str(&nodes_json).map_err(|error| error.to_string())?;
    let edges: Vec<Value> = serde_json::from_str(&edges_json).map_err(|error| error.to_string())?;
    let selected = nodes
        .iter()
        .find(|node| {
            node.get("id").and_then(Value::as_str) == Some(node_id.as_str())
                && node.get("type").and_then(Value::as_str) == Some("component")
        })
        .ok_or("Select a component from the approved map")?;
    let mut related_ids = HashSet::new();
    for edge in &edges {
        let source = edge.get("source").and_then(Value::as_str);
        let target = edge.get("target").and_then(Value::as_str);
        if source == Some(node_id.as_str()) {
            if let Some(target) = target {
                related_ids.insert(target.to_owned());
            }
        } else if target == Some(node_id.as_str()) {
            if let Some(source) = source {
                related_ids.insert(source.to_owned());
            }
        }
    }
    let related = nodes
        .iter()
        .filter(|node| {
            node.get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| related_ids.contains(id))
        })
        .collect::<Vec<_>>();
    let prompt = format!(
        "The user approved Heim map revision {revision} and explicitly requested development for exactly this selected unit. Implement only the selected component; do not implement other map nodes. Respect the existing Codex workspace-write sandbox and approval policy.\n\nSelected component:\n{}\n\nDirectly related context (context only, not scope expansion):\n{}\n\nUser development request:\n{text}",
        serde_json::to_string_pretty(selected).map_err(|error| error.to_string())?,
        serde_json::to_string_pretty(&related).map_err(|error| error.to_string())?,
    );
    let mut guard = manager.session.lock().await;
    if guard.as_ref().is_some_and(|session| {
        session.project_id != project_id
            || session.mode != AgentMode::Development
            || !session.alive.load(Ordering::Acquire)
    }) {
        if guard
            .as_ref()
            .is_some_and(|session| session.busy.load(Ordering::Acquire))
        {
            return Err(
                "Finish or cancel the current agent turn before starting development".into(),
            );
        }
        *guard = None;
    }
    if guard.is_none() {
        *guard = Some(start_session(&app, project_id, &root, AgentMode::Development).await?);
    }
    let session = guard.as_ref().ok_or("Agent session unavailable")?;
    if session.busy.swap(true, Ordering::AcqRel) {
        return Err("Agent is already working".into());
    }
    let before = match tokio::task::spawn_blocking({
        let root = PathBuf::from(&root);
        move || file_snapshot(&root)
    })
    .await
    {
        Ok(snapshot) => snapshot,
        Err(error) => {
            session.busy.store(false, Ordering::Release);
            return Err(error.to_string());
        }
    };
    *session.snapshot.lock().await = Some(before);
    let result = session.request(
        "turn/start",
        json!({"threadId":session.thread_id, "input":[{"type":"text", "text":prompt}], "cwd":root}),
    ).await;
    if result.is_err() {
        session.busy.store(false, Ordering::Release);
    }
    result.map(|_| ())
}

pub(crate) async fn request_design_draft(
    app: AppHandle,
    manager: &AgentManager,
    project_id: i64,
    text: &str,
    latest_draft: Option<&str>,
) -> Result<DesignResponse, String> {
    let text = text.trim();
    if text.is_empty() || text.len() > 20_000 {
        return Err("Invalid design request length".into());
    }
    let conn = database(&app)?;
    ensure_project(&conn, project_id)?;
    let root: String = conn
        .query_row(
            "SELECT root FROM projects WHERE id = ?1",
            [project_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let prompt = match latest_draft {
        Some(draft) => format!(
            "Revise the latest saved design draft only. Do not implement or create/edit files. Treat the saved draft below as the current user-edited source of truth; preserve stable IDs for unchanged nodes and apply the user's requested changes. Return exactly one JSON object without markdown using this schema: {{\"goal\":string,\"nodes\":[{{\"id\":string,\"title\":string,\"summary\":string,\"type\":\"component\",\"status\":\"not_started\"}}],\"edges\":[{{\"id\":string,\"source\":string,\"target\":string,\"label\":string}}],\"questions\":[string],\"assumptions\":[string]}}. Use unique stable IDs, valid edge endpoints, and status not_started for every node.\n\nUser change request:\n{text}\n\nLatest saved user-edited design draft:\n{draft}"
        ),
        None => format!(
            "Design a project map only. Do not implement or create/edit files. Return exactly one JSON object without markdown using this schema: {{\"goal\":string,\"nodes\":[{{\"id\":string,\"title\":string,\"summary\":string,\"type\":\"component\",\"status\":\"not_started\"}}],\"edges\":[{{\"id\":string,\"source\":string,\"target\":string,\"label\":string}}],\"questions\":[string],\"assumptions\":[string]}}. Use unique stable IDs, valid edge endpoints, and status not_started for every node.\n\nProduct description:\n{text}"
        ),
    };
    let (busy, response) = {
        let mut guard = manager.session.lock().await;
        if guard.as_ref().is_some_and(|session| {
            session.project_id != project_id
                || session.mode != AgentMode::Design
                || !session.alive.load(Ordering::Acquire)
        }) {
            if guard
                .as_ref()
                .is_some_and(|session| session.busy.load(Ordering::Acquire))
            {
                return Err(
                    "Finish or cancel the current agent turn before changing projects".into(),
                );
            }
            *guard = None;
        }
        if guard.is_none() {
            *guard = Some(start_session(&app, project_id, &root, AgentMode::Design).await?);
        }
        let session = guard.as_ref().ok_or("Agent session unavailable")?;
        if session.busy.swap(true, Ordering::AcqRel) {
            return Err("Agent is already working".into());
        }
        *session.response.lock().await = None;
        *session.snapshot.lock().await = None;
        if let Err(error) = session
            .request(
                "turn/start",
                json!({"threadId":session.thread_id, "input":[{"type":"text", "text":prompt}], "cwd":root}),
            )
            .await
        {
            session.busy.store(false, Ordering::Release);
            return Err(error);
        }
        (session.busy.clone(), session.response.clone())
    };
    for _ in 0..300 {
        if !busy.load(Ordering::Acquire) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if busy.load(Ordering::Acquire) {
        return Err("Design response timed out; cancel the active turn before retrying".into());
    }
    let text = response
        .lock()
        .await
        .clone()
        .ok_or("Agent returned no design response")?;
    let revision: i64 = conn
        .query_row(
            "SELECT revision FROM project_maps WHERE project_id = ?1",
            [project_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    Ok(DesignResponse { text, revision })
}

#[tauri::command]
pub async fn cancel_agent_turn(
    manager: State<'_, AgentManager>,
    project_id: i64,
) -> Result<(), String> {
    let guard = manager.session.lock().await;
    let session = guard
        .as_ref()
        .filter(|session| session.project_id == project_id)
        .ok_or("No agent session")?;
    let mut turn_id = session.turn_id.lock().await.clone();
    for _ in 0..20 {
        if turn_id.is_some() || !session.busy.load(Ordering::Acquire) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        turn_id = session.turn_id.lock().await.clone();
    }
    let turn_id = turn_id.ok_or("No active turn")?;
    session
        .request(
            "turn/interrupt",
            json!({"threadId":session.thread_id,"turnId":turn_id}),
        )
        .await?;
    Ok(())
}

#[tauri::command]
pub async fn answer_agent_approval(
    manager: State<'_, AgentManager>,
    project_id: i64,
    request_id: i64,
    decision: String,
) -> Result<(), String> {
    let guard = manager.session.lock().await;
    let session = guard
        .as_ref()
        .filter(|session| session.project_id == project_id)
        .ok_or("No agent session")?;
    if !session.mode.permits_approval(&decision) {
        return Err("Approvals are disabled in read-only design mode".into());
    }
    if !session.approvals.lock().await.remove(&request_id) {
        return Err("Approval request is not pending".into());
    }
    write_message(
        &session.writer,
        &json!({"id":request_id,"result":{"decision":decision}}),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_diff_includes_created_modified_and_removed_files() {
        let before = HashMap::from([
            ("updated.txt".into(), (1, None)),
            ("removed.txt".into(), (2, None)),
            ("same.txt".into(), (3, None)),
        ]);
        let after = HashMap::from([
            ("updated.txt".into(), (4, None)),
            ("created.txt".into(), (1, None)),
            ("same.txt".into(), (3, None)),
        ]);
        assert_eq!(
            changed_files(&before, &after),
            ["created.txt", "removed.txt", "updated.txt"]
        );
    }

    #[test]
    fn design_policy_is_fixed_to_read_only_without_approval_escalation() {
        let policy = AgentMode::Design.thread_policy();
        assert_eq!(policy["sandbox"], "read-only");
        assert_eq!(policy["approvalPolicy"], "never");
        assert!(!AgentMode::Design.permits_approval("accept"));
        assert!(!AgentMode::Design.permits_approval("decline"));
    }

    #[test]
    fn development_policy_allows_only_explicit_approval_responses() {
        assert_eq!(
            AgentMode::Development.thread_policy()["sandbox"],
            "workspace-write"
        );
        assert_eq!(
            AgentMode::Development.thread_policy()["approvalPolicy"],
            "on-request"
        );
        assert!(AgentMode::Development.permits_approval("accept"));
        assert!(AgentMode::Development.permits_approval("decline"));
        assert!(!AgentMode::Development.permits_approval("other"));
    }
}
