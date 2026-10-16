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

use crate::{database, ensure_project, record_development_unit, update_development_unit_status};

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

fn command_is_safe(command: &str) -> bool {
    let command = command.to_ascii_lowercase();
    let risky = [
        "rm ",
        "rm -",
        "del ",
        "erase ",
        "rmdir ",
        "format ",
        "git reset",
        "git clean",
        "git push",
        "git commit",
        "curl ",
        "wget ",
        "invoke-webrequest",
        "powershell",
        "cmd /c",
        "ssh ",
        "sudo ",
        "chmod ",
        "--force",
        ".env",
        "credential",
    ];
    if risky.iter().any(|token| command.contains(token)) {
        return false;
    }
    [
        "npm test",
        "npm run build",
        "npm run lint",
        "node --test",
        "cargo test",
        "cargo check",
        "cargo fmt",
        "git status",
        "git diff",
        "git log",
        "dir",
        "ls",
        "type ",
        "cat ",
    ]
    .iter()
    .any(|token| command.starts_with(token))
}

fn file_change_is_safe(params: &Value, root: &Path) -> bool {
    let Some(changes) = params.get("changes").and_then(Value::as_array) else {
        return false;
    };
    !changes.is_empty()
        && changes.iter().all(|change| {
            let Some(path) = change.get("path").and_then(Value::as_str) else {
                return false;
            };
            let candidate = root.join(path);
            let normalized = path.to_ascii_lowercase();
            !Path::new(path).is_absolute()
                && !normalized.contains(".env")
                && !normalized.contains("credential")
                && candidate.starts_with(root)
        })
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
                let detail = params
                    .get("command")
                    .and_then(Value::as_str)
                    .or_else(|| params.get("reason").and_then(Value::as_str))
                    .or_else(|| params.get("grantRoot").and_then(Value::as_str))
                    .unwrap_or(method);
                let auto_approve = match method {
                    "item/commandExecution/requestApproval" => command_is_safe(detail),
                    "item/fileChange/requestApproval" => file_change_is_safe(params, &root),
                    _ => false,
                };
                if auto_approve {
                    let _ =
                        write_message(&writer, &json!({"id":id,"result":{"decision":"accept"}}))
                            .await;
                    emit(
                        &app,
                        project_id,
                        "status",
                        format!("Auto-approved safe agent request: {detail}"),
                        item_id(params),
                        None,
                    );
                    continue;
                }
                approvals.lock().await.insert(id);
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
    map_revision: i64,
    node_id: String,
    goal: String,
    completion_criteria: Vec<String>,
) -> Result<i64, String> {
    let mut conn = database(&app)?;
    ensure_project(&conn, project_id)?;
    let root: String = conn
        .query_row(
            "SELECT root FROM projects WHERE id = ?1",
            [project_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let mut guard = manager.session.lock().await;
    if guard
        .as_ref()
        .is_some_and(|session| session.busy.load(Ordering::Acquire))
    {
        return Err("Finish or cancel the current agent turn before starting development".into());
    }
    let unit = record_development_unit(
        &mut conn,
        project_id,
        map_revision,
        &node_id,
        &goal,
        &completion_criteria,
    )?;
    let criteria = unit
        .completion_criteria
        .iter()
        .map(|criterion| format!("- {criterion}"))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = format!(
        "The user explicitly confirmed development unit #{id} against approved Heim map revision {revision}. Implement only the unit goal and its completion criteria, within the selected architecture responsibility. Do not interpret the full node responsibility as additional scope, choose another unit, or continue after this unit. Implement product behavior, not merely tests, fixtures, documentation, or a test harness. Tests may support verification after implementation. Before reporting, assess every completion criterion and state which are met or unmet. Respect the existing Codex workspace-write sandbox and approval policy.\n\nSelected architecture responsibility:\n{selected}\n\nDirectly related context (context only, not scope expansion):\n{related}\n\nConfirmed unit goal:\n{goal}\n\nCompletion criteria:\n{criteria}",
        id = unit.id,
        revision = unit.map_revision,
        selected =
            serde_json::to_string_pretty(&unit.selected_node).map_err(|error| error.to_string())?,
        related =
            serde_json::to_string_pretty(&unit.related_nodes).map_err(|error| error.to_string())?,
        goal = unit.goal,
    );
    *guard = None;
    let session = match start_session(&app, project_id, &root, AgentMode::Development).await {
        Ok(session) => session,
        Err(error) => {
            update_development_unit_status(&conn, unit.id, "start_failed", Some(&error))?;
            return Err(error);
        }
    };
    *guard = Some(session);
    let session = guard.as_ref().ok_or("Agent session unavailable")?;
    if session.busy.swap(true, Ordering::AcqRel) {
        update_development_unit_status(
            &conn,
            unit.id,
            "start_failed",
            Some("Agent is already working"),
        )?;
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
            update_development_unit_status(
                &conn,
                unit.id,
                "start_failed",
                Some(&error.to_string()),
            )?;
            return Err(error.to_string());
        }
    };
    *session.snapshot.lock().await = Some(before);
    if let Err(error) = update_development_unit_status(&conn, unit.id, "started", None) {
        session.busy.store(false, Ordering::Release);
        return Err(error);
    }
    let result = session.request(
        "turn/start",
        json!({"threadId":session.thread_id, "input":[{"type":"text", "text":prompt}], "cwd":root}),
    ).await;
    if let Err(error) = result {
        session.busy.store(false, Ordering::Release);
        update_development_unit_status(&conn, unit.id, "start_failed", Some(&error))?;
        return Err(error);
    }
    Ok(unit.id)
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
            "Revise the latest saved design draft only. Do not implement or create/edit files. Treat the saved draft below as the current user-edited source of truth; preserve stable IDs for unchanged nodes and apply the user's requested changes. Return exactly one JSON object without markdown using this schema: {{\"goal\":string,\"nodes\":[{{\"id\":string,\"title\":string,\"summary\":string explaining the node's responsibility and boundaries,\"responsibilities\":[string],\"type\":\"component\",\"status\":\"not_started\",\"questions\":[string]}}],\"edges\":[{{\"id\":string,\"source\":string,\"target\":string,\"relation\":\"depends_on\"|\"interacts_with\"}}],\"questions\":[string],\"assumptions\":[string]}}. For depends_on, source requires target; for interacts_with, source interacts with target. Never use an untyped or generic related edge. Put unresolved decisions next to the relevant node in its questions array; use the top-level questions array only for project-wide questions. Use unique stable IDs, valid edge endpoints, at least one responsibility per node, and status not_started for every node.\n\nUser change request:\n{text}\n\nLatest saved user-edited design draft:\n{draft}"
        ),
        None => format!(
            "Design a logical architecture map only. Do not implement or create/edit files. Nodes are persistent system responsibilities, not files or one-off tasks. Return exactly one JSON object without markdown using this schema: {{\"goal\":string,\"nodes\":[{{\"id\":string,\"title\":string,\"summary\":string explaining the node's responsibility and boundaries,\"responsibilities\":[string],\"type\":\"component\",\"status\":\"not_started\",\"questions\":[string]}}],\"edges\":[{{\"id\":string,\"source\":string,\"target\":string,\"relation\":\"depends_on\"|\"interacts_with\"}}],\"questions\":[string],\"assumptions\":[string]}}. For depends_on, source requires target; for interacts_with, source interacts with target. Never use an untyped or generic related edge. Put unresolved decisions next to the relevant node in its questions array; use the top-level questions array only for project-wide questions. Use unique stable IDs, valid edge endpoints, at least one responsibility per node, and status not_started for every node.\n\nProduct description:\n{text}"
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
pub async fn send_design_message(
    app: AppHandle,
    manager: State<'_, AgentManager>,
    project_id: i64,
    text: String,
    map_context: String,
) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() || text.len() > 20_000 || map_context.len() > 200_000 {
        return Err("Invalid design message".into());
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
    let prompt = format!(
        "Discuss project design only. Do not create, edit, or delete files and do not run development work. Treat the confirmed Heim map below as context. Explain design implications or propose map changes in conversation; the user must explicitly save or approve map changes in Heim.\n\nConfirmed map:\n{map_context}\n\nUser design request:\n{text}"
    );
    let mut guard = manager.session.lock().await;
    if guard
        .as_ref()
        .is_some_and(|session| session.busy.load(Ordering::Acquire))
    {
        return Err(
            "Finish or cancel the current agent turn before starting a design message".into(),
        );
    }
    *guard = None;
    *guard = Some(start_session(&app, project_id, &root, AgentMode::Design).await?);
    let session = guard.as_ref().ok_or("Agent session unavailable")?;
    if session.busy.swap(true, Ordering::AcqRel) {
        return Err("Agent is already working".into());
    }
    session
        .request(
            "turn/start",
            json!({"threadId":session.thread_id,"input":[{"type":"text","text":prompt}],"cwd":root}),
        )
        .await
        .map(|_| ())
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
