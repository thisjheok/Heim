use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
    },
    time::SystemTime,
};

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{Value, json};

use tauri::{AppHandle, Emitter, State};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{Mutex, oneshot},
    time::{Duration, timeout},
};

use crate::{
    DevelopmentUnitSpec, create_development_unit_run, database, ensure_project,
    has_current_approved_map, load_development_unit_spec, node_has_unanswered_questions,
    record_development_unit, record_development_unit_run_evidence,
    save_development_unit_run_report, store_generated_development_unit_spec,
    transition_development_unit_run, update_development_unit_status,
    validate_development_unit_spec, validate_map,
};

#[derive(Default)]
pub struct AgentManager {
    session: Mutex<Option<Session>>,
    suppress_disconnect_events: bool,
}

pub(crate) struct DesignResponse {
    pub text: String,
    pub revision: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevelopmentStartResult {
    unit_id: i64,
    run_id: i64,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NextDevelopmentRecommendation {
    node_id: String,
    rationale: String,
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
    expected_response: Arc<Mutex<Option<ResponseExpectation>>>,
    active_run_id: Arc<AtomicI64>,
}

#[derive(Clone, Debug)]
struct ResponseExpectation {
    mode: AgentMode,
    map_revision: i64,
    draft_id: Option<i64>,
    draft_version: Option<i64>,
    unit_id: Option<i64>,
    node_id: Option<String>,
    criteria_count: usize,
    result_kind: &'static str,
}

fn is_internal_design_result(kind: &str) -> bool {
    matches!(kind, "unit_plan" | "next_development_node")
}

fn validate_agent_response(
    text: &str,
    expected: &ResponseExpectation,
) -> Result<(Value, String), String> {
    if text.len() > 200_000 {
        return Err("Agent response exceeds the 200000-byte limit".into());
    }
    let envelope: Value =
        serde_json::from_str(text).map_err(|_| "Agent response is not valid contract JSON")?;
    if envelope["contractVersion"] != 1
        || envelope["mode"].as_str()
            != Some(match expected.mode {
                AgentMode::Design => "design",
                AgentMode::Development => "development",
            })
    {
        return Err("Agent response has the wrong contract version or mode".into());
    }
    let context = &envelope["context"];
    if context.get("mapRevision").and_then(Value::as_i64) != Some(expected.map_revision)
        || context.get("draftId").is_none()
        || context.get("draftVersion").is_none()
        || context.get("unitId").is_none()
        || context.get("draftId").and_then(Value::as_i64) != expected.draft_id
        || context.get("draftVersion").and_then(Value::as_i64) != expected.draft_version
        || context.get("unitId").and_then(Value::as_i64) != expected.unit_id
    {
        return Err("Agent response context does not match the request snapshot".into());
    }
    let result = &envelope["result"];
    if result["kind"].as_str() != Some(expected.result_kind) {
        return Err("Agent response has an unexpected result kind".into());
    }
    let display = match expected.result_kind {
        "discussion" => result["message"]
            .as_str()
            .filter(|message| !message.trim().is_empty() && message.len() <= 20_000)
            .ok_or("Agent response message is missing or too long")
            .and_then(|message| {
                for field in ["questions", "assumptions"] {
                    if !result[field]
                        .as_array()
                        .is_some_and(|items| items.iter().all(Value::is_string))
                    {
                        return Err(
                            "Design discussion questions and assumptions must be string arrays"
                                .into(),
                        );
                    }
                }
                Ok(message.to_owned())
            })?,
        "map_draft" => {
            let message = result["message"]
                .as_str()
                .filter(|message| !message.trim().is_empty() && message.len() <= 20_000)
                .ok_or("Agent response message is missing or too long")?;
            if !result["draft"]["goal"].is_string()
                || !result["draft"]["nodes"].is_array()
                || !result["draft"]["edges"].is_array()
                || !result["draft"]["questions"].is_array()
                || !result["draft"]["assumptions"].is_array()
            {
                return Err("Structured map draft payload is incomplete".into());
            }
            message.to_owned()
        }
        "unit_plan" => {
            if result["nodeId"].as_str() != expected.node_id.as_deref() {
                return Err("AI unit plan refers to a different map node".into());
            }
            let goal = result["goal"]
                .as_str()
                .filter(|goal| !goal.trim().is_empty() && goal.len() <= 4_000)
                .ok_or("AI unit plan goal is missing or too long")?;
            let criteria = result["completionCriteria"]
                .as_array()
                .filter(|criteria| !criteria.is_empty() && criteria.len() <= 20)
                .ok_or("AI unit plan must include 1-20 completion criteria")?;
            if criteria.iter().any(|criterion| {
                criterion.as_str().map_or(true, |criterion| {
                    criterion.trim().is_empty() || criterion.len() > 1_000
                })
            }) {
                return Err("AI unit plan contains an invalid completion criterion".into());
            }
            goal.to_owned()
        }
        "next_development_node" => {
            let node_id = result["nodeId"]
                .as_str()
                .filter(|id| !id.is_empty() && id.len() <= 128)
                .ok_or("AI recommendation node ID is missing or invalid")?;
            let rationale = result["rationale"]
                .as_str()
                .filter(|text| !text.trim().is_empty() && text.len() <= 1_000)
                .ok_or("AI recommendation rationale is missing or too long")?;
            if rationale != rationale.trim() {
                return Err(
                    "AI recommendation rationale must not contain surrounding whitespace".into(),
                );
            }
            node_id.to_owned()
        }
        "development_report" => {
            let summary = result["summary"]
                .as_str()
                .filter(|summary| !summary.trim().is_empty() && summary.len() <= 20_000)
                .ok_or("Development report summary is missing or too long")?;
            if !matches!(
                result["outcome"].as_str(),
                Some("completed" | "incomplete" | "blocked")
            ) {
                return Err("Development report has an invalid outcome".into());
            }
            let criteria = result["criteria"]
                .as_array()
                .ok_or("Development report criteria are missing")?;
            if criteria.len() != expected.criteria_count
                || criteria.iter().enumerate().any(|(index, item)| {
                    item["index"].as_u64() != Some(index as u64)
                        || !matches!(item["status"].as_str(), Some("met" | "not_met" | "not_run"))
                        || !item["evidence"].as_array().is_some_and(|evidence| {
                            evidence.iter().all(|entry| entry.as_str().is_some())
                        })
                })
            {
                return Err(
                    "Development report does not assess each requested criterion exactly once"
                        .into(),
                );
            }
            let changes = result["changes"]
                .as_array()
                .ok_or("Development report changes are missing")?;
            let checks = result["checks"]
                .as_array()
                .ok_or("Development report checks are missing")?;
            let blockers = result["blockers"]
                .as_array()
                .ok_or("Development report blockers are missing")?;
            if changes.iter().any(|change| {
                change["path"].as_str().map_or(true, str::is_empty)
                    || change["summary"].as_str().map_or(true, str::is_empty)
            }) || checks.iter().any(|check| {
                check["command"].as_str().map_or(true, str::is_empty)
                    || check["summary"].as_str().map_or(true, str::is_empty)
                    || !matches!(
                        check["status"].as_str(),
                        Some("passed" | "failed" | "not_run")
                    )
            }) || blockers.iter().any(|blocker| blocker.as_str().is_none())
            {
                return Err(
                    "Development report contains an invalid change, check, or blocker".into(),
                );
            }
            let outcome = result["outcome"].as_str().unwrap_or("unknown");
            let all_met = criteria
                .iter()
                .all(|criterion| criterion["status"] == "met");
            if (outcome == "completed") != all_met
                || (outcome == "blocked" && blockers.is_empty())
                || (outcome == "completed" && !blockers.is_empty())
            {
                return Err(
                    "Development report outcome conflicts with its criteria or blockers".into(),
                );
            }
            let criteria_text = criteria
                .iter()
                .map(|criterion| {
                    let evidence = criterion["evidence"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("; ");
                    format!(
                        "- Criterion {}: {}{}",
                        criterion["index"],
                        criterion["status"].as_str().unwrap_or("unknown"),
                        if evidence.is_empty() {
                            String::new()
                        } else {
                            format!(" — {evidence}")
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let changes_text = changes
                .iter()
                .map(|change| {
                    format!(
                        "- {} — {}",
                        change["path"].as_str().unwrap_or(""),
                        change["summary"].as_str().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let checks_text = checks
                .iter()
                .map(|check| {
                    format!(
                        "- {}: {} — {}",
                        check["status"].as_str().unwrap_or("unknown"),
                        check["command"].as_str().unwrap_or(""),
                        check["summary"].as_str().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let blockers_text = blockers
                .iter()
                .filter_map(Value::as_str)
                .map(|blocker| format!("- {blocker}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "Agent-reported status (not independently verified by Heim)\nOutcome: {outcome}\n{summary}\n\nCompletion criteria:\n{criteria_text}\n\nReported changes:\n{}\n\nChecks:\n{}\n\nBlockers:\n{}",
                if changes_text.is_empty() {
                    "- None reported"
                } else {
                    &changes_text
                },
                if checks_text.is_empty() {
                    "- None reported"
                } else {
                    &checks_text
                },
                if blockers_text.is_empty() {
                    "- None"
                } else {
                    &blockers_text
                }
            )
        }
        _ => return Err("Unsupported Agent response contract".into()),
    };
    Ok((result.clone(), display))
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
                    Some(
                        ".git"
                            | "node_modules"
                            | "target"
                            | "dist"
                            | "__pycache__"
                            | ".pytest_cache"
                            | ".mypy_cache"
                            | ".ruff_cache"
                            | ".tox"
                            | ".venv"
                            | "venv"
                            | "env"
                    )
                )
        })
        .flatten()
        .take(20_000)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        if entry
            .path()
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                matches!(extension.to_ascii_lowercase().as_str(), "pyc" | "pyo")
            })
        {
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

fn terminal_development_run_state<'a>(
    turn_status: &'a str,
    error: Option<&'a str>,
) -> (&'static str, Option<&'a str>) {
    if turn_status == "completed" && error.is_none() {
        ("finished", None)
    } else if matches!(turn_status, "interrupted" | "cancelled" | "canceled") {
        ("interrupted", error.or(Some(turn_status)))
    } else {
        ("failed", error.or(Some(turn_status)))
    }
}

fn command_for_persistent_evidence(command: &str) -> String {
    let lowered = command.to_ascii_lowercase();
    if [
        "--token",
        "--password",
        "--secret",
        "--api-key",
        "apikey=",
        "token=",
        "password=",
        "secret=",
        "authorization:",
        "bearer ",
    ]
    .iter()
    .any(|marker| lowered.contains(marker))
    {
        return "[redacted command containing a credential-like argument]".into();
    }
    command.chars().take(4_000).collect()
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
    expected_response: Arc<Mutex<Option<ResponseExpectation>>>,
    active_run_id: Arc<AtomicI64>,
    root: PathBuf,
    suppress_disconnect_event: bool,
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
                let run_id = active_run_id.load(Ordering::Acquire);
                if run_id > 0 {
                    match database(&app).and_then(|conn| {
                        transition_development_unit_run(&conn, run_id, "running", None).map(|_| ())
                    }) {
                        Ok(()) => {}
                        Err(error) => emit(
                            &app,
                            project_id,
                            "status",
                            format!("Could not record development run start: {error}"),
                            None,
                            None,
                        ),
                    }
                }
                let internal_plan = expected_response
                    .lock()
                    .await
                    .as_ref()
                    .is_some_and(|expected| is_internal_design_result(expected.result_kind));
                if !internal_plan {
                    emit(&app, project_id, "status", "Agent is working…", None, None);
                }
            }
            "turn/completed" => {
                *turn_id.lock().await = None;
                approvals.lock().await.clear();
                let run_id = active_run_id.swap(0, Ordering::AcqRel);
                let status = params
                    .pointer("/turn/status")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let error = params
                    .pointer("/turn/error/message")
                    .and_then(Value::as_str);
                if run_id > 0 {
                    let (run_state, detail) = terminal_development_run_state(status, error);
                    match database(&app).and_then(|conn| {
                        transition_development_unit_run(&conn, run_id, run_state, detail)
                            .map(|_| ())
                    }) {
                        Ok(()) => {}
                        Err(error) => emit(
                            &app,
                            project_id,
                            "status",
                            format!("Could not record development run outcome: {error}"),
                            None,
                            None,
                        ),
                    }
                }
                if let Some(before) = snapshot.lock().await.take() {
                    let after = tokio::task::spawn_blocking({
                        let root = root.clone();
                        move || file_snapshot(&root)
                    })
                    .await;
                    match after {
                        Ok(after) => {
                            let files = changed_files(&before, &after);
                            if run_id > 0 {
                                let evidence = json!({"available":true,"changedFiles":files});
                                if let Err(error) = database(&app).and_then(|conn| {
                                    record_development_unit_run_evidence(
                                        &conn,
                                        run_id,
                                        "workspace_snapshot",
                                        &evidence,
                                    )
                                }) {
                                    emit(
                                        &app,
                                        project_id,
                                        "status",
                                        format!("Could not persist workspace evidence: {error}"),
                                        None,
                                        None,
                                    );
                                }
                            }
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
                        Err(error) => {
                            if run_id > 0 {
                                let evidence = json!({"available":false,"error":error.to_string()});
                                if let Err(error) = database(&app).and_then(|conn| {
                                    record_development_unit_run_evidence(
                                        &conn,
                                        run_id,
                                        "workspace_snapshot",
                                        &evidence,
                                    )
                                }) {
                                    emit(
                                        &app,
                                        project_id,
                                        "status",
                                        format!(
                                            "Could not persist unavailable workspace evidence: {error}"
                                        ),
                                        None,
                                        None,
                                    );
                                }
                            }
                        }
                    }
                }
                busy.store(false, Ordering::Release);
                let internal_plan = expected_response
                    .lock()
                    .await
                    .as_ref()
                    .is_some_and(|expected| is_internal_design_result(expected.result_kind));
                if !internal_plan {
                    emit(
                        &app,
                        project_id,
                        "done",
                        error.unwrap_or(status),
                        None,
                        None,
                    );
                }
            }
            "item/agentMessage/delta" => {
                let internal_plan = expected_response
                    .lock()
                    .await
                    .as_ref()
                    .is_some_and(|expected| is_internal_design_result(expected.result_kind));
                if !internal_plan && let Some(delta) = params.get("delta").and_then(Value::as_str) {
                    emit(&app, project_id, "delta", delta, item_id(params), None);
                }
            }
            "item/completed" => {
                let item = &params["item"];
                match item.get("type").and_then(Value::as_str) {
                    Some("agentMessage") => {
                        if let Some(text) = item.get("text").and_then(Value::as_str) {
                            let expected = expected_response.lock().await.clone();
                            let Some(expected) = expected else {
                                emit(
                                    &app,
                                    project_id,
                                    "protocol_error",
                                    "No response contract is active for this Agent turn",
                                    item_id(params),
                                    None,
                                );
                                continue;
                            };
                            match validate_agent_response(text, &expected) {
                                Ok((result, display)) => {
                                    let persist = if expected.mode == AgentMode::Development {
                                        let run_id = active_run_id.load(Ordering::Acquire);
                                        if run_id <= 0 {
                                            Err("No active development run is associated with this report".into())
                                        } else {
                                            database(&app).and_then(|mut conn| {
                                                save_development_unit_run_report(
                                                    &mut conn, run_id, &result,
                                                )
                                            })
                                        }
                                    } else {
                                        Ok(())
                                    };
                                    match persist {
                                        Ok(()) => {
                                            *response.lock().await = Some(text.to_owned());
                                            if !is_internal_design_result(expected.result_kind) {
                                                emit(
                                                    &app,
                                                    project_id,
                                                    "message",
                                                    display,
                                                    item_id(params),
                                                    None,
                                                );
                                            }
                                        }
                                        Err(error) => emit(
                                            &app,
                                            project_id,
                                            "protocol_error",
                                            format!(
                                                "Validated report could not be persisted: {error}"
                                            ),
                                            item_id(params),
                                            None,
                                        ),
                                    }
                                }
                                Err(error) => {
                                    if !is_internal_design_result(expected.result_kind) {
                                        emit(
                                            &app,
                                            project_id,
                                            "protocol_error",
                                            error,
                                            item_id(params),
                                            None,
                                        );
                                    }
                                }
                            }
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
                            .unwrap_or("unknown");
                        let run_id = active_run_id.load(Ordering::Acquire);
                        if run_id > 0 {
                            let evidence = json!({
                                "command": command_for_persistent_evidence(command),
                                "status": status,
                                "exitCode": item.get("exitCode").and_then(Value::as_i64),
                                "durationMs": item.get("durationMs").and_then(Value::as_u64),
                                "itemId": item.get("id").and_then(Value::as_str),
                            });
                            if let Err(error) = database(&app).and_then(|conn| {
                                record_development_unit_run_evidence(
                                    &conn,
                                    run_id,
                                    "command_execution",
                                    &evidence,
                                )
                            }) {
                                emit(
                                    &app,
                                    project_id,
                                    "status",
                                    format!("Could not persist command evidence: {error}"),
                                    None,
                                    None,
                                );
                            }
                        }
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
    let run_id = active_run_id.swap(0, Ordering::AcqRel);
    if run_id > 0 {
        if let Err(error) = database(&app).and_then(|conn| {
            transition_development_unit_run(
                &conn,
                run_id,
                "disconnected",
                Some("Agent process disconnected before the turn completed"),
            )
            .map(|_| ())
        }) {
            emit(
                &app,
                project_id,
                "status",
                format!("Could not record disconnected development run: {error}"),
                None,
                None,
            );
        }
        if let Some(before) = snapshot.lock().await.take() {
            let after = tokio::task::spawn_blocking({
                let root = root.clone();
                move || file_snapshot(&root)
            })
            .await;
            match after {
                Ok(after) => {
                    let files = changed_files(&before, &after);
                    let evidence = json!({"available":true,"changedFiles":files});
                    if let Err(error) = database(&app).and_then(|conn| {
                        record_development_unit_run_evidence(
                            &conn,
                            run_id,
                            "workspace_snapshot",
                            &evidence,
                        )
                    }) {
                        emit(
                            &app,
                            project_id,
                            "status",
                            format!("Could not persist disconnected workspace evidence: {error}"),
                            None,
                            None,
                        );
                    }
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
                Err(error) => {
                    let evidence = json!({"available":false,"error":error.to_string()});
                    if let Err(error) = database(&app).and_then(|conn| {
                        record_development_unit_run_evidence(
                            &conn,
                            run_id,
                            "workspace_snapshot",
                            &evidence,
                        )
                    }) {
                        emit(
                            &app,
                            project_id,
                            "status",
                            format!("Could not persist disconnected workspace evidence: {error}"),
                            None,
                            None,
                        );
                    }
                }
            }
        }
    }
    if !suppress_disconnect_event {
        emit(
            &app,
            project_id,
            "disconnected",
            "Agent process disconnected",
            None,
            None,
        );
    }
}

async fn start_session(
    app: &AppHandle,
    project_id: i64,
    root: &str,
    mode: AgentMode,
    suppress_disconnect_event: bool,
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
    let expected_response = Arc::new(Mutex::new(None));
    let active_run_id = Arc::new(AtomicI64::new(0));
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
        expected_response.clone(),
        active_run_id.clone(),
        PathBuf::from(root),
        suppress_disconnect_event,
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
        expected_response,
        active_run_id,
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

pub(crate) fn design_context(
    conn: &rusqlite::Connection,
    project_id: i64,
) -> Result<(String, i64), String> {
    let (nodes_json, edges_json, revision, pending_draft): (
        String,
        String,
        i64,
        Option<(i64, i64, i64, String)>,
    ) = conn
        .query_row(
            "SELECT m.nodes, m.edges, m.revision, d.id, d.base_revision, d.version, d.draft
             FROM project_maps m
             LEFT JOIN design_drafts d ON d.id = (
                 SELECT id FROM design_drafts
                 WHERE project_id = m.project_id AND state = 'pending'
                 ORDER BY version DESC LIMIT 1
             )
             WHERE m.project_id = ?1",
            [project_id],
            |row| {
                let draft_id: Option<i64> = row.get(3)?;
                let draft = if let Some(id) = draft_id {
                    Some((id, row.get(4)?, row.get(5)?, row.get(6)?))
                } else {
                    None
                };
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, draft))
            },
        )
        .map_err(|error| error.to_string())?;
    let draft = match pending_draft {
        Some((id, base_revision, version, serialized)) => Some(json!({
            "id": id,
            "baseRevision": base_revision,
            "version": version,
            "current": base_revision == revision,
            "draft": serde_json::from_str::<Value>(&serialized)
                .map_err(|error| error.to_string())?,
        })),
        None => None,
    };
    let serialized = serde_json::to_string_pretty(&json!({
        "mode": "design",
        "confirmedMap": {
            "revision": revision,
            "nodes": serde_json::from_str::<Value>(&nodes_json).map_err(|error| error.to_string())?,
            "edges": serde_json::from_str::<Value>(&edges_json).map_err(|error| error.to_string())?,
        },
        "pendingDraft": draft,
        "rules": [
            "The confirmed map is authoritative at the stated revision.",
            "A pending draft is unconfirmed and must never be described as the confirmed map.",
            "Do not use unsaved UI state; only the persisted map and draft above are available.",
            "This is design-only context. Do not modify project files or perform implementation work.",
        ],
    }))
    .map_err(|error| error.to_string())?;
    Ok((serialized, revision))
}

fn optional_context_id(context: &str, pointer: &str) -> String {
    serde_json::from_str::<Value>(context)
        .ok()
        .and_then(|value| value.pointer(pointer).and_then(Value::as_i64))
        .map_or_else(|| "null".to_owned(), |value| value.to_string())
}

async fn request_design_turn(
    app: &AppHandle,
    manager: &AgentManager,
    project_id: i64,
    root: &str,
    expected: ResponseExpectation,
    prompt: &str,
) -> Result<String, String> {
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
            *guard = Some(
                start_session(
                    app,
                    project_id,
                    root,
                    AgentMode::Design,
                    manager.suppress_disconnect_events,
                )
                .await?,
            );
        }
        let session = guard.as_ref().ok_or("Agent session unavailable")?;
        if session.busy.swap(true, Ordering::AcqRel) {
            return Err("Agent is already working".into());
        }
        *session.expected_response.lock().await = Some(expected);
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
    response
        .lock()
        .await
        .clone()
        .ok_or_else(|| "Agent returned no valid structured design response".into())
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
) -> Result<DevelopmentStartResult, String> {
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
    let run_id = create_development_unit_run(&conn, unit.id)?;
    let criteria = unit
        .completion_criteria
        .iter()
        .map(|criterion| format!("- {criterion}"))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = format!(
        "HEIM MODE: DEVELOPMENT. Return exactly one JSON object as your final response, without markdown, using this contract: {{\"contractVersion\":1,\"mode\":\"development\",\"context\":{{\"mapRevision\":{revision},\"draftId\":null,\"draftVersion\":null,\"unitId\":{id}}},\"result\":{{\"kind\":\"development_report\",\"outcome\":\"completed\"|\"incomplete\"|\"blocked\",\"summary\":string,\"criteria\":[{{\"index\":number,\"status\":\"met\"|\"not_met\"|\"not_run\",\"evidence\":[string]}}],\"changes\":[{{\"path\":string,\"summary\":string}}],\"checks\":[{{\"command\":string,\"status\":\"passed\"|\"failed\"|\"not_run\",\"summary\":string}}],\"blockers\":[string]}}}}. Include exactly one criteria entry for each listed criterion, in the same order, using zero-based indexes. Report only checks actually run; your claims are not independent Heim verification. Contract version: 1. The user explicitly confirmed development unit #{id} against approved Heim map revision {revision}. This revision and unit are immutable for this turn. Implement only the unit goal and its completion criteria, within the selected architecture responsibility. Do not interpret the full node responsibility as additional scope, choose another unit, or continue after this unit. Implement product behavior, not merely tests, fixtures, documentation, or a test harness. Tests may support verification after implementation. Do not alter the confirmed map or mark any node implemented/verified based on your response. Respect the existing Codex workspace-write sandbox and approval policy.\n\nSelected architecture responsibility:\n{selected}\n\nDirectly related nodes (context only, not scope expansion):\n{related}\n\nDirect relationships, preserving source, target, and relation type (context only):\n{related_edges}\n\nConfirmed unit goal:\n{goal}\n\nCompletion criteria:\n{criteria}",
        id = unit.id,
        revision = unit.map_revision,
        selected =
            serde_json::to_string_pretty(&unit.selected_node).map_err(|error| error.to_string())?,
        related =
            serde_json::to_string_pretty(&unit.related_nodes).map_err(|error| error.to_string())?,
        related_edges =
            serde_json::to_string_pretty(&unit.related_edges).map_err(|error| error.to_string())?,
        goal = unit.goal,
    );
    *guard = None;
    let session = match start_session(&app, project_id, &root, AgentMode::Development, false).await
    {
        Ok(session) => session,
        Err(error) => {
            update_development_unit_status(&conn, unit.id, "start_failed", Some(&error))?;
            transition_development_unit_run(&conn, run_id, "start_failed", Some(&error))?;
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
        transition_development_unit_run(
            &conn,
            run_id,
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
            transition_development_unit_run(
                &conn,
                run_id,
                "start_failed",
                Some(&error.to_string()),
            )?;
            return Err(error.to_string());
        }
    };
    *session.snapshot.lock().await = Some(before);
    if let Err(error) = update_development_unit_status(&conn, unit.id, "started", None) {
        session.busy.store(false, Ordering::Release);
        transition_development_unit_run(&conn, run_id, "start_failed", Some(&error))?;
        return Err(error);
    }
    *session.expected_response.lock().await = Some(ResponseExpectation {
        mode: AgentMode::Development,
        map_revision: unit.map_revision,
        draft_id: None,
        draft_version: None,
        unit_id: Some(unit.id),
        node_id: None,
        criteria_count: unit.completion_criteria.len(),
        result_kind: "development_report",
    });
    *session.response.lock().await = None;
    session.active_run_id.store(run_id, Ordering::Release);
    let result = session.request(
        "turn/start",
        json!({"threadId":session.thread_id, "input":[{"type":"text", "text":prompt}], "cwd":root}),
    ).await;
    if let Err(error) = result {
        session.busy.store(false, Ordering::Release);
        let active_run_id = session.active_run_id.swap(0, Ordering::AcqRel);
        if active_run_id > 0 {
            transition_development_unit_run(&conn, run_id, "failed", Some(&error))?;
        }
        return Err(error);
    }
    transition_development_unit_run(&conn, run_id, "running", None)?;
    Ok(DevelopmentStartResult {
        unit_id: unit.id,
        run_id,
    })
}

pub(crate) async fn request_design_draft(
    app: AppHandle,
    manager: &AgentManager,
    project_id: i64,
    text: &str,
    latest_draft: Option<&str>,
    expected_draft: Option<(i64, i64)>,
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
    let (context, context_revision) = design_context(&conn, project_id)?;
    let draft_instructions = match latest_draft {
        Some(draft) => format!(
            "Revise the latest saved design draft only. Do not implement or create/edit files. Treat the saved draft below as the current user-edited source of truth; preserve stable IDs for unchanged nodes and apply the user's requested changes. Put the map payload inside result.draft in the required versioned response envelope. Its schema is {{\"goal\":string,\"nodes\":[{{\"id\":string,\"title\":string,\"summary\":string explaining the node's responsibility and boundaries,\"responsibilities\":[string],\"type\":\"component\",\"status\":\"not_started\",\"questions\":[{{\"prompt\":string,\"options\":[{{\"label\":string,\"recommended\":boolean}},{{\"label\":string,\"recommended\":boolean}}]}}]}}],\"edges\":[{{\"id\":string,\"source\":string,\"target\":string,\"relation\":\"depends_on\"|\"interacts_with\"}}],\"questions\":[string],\"assumptions\":[string]}}. Every node question has exactly two distinct options and exactly one recommended:true. The UI provides a third, user-entered option. Use two viable, concise alternatives and recommend the best fit to the user's stated goals. For depends_on, source requires target; for interacts_with, source interacts with target. Never use an untyped or generic related edge. Put unresolved decisions next to the relevant node in its questions array; use the top-level questions array only for project-wide questions. Use unique stable IDs, valid edge endpoints, at least one responsibility per node, and status not_started for every node.\n\nUser change request:\n{text}\n\nLatest saved user-edited design draft:\n{draft}"
        ),
        None => format!(
            "Design a logical architecture map only. Do not implement or create/edit files. Nodes are persistent system responsibilities, not files or one-off tasks. Put the map payload inside result.draft in the required versioned response envelope. Its schema is {{\"goal\":string,\"nodes\":[{{\"id\":string,\"title\":string,\"summary\":string explaining the node's responsibility and boundaries,\"responsibilities\":[string],\"type\":\"component\",\"status\":\"not_started\",\"questions\":[{{\"prompt\":string,\"options\":[{{\"label\":string,\"recommended\":boolean}},{{\"label\":string,\"recommended\":boolean}}]}}]}}],\"edges\":[{{\"id\":string,\"source\":string,\"target\":string,\"relation\":\"depends_on\"|\"interacts_with\"}}],\"questions\":[string],\"assumptions\":[string]}}. Every node question has exactly two distinct options and exactly one recommended:true. The UI provides a third, user-entered option. Use two viable, concise alternatives and recommend the best fit to the user's stated goals. For depends_on, source requires target; for interacts_with, source interacts with target. Never use an untyped or generic related edge. Put unresolved decisions next to the relevant node in its questions array; use the top-level questions array only for project-wide questions. Use unique stable IDs, valid edge endpoints, at least one responsibility per node, and status not_started for every node.\n\nUser product description:\n{text}"
        ),
    };
    let (draft_id, draft_version) =
        expected_draft.map_or((None, None), |(id, version)| (Some(id), Some(version)));
    let prompt = format!(
        "Authoritative Heim context for this design turn:\n{context}\n\nReturn exactly one JSON object, no markdown, in this envelope: {{\"contractVersion\":1,\"mode\":\"design\",\"context\":{{\"mapRevision\":{context_revision},\"draftId\":{draft_id_json},\"draftVersion\":{draft_version_json},\"unitId\":null}},\"result\":{{\"kind\":\"map_draft\",\"message\":string,\"draft\":{{\"goal\":string,\"nodes\":[...],\"edges\":[...],\"questions\":[string],\"assumptions\":[string]}}}}}}. Echo these exact context identifiers. The draft is a proposal, not saved or approved.\n\n{draft_instructions}",
        draft_id_json = draft_id.map_or("null".to_owned(), |id| id.to_string()),
        draft_version_json = draft_version.map_or("null".to_owned(), |version| version.to_string()),
    );
    let text = request_design_turn(
        &app,
        manager,
        project_id,
        &root,
        ResponseExpectation {
            mode: AgentMode::Design,
            map_revision: context_revision,
            draft_id,
            draft_version,
            unit_id: None,
            node_id: None,
            criteria_count: 0,
            result_kind: "map_draft",
        },
        &prompt,
    )
    .await?;
    Ok(DesignResponse {
        text,
        revision: context_revision,
    })
}

#[derive(Clone)]
struct RecommendationSnapshot {
    revision: i64,
    candidates: Vec<Value>,
    completed: HashSet<String>,
    reported_completed: HashSet<String>,
    recent_runs: Vec<Value>,
    edges: Vec<Value>,
    progress_key: String,
}

fn eligible_development_nodes(
    nodes: &[Value],
    edges: &[Value],
    completed: &HashSet<String>,
    reported_completed: &HashSet<String>,
    active: &HashSet<String>,
) -> Vec<Value> {
    nodes
        .iter()
        .filter(|node| {
            if node["type"] != "component" || node_has_unanswered_questions(node) {
                return false;
            }
            let Some(id) = node["id"].as_str() else {
                return false;
            };
            if completed.contains(id) || reported_completed.contains(id) || active.contains(id) {
                return false;
            }
            edges
                .iter()
                .filter(|edge| {
                    edge["source"].as_str() == Some(id)
                        && edge.pointer("/data/relation").and_then(Value::as_str)
                            == Some("depends_on")
                })
                .all(|edge| {
                    edge["target"].as_str().is_some_and(|target| {
                        (completed.contains(target) || reported_completed.contains(target))
                            && !active.contains(target)
                    })
                })
        })
        .cloned()
        .collect()
}

fn recommendation_snapshot(
    conn: &Connection,
    project_id: i64,
    map_revision: i64,
) -> Result<RecommendationSnapshot, String> {
    let (nodes_json, edges_json, revision): (String, String, i64) = conn
        .query_row(
            "SELECT nodes, edges, revision FROM project_maps WHERE project_id = ?1",
            [project_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| error.to_string())?;
    if revision != map_revision || !has_current_approved_map(conn, project_id)? {
        return Err("Approved map changed; reload before requesting a recommendation".into());
    }
    let pending: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM design_drafts WHERE project_id = ?1 AND state = 'pending')",
            [project_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if pending {
        return Err("Resolve the pending design draft before requesting a recommendation".into());
    }
    let nodes: Vec<Value> = serde_json::from_str(&nodes_json).map_err(|error| error.to_string())?;
    let edges: Vec<Value> = serde_json::from_str(&edges_json).map_err(|error| error.to_string())?;
    validate_map(&nodes, &edges)?;
    let mut completed = HashSet::new();
    for node in &nodes {
        if node["type"] == "component"
            && node
                .pointer("/data/status")
                .and_then(Value::as_str)
                .is_some_and(|status| status.trim().eq_ignore_ascii_case("completed"))
        {
            if let Some(id) = node["id"].as_str() {
                completed.insert(id.to_owned());
            }
        }
    }
    let mut reviews = conn
        .prepare(
            "SELECT DISTINCT u.node_id FROM development_units u
         JOIN development_unit_reviews v ON v.unit_id = u.id
         WHERE u.project_id = ?1 AND v.decision = 'approved_completion'
           AND v.id = (SELECT MAX(id) FROM development_unit_reviews WHERE unit_id = u.id)
           AND v.run_id = (SELECT MAX(id) FROM development_unit_runs WHERE unit_id = u.id)",
        )
        .map_err(|error| error.to_string())?;
    let reviewed = reviews
        .query_map([project_id], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?;
    for node_id in reviewed {
        completed.insert(node_id.map_err(|error| error.to_string())?);
    }
    let mut active = HashSet::new();
    let mut runs = conn
        .prepare(
            "SELECT DISTINCT u.node_id FROM development_units u
         JOIN development_unit_runs r ON r.unit_id = u.id
         WHERE u.project_id = ?1 AND r.state IN ('starting', 'running')",
        )
        .map_err(|error| error.to_string())?;
    let running = runs
        .query_map([project_id], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?;
    for node_id in running {
        active.insert(node_id.map_err(|error| error.to_string())?);
    }
    let mut recent_runs_stmt = conn
        .prepare(
            "SELECT node_id, run_id, state, outcome, summary, review_decision FROM (
                SELECT u.node_id AS node_id, r.id AS run_id, r.state AS state,
                    COALESCE(report.outcome, '') AS outcome,
                    COALESCE(report.summary, '') AS summary,
                    COALESCE((SELECT v.decision FROM development_unit_reviews v
                        WHERE v.run_id = r.id ORDER BY v.id DESC LIMIT 1), '') AS review_decision,
                    ROW_NUMBER() OVER (PARTITION BY u.node_id ORDER BY r.id DESC) AS row_num
                FROM development_units u
                JOIN development_unit_runs r ON r.unit_id = u.id
                LEFT JOIN development_unit_run_reports report ON report.run_id = r.id
                WHERE u.project_id = ?1 AND u.map_revision = ?2
            ) WHERE row_num = 1 ORDER BY node_id",
        )
        .map_err(|error| error.to_string())?;
    let recent_runs = recent_runs_stmt
        .query_map(rusqlite::params![project_id, map_revision], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(
            |(node_id, run_id, state, outcome, summary, review_decision)| {
                json!({
                    "nodeId": node_id,
                    "runId": run_id,
                    "state": state,
                    "reportedOutcome": outcome,
                    "summary": summary,
                    "reviewDecision": review_decision,
                })
            },
        )
        .collect::<Vec<_>>();
    drop(recent_runs_stmt);
    let mut reported_completed = HashSet::new();
    for run in &recent_runs {
        if run["state"] == "finished"
            && run["reportedOutcome"] == "completed"
            && !matches!(
                run["reviewDecision"].as_str(),
                Some("rework_requested" | "blocked" | "incomplete")
            )
        {
            if let Some(node_id) = run["nodeId"].as_str() {
                reported_completed.insert(node_id.to_owned());
            }
        }
    }
    let candidates =
        eligible_development_nodes(&nodes, &edges, &completed, &reported_completed, &active);
    let mut unit_progress = conn
        .prepare(
            "SELECT u.node_id, u.status,
                COALESCE((SELECT state FROM development_unit_runs WHERE unit_id = u.id ORDER BY id DESC LIMIT 1), ''),
                COALESCE((SELECT decision FROM development_unit_reviews WHERE unit_id = u.id ORDER BY id DESC LIMIT 1), '')
             FROM development_units u WHERE u.project_id = ?1 ORDER BY u.node_id, u.id",
        )
        .map_err(|error| error.to_string())?;
    let unit_progress = unit_progress
        .query_map([project_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let mut completed_ids = completed.iter().cloned().collect::<Vec<_>>();
    completed_ids.sort();
    let mut active_ids = active.iter().cloned().collect::<Vec<_>>();
    active_ids.sort();
    let mut reported_completed_ids = reported_completed.iter().cloned().collect::<Vec<_>>();
    reported_completed_ids.sort();
    let candidate_ids = candidates
        .iter()
        .filter_map(|node| node["id"].as_str())
        .collect::<Vec<_>>();
    let progress_key = serde_json::to_string(&json!({
        "completedNodeIds": completed_ids,
        "reportedCompletedNodeIds": reported_completed_ids,
        "activeNodeIds": active_ids,
        "eligibleNodeIds": candidate_ids,
        "developmentUnits": unit_progress,
        "recentRuns": recent_runs,
    }))
    .map_err(|error| error.to_string())?;
    Ok(RecommendationSnapshot {
        revision,
        candidates,
        completed,
        reported_completed,
        recent_runs,
        edges,
        progress_key,
    })
}

fn load_cached_recommendation(
    conn: &Connection,
    project_id: i64,
    snapshot: &RecommendationSnapshot,
) -> Result<Option<Option<NextDevelopmentRecommendation>>, String> {
    let cached = conn
        .query_row(
            "SELECT node_id, rationale FROM next_development_recommendations
             WHERE project_id = ?1 AND map_revision = ?2 AND progress_key = ?3",
            rusqlite::params![project_id, snapshot.revision, snapshot.progress_key],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )
        .optional()
        .map_err(|error| error.to_string())?;
    match cached {
        None => Ok(None),
        Some((None, None)) => Ok(Some(None)),
        Some((Some(node_id), Some(rationale))) => {
            if !snapshot
                .candidates
                .iter()
                .any(|candidate| candidate["id"].as_str() == Some(node_id.as_str()))
            {
                return Err("Cached next-node recommendation is no longer eligible".into());
            }
            Ok(Some(Some(NextDevelopmentRecommendation {
                node_id,
                rationale,
            })))
        }
        Some(_) => Err("Cached next-node recommendation is incomplete".into()),
    }
}

fn store_cached_recommendation(
    conn: &mut Connection,
    project_id: i64,
    snapshot: &RecommendationSnapshot,
    recommendation: Option<&NextDevelopmentRecommendation>,
) -> Result<(), String> {
    let tx = conn.transaction().map_err(|error| error.to_string())?;
    tx.execute(
        "DELETE FROM next_development_recommendations WHERE project_id = ?1",
        [project_id],
    )
    .map_err(|error| error.to_string())?;
    tx.execute(
        "INSERT INTO next_development_recommendations
            (project_id, map_revision, progress_key, node_id, rationale)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            project_id,
            snapshot.revision,
            snapshot.progress_key,
            recommendation.map(|item| item.node_id.as_str()),
            recommendation.map(|item| item.rationale.as_str()),
        ],
    )
    .map_err(|error| error.to_string())?;
    tx.commit().map_err(|error| error.to_string())
}

fn parse_recommendation(
    response: &str,
    revision: i64,
    candidates: &[Value],
) -> Result<NextDevelopmentRecommendation, String> {
    let expected = ResponseExpectation {
        mode: AgentMode::Design,
        map_revision: revision,
        draft_id: None,
        draft_version: None,
        unit_id: None,
        node_id: None,
        criteria_count: 0,
        result_kind: "next_development_node",
    };
    let (result, _) = validate_agent_response(response, &expected)?;
    let node_id = result["nodeId"]
        .as_str()
        .ok_or("AI recommendation node ID is missing")?;
    if !candidates
        .iter()
        .any(|node| node["id"].as_str() == Some(node_id))
    {
        return Err("AI recommendation is not an eligible component node".into());
    }
    Ok(NextDevelopmentRecommendation {
        node_id: node_id.to_owned(),
        rationale: result["rationale"]
            .as_str()
            .ok_or("AI recommendation rationale is missing")?
            .to_owned(),
    })
}

#[tauri::command]
pub async fn recommend_next_development_node(
    app: AppHandle,
    project_id: i64,
    map_revision: i64,
) -> Result<Option<NextDevelopmentRecommendation>, String> {
    let mut conn = database(&app)?;
    ensure_project(&conn, project_id)?;
    let root: String = conn
        .query_row(
            "SELECT root FROM projects WHERE id = ?1",
            [project_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let snapshot = recommendation_snapshot(&conn, project_id, map_revision)?;
    if let Some(cached) = load_cached_recommendation(&conn, project_id, &snapshot)? {
        return Ok(cached);
    }
    if snapshot.candidates.is_empty() {
        store_cached_recommendation(&mut conn, project_id, &snapshot, None)?;
        return Ok(None);
    }
    let context = json!({
        "confirmedMapRevision": snapshot.revision,
        "eligibleComponentNodes": snapshot.candidates,
        "completedNodeIds": snapshot.completed,
        "agentReportedCompletedNodeIds": snapshot.reported_completed,
        "recentDevelopmentRuns": snapshot.recent_runs,
        "relationships": snapshot.edges,
    });
    let response_contract = json!({
        "contractVersion": 1,
        "mode": "design",
        "context": {
            "mapRevision": map_revision,
            "draftId": null,
            "draftVersion": null,
            "unitId": null,
        },
        "result": {
            "kind": "next_development_node",
            "nodeId": "string",
            "rationale": "string",
        },
    });
    let prompt = format!(
        "HEIM MODE: NEXT DEVELOPMENT NODE RECOMMENDATION. This is read-only design advice: do not edit files, run commands, create development units, or start implementation. Use recentDevelopmentRuns and agentReportedCompletedNodeIds as explicit progress history. Do not recommend a node whose latest finished report says completed unless its latest review requests rework, marks it blocked, or marks it incomplete. Candidates exclude reported-completed nodes; a depends_on candidate is eligible when its target is user-approved complete or agent-reported complete. Agent reports are not independently verified: this recommendation does not approve a run or change node status. Choose exactly one best node from eligibleComponentNodes, considering map responsibilities, relationships, and recent run summaries; do not repeat already implemented work. Give a concise rationale that acknowledges relevant progress. Return one JSON object matching this contract, with string values for nodeId and rationale, and no markdown:\n{contract}\n\nProject progress context:\n{context}",
        contract = response_contract,
        context = serde_json::to_string_pretty(&context).map_err(|error| error.to_string())?,
    );
    drop(conn);
    // Keep map recommendations independent from interactive design/development turns.
    let manager = AgentManager {
        suppress_disconnect_events: true,
        ..AgentManager::default()
    };
    let response = request_design_turn(
        &app,
        &manager,
        project_id,
        &root,
        ResponseExpectation {
            mode: AgentMode::Design,
            map_revision,
            draft_id: None,
            draft_version: None,
            unit_id: None,
            node_id: None,
            criteria_count: 0,
            result_kind: "next_development_node",
        },
        &prompt,
    )
    .await?;
    let recommendation = parse_recommendation(&response, map_revision, &snapshot.candidates)?;
    let mut conn = database(&app)?;
    let current = recommendation_snapshot(&conn, project_id, map_revision)?;
    if current.progress_key != snapshot.progress_key
        || !current
            .candidates
            .iter()
            .any(|node| node["id"].as_str() == Some(recommendation.node_id.as_str()))
    {
        return Err("Development progress changed; request a new recommendation".into());
    }
    store_cached_recommendation(&mut conn, project_id, &current, Some(&recommendation))?;
    Ok(Some(recommendation))
}

async fn request_development_unit_plan(
    app: &AppHandle,
    manager: &AgentManager,
    project_id: i64,
    map_revision: i64,
    node_id: &str,
    context: &Value,
) -> Result<Value, String> {
    let conn = database(app)?;
    ensure_project(&conn, project_id)?;
    let root: String = conn
        .query_row(
            "SELECT root FROM projects WHERE id = ?1",
            [project_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let prompt = format!(
        "HEIM MODE: UNIT PLANNING. This is read-only planning: do not edit files, run commands, or implement. For the selected confirmed architecture node, propose exactly one small, coherent, actionable first development unit, not the whole node responsibility. Completion criteria must be concrete, observable, and independently assessable. Include tests/verification only as criteria when the product behavior makes them relevant. Use direct neighbors and relationships only as context, never as scope expansion. Return exactly one JSON object without markdown: {{\"contractVersion\":1,\"mode\":\"design\",\"context\":{{\"mapRevision\":{map_revision},\"draftId\":null,\"draftVersion\":null,\"unitId\":null}},\"result\":{{\"kind\":\"unit_plan\",\"nodeId\":{node_id_json},\"goal\":string,\"completionCriteria\":[string]}}}}. Echo the exact node id and revision.\n\nAuthoritative node context:\n{context}",
        node_id_json = serde_json::to_string(node_id).map_err(|error| error.to_string())?,
        context = serde_json::to_string_pretty(context).map_err(|error| error.to_string())?,
    );
    let response = request_design_turn(
        app,
        manager,
        project_id,
        &root,
        ResponseExpectation {
            mode: AgentMode::Design,
            map_revision,
            draft_id: None,
            draft_version: None,
            unit_id: None,
            node_id: Some(node_id.to_owned()),
            criteria_count: 0,
            result_kind: "unit_plan",
        },
        &prompt,
    )
    .await?;
    let envelope: Value = serde_json::from_str(&response).map_err(|error| error.to_string())?;
    envelope
        .get("result")
        .cloned()
        .ok_or("AI unit plan response is missing result data".into())
}

#[tauri::command]
pub async fn get_or_generate_development_unit_spec(
    app: AppHandle,
    manager: State<'_, AgentManager>,
    project_id: i64,
    map_revision: i64,
    node_id: String,
) -> Result<DevelopmentUnitSpec, String> {
    let conn = database(&app)?;
    ensure_project(&conn, project_id)?;
    if !has_current_approved_map(&conn, project_id)? {
        return Err("Approve the architecture map before generating a development plan".into());
    }
    let has_pending_draft: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM design_drafts WHERE project_id = ?1 AND state = 'pending')",
            [project_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if has_pending_draft {
        return Err("Resolve the pending design draft before generating a development plan".into());
    }
    let (nodes_json, edges_json, current_revision): (String, String, i64) = conn
        .query_row(
            "SELECT nodes, edges, revision FROM project_maps WHERE project_id = ?1",
            [project_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| error.to_string())?;
    if current_revision != map_revision {
        return Err("Map changed; reload before generating a development plan".into());
    }
    if let Some(spec) = load_development_unit_spec(&conn, project_id, map_revision, &node_id)? {
        return Ok(spec);
    }
    let nodes: Vec<Value> = serde_json::from_str(&nodes_json).map_err(|error| error.to_string())?;
    let edges: Vec<Value> = serde_json::from_str(&edges_json).map_err(|error| error.to_string())?;
    let selected_node = nodes
        .iter()
        .find(|node| {
            node.get("id").and_then(Value::as_str) == Some(node_id.as_str())
                && node.get("type").and_then(Value::as_str) == Some("component")
        })
        .ok_or("Select a system responsibility from the confirmed map")?;
    if node_has_unanswered_questions(selected_node) {
        return Err(
            "Answer the selected node's open questions before generating its development plan"
                .into(),
        );
    }
    let related_ids = edges
        .iter()
        .filter(|edge| {
            edge.get("source").and_then(Value::as_str) == Some(node_id.as_str())
                || edge.get("target").and_then(Value::as_str) == Some(node_id.as_str())
        })
        .flat_map(|edge| {
            [
                edge.get("source").and_then(Value::as_str),
                edge.get("target").and_then(Value::as_str),
            ]
            .into_iter()
            .flatten()
        })
        .filter(|related_id| *related_id != node_id)
        .collect::<HashSet<_>>();
    let related_nodes = nodes
        .iter()
        .filter(|node| {
            node.get("type").and_then(Value::as_str) == Some("component")
                && node
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| related_ids.contains(id))
        })
        .cloned()
        .collect::<Vec<_>>();
    let related_edges = edges
        .iter()
        .filter(|edge| {
            edge.get("source").and_then(Value::as_str) == Some(node_id.as_str())
                || edge.get("target").and_then(Value::as_str) == Some(node_id.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    let context = json!({
        "confirmedMapRevision": map_revision,
        "selectedNode": selected_node,
        "relatedNodes": related_nodes,
        "relatedEdges": related_edges,
        "scope": "Only the selected node's first development unit; relationships are context, not scope.",
    });
    drop(conn);
    let result =
        request_development_unit_plan(&app, &manager, project_id, map_revision, &node_id, &context)
            .await?;
    let goal = result["goal"]
        .as_str()
        .ok_or("AI unit plan goal is missing")?
        .to_owned();
    let completion_criteria = result["completionCriteria"]
        .as_array()
        .ok_or("AI unit plan criteria are missing")?
        .iter()
        .map(|criterion| {
            criterion
                .as_str()
                .map(str::to_owned)
                .ok_or("AI unit plan criteria must be strings")
        })
        .collect::<Result<Vec<_>, _>>()?;
    validate_development_unit_spec(&goal, &completion_criteria)?;
    let generated = DevelopmentUnitSpec {
        project_id,
        map_revision,
        node_id,
        goal,
        completion_criteria,
        source: "agent".into(),
        version: 1,
        updated_at: String::new(),
    };
    let mut conn = database(&app)?;
    store_generated_development_unit_spec(&mut conn, &generated)
}

#[tauri::command]
pub async fn send_design_message(
    app: AppHandle,
    manager: State<'_, AgentManager>,
    project_id: i64,
    text: String,
) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() || text.len() > 20_000 {
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
    let (map_context, context_revision) = design_context(&conn, project_id)?;
    let prompt = format!(
        "HEIM MODE: DESIGN. Discuss architecture, clarify questions, and propose changes only. This session is read-only; do not create, edit, or delete files, run commands, or begin implementation. The backend-provided context is authoritative: respect its confirmed-map revision, and treat any pending draft as unconfirmed. Never claim a proposal is persisted or approved. Clearly label any proposed changes as suggestions and ask the user to use Heim's draft review to save and approve them. Return exactly one JSON object without markdown: {{\"contractVersion\":1,\"mode\":\"design\",\"context\":{{\"mapRevision\":{context_revision},\"draftId\":{draft_id},\"draftVersion\":{draft_version},\"unitId\":null}},\"result\":{{\"kind\":\"discussion\",\"message\":string,\"questions\":[string],\"assumptions\":[string]}}}}. Echo the context identifiers exactly; do not include an unrequested map draft.\n\nAuthoritative persisted Heim context:\n{map_context}\n\nUser design request:\n{text}",
        draft_id = optional_context_id(&map_context, "/pendingDraft/id"),
        draft_version = optional_context_id(&map_context, "/pendingDraft/version"),
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
    *guard = Some(start_session(&app, project_id, &root, AgentMode::Design, false).await?);
    let session = guard.as_ref().ok_or("Agent session unavailable")?;
    if session.busy.swap(true, Ordering::AcqRel) {
        return Err("Agent is already working".into());
    }
    *session.expected_response.lock().await = Some(ResponseExpectation {
        mode: AgentMode::Design,
        map_revision: context_revision,
        draft_id: serde_json::from_str::<Value>(&map_context)
            .ok()
            .and_then(|value| value.pointer("/pendingDraft/id").and_then(Value::as_i64)),
        draft_version: serde_json::from_str::<Value>(&map_context)
            .ok()
            .and_then(|value| {
                value
                    .pointer("/pendingDraft/version")
                    .and_then(Value::as_i64)
            }),
        unit_id: None,
        node_id: None,
        criteria_count: 0,
        result_kind: "discussion",
    });
    *session.response.lock().await = None;
    let result = session
        .request(
            "turn/start",
            json!({"threadId":session.thread_id,"input":[{"type":"text","text":prompt}],"cwd":root}),
        )
        .await;
    if result.is_err() {
        session.busy.store(false, Ordering::Release);
    }
    result.map(|_| ())
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

    #[test]
    fn development_run_outcomes_do_not_treat_errors_as_success() {
        assert_eq!(
            terminal_development_run_state("completed", None),
            ("finished", None)
        );
        assert_eq!(
            terminal_development_run_state("completed", Some("provider error")),
            ("failed", Some("provider error"))
        );
        assert_eq!(
            terminal_development_run_state("interrupted", None),
            ("interrupted", Some("interrupted"))
        );
        assert_eq!(
            terminal_development_run_state("unknown", None),
            ("failed", Some("unknown"))
        );
    }

    #[test]
    fn persistent_command_evidence_redacts_credential_like_arguments() {
        assert_eq!(command_for_persistent_evidence("cargo test"), "cargo test");
        assert_eq!(
            command_for_persistent_evidence("tool --api-key=secret123 verify"),
            "[redacted command containing a credential-like argument]"
        );
    }

    #[test]
    fn response_contract_rejects_wrong_versions_modes_and_unit_criteria() {
        let expected = ResponseExpectation {
            mode: AgentMode::Development,
            map_revision: 7,
            draft_id: None,
            draft_version: None,
            unit_id: Some(42),
            node_id: None,
            criteria_count: 2,
            result_kind: "development_report",
        };
        let response = json!({
            "contractVersion":1,
            "mode":"development",
            "context":{"mapRevision":7,"draftId":null,"draftVersion":null,"unitId":42},
            "result":{
                "kind":"development_report",
                "outcome":"incomplete",
                "summary":"One criterion is not verified.",
                "criteria":[
                    {"index":0,"status":"met","evidence":["Implemented behavior"]},
                    {"index":1,"status":"not_run","evidence":["Test not run"]}
                ],
                "changes":[{"path":"src/lib.rs","summary":"Added behavior"}],
                "checks":[{"command":"cargo test","status":"not_run","summary":"Not run"}],
                "blockers":[]
            }
        })
        .to_string();
        let (result, summary) = validate_agent_response(&response, &expected).unwrap();
        assert_eq!(result["outcome"], "incomplete");
        assert!(summary.contains("One criterion is not verified."));
        assert!(summary.contains("Criterion 1: not_run"));

        let wrong_revision = response.replace("\"mapRevision\":7", "\"mapRevision\":6");
        assert!(validate_agent_response(&wrong_revision, &expected).is_err());
        let mut missing_criterion: Value = serde_json::from_str(&response).unwrap();
        missing_criterion["result"]["criteria"]
            .as_array_mut()
            .unwrap()
            .pop();
        assert!(validate_agent_response(&missing_criterion.to_string(), &expected).is_err());
        let wrong_mode = response.replace("\"mode\":\"development\"", "\"mode\":\"design\"");
        assert!(validate_agent_response(&wrong_mode, &expected).is_err());
    }

    #[test]
    fn design_discussion_contract_requires_the_pinned_draft_identity() {
        let expected = ResponseExpectation {
            mode: AgentMode::Design,
            map_revision: 3,
            draft_id: Some(9),
            draft_version: Some(2),
            unit_id: None,
            node_id: None,
            criteria_count: 0,
            result_kind: "discussion",
        };
        let response = json!({
            "contractVersion":1,
            "mode":"design",
            "context":{"mapRevision":3,"draftId":9,"draftVersion":2,"unitId":null},
            "result":{"kind":"discussion","message":"The draft keeps the approved map unchanged.","questions":[],"assumptions":[]}
        }).to_string();
        let (_, message) = validate_agent_response(&response, &expected).unwrap();
        assert_eq!(message, "The draft keeps the approved map unchanged.");
        let wrong_draft = response.replace("\"draftVersion\":2", "\"draftVersion\":1");
        assert!(validate_agent_response(&wrong_draft, &expected).is_err());
        let malformed = response.replace("\"questions\":[]", "\"questions\":\"none\"");
        assert!(validate_agent_response(&malformed, &expected).is_err());
    }

    #[test]
    fn unit_plan_contract_binds_goal_and_criteria_to_selected_node() {
        let expected = ResponseExpectation {
            mode: AgentMode::Design,
            map_revision: 5,
            draft_id: None,
            draft_version: None,
            unit_id: None,
            node_id: Some("records".into()),
            criteria_count: 0,
            result_kind: "unit_plan",
        };
        let response = json!({
            "contractVersion":1,
            "mode":"design",
            "context":{"mapRevision":5,"draftId":null,"draftVersion":null,"unitId":null},
            "result":{"kind":"unit_plan","nodeId":"records","goal":"Persist a reading status change","completionCriteria":["A user can mark a book complete","The new status remains after reopening the project"]}
        }).to_string();
        let (result, _) = validate_agent_response(&response, &expected).unwrap();
        assert_eq!(result["nodeId"], "records");
        assert_eq!(result["completionCriteria"].as_array().unwrap().len(), 2);
        let wrong_node = response.replace("records", "storage");
        assert!(validate_agent_response(&wrong_node, &expected).is_err());
        let too_many_criteria = response.replace(
            "[\"A user can mark a book complete\",\"The new status remains after reopening the project\"]",
            "[]",
        );
        assert!(validate_agent_response(&too_many_criteria, &expected).is_err());
    }

    #[test]
    fn recommendation_eligibility_respects_dependency_direction_and_progress() {
        let nodes = vec![
            json!({"id":"storage","type":"component","data":{"status":"Not started"}}),
            json!({"id":"api","type":"component","data":{"status":"Not started"}}),
            json!({"id":"ui","type":"component","data":{"status":"Not started"}}),
            json!({"id":"question","type":"component","data":{"questions":[{"prompt":"Choose storage"}]}}),
            json!({"id":"layer","type":"layer"}),
        ];
        let edges = vec![
            json!({"source":"api","target":"storage","data":{"relation":"depends_on"}}),
            json!({"source":"ui","target":"api","data":{"relation":"interacts_with"}}),
        ];
        let ids =
            |completed: HashSet<String>, reported: HashSet<String>, active: HashSet<String>| {
                eligible_development_nodes(&nodes, &edges, &completed, &reported, &active)
                    .iter()
                    .map(|node| node["id"].as_str().unwrap().to_owned())
                    .collect::<Vec<_>>()
            };
        assert_eq!(
            ids(HashSet::new(), HashSet::new(), HashSet::new()),
            ["storage", "ui"]
        );
        assert_eq!(
            ids(
                HashSet::new(),
                HashSet::from(["storage".into()]),
                HashSet::new()
            ),
            ["api", "ui"],
            "agent-reported completion must keep recommending a different eligible node"
        );
        assert_eq!(
            ids(
                HashSet::from(["storage".into()]),
                HashSet::new(),
                HashSet::new()
            ),
            ["api", "ui"]
        );
        assert_eq!(
            ids(
                HashSet::from(["storage".into()]),
                HashSet::new(),
                HashSet::from(["storage".into()])
            ),
            ["ui"]
        );
        assert_eq!(
            ids(
                HashSet::from(["storage".into()]),
                HashSet::new(),
                HashSet::from(["api".into()])
            ),
            ["ui"]
        );
        assert!(
            ids(
                HashSet::from(["storage".into(), "api".into(), "ui".into()]),
                HashSet::new(),
                HashSet::new()
            )
            .is_empty()
        );
    }

    #[test]
    fn recommendation_requires_nonempty_allowed_node_and_valid_envelope() {
        let candidates = vec![json!({"id":"storage","type":"component"})];
        let response = json!({
            "contractVersion":1,
            "mode":"design",
            "context":{"mapRevision":3,"draftId":null,"draftVersion":null,"unitId":null},
            "result":{"kind":"next_development_node","nodeId":"storage","rationale":"Unblocks the API."}
        });
        assert_eq!(
            parse_recommendation(&response.to_string(), 3, &candidates).unwrap(),
            NextDevelopmentRecommendation {
                node_id: "storage".into(),
                rationale: "Unblocks the API.".into()
            }
        );
        assert!(parse_recommendation(&response.to_string(), 3, &[]).is_err());
        for (field, invalid) in [
            ("nodeId", json!("other")),
            ("nodeId", json!("")),
            ("rationale", json!("  ")),
            ("rationale", Value::Null),
        ] {
            let mut invalid_response = response.clone();
            invalid_response["result"][field] = invalid;
            assert!(parse_recommendation(&invalid_response.to_string(), 3, &candidates).is_err());
        }
        let mut wrong_revision = response.clone();
        wrong_revision["context"]["mapRevision"] = json!(2);
        assert!(parse_recommendation(&wrong_revision.to_string(), 3, &candidates).is_err());
    }

    #[test]
    fn recommendation_snapshot_uses_approved_map_reviews_and_active_runs() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(crate::SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, root) VALUES (1, 'test', '.')",
            [],
        )
        .unwrap();
        let nodes = json!([
            {"id":"storage","type":"component","position":{"x":0,"y":0},"data":{"title":"Storage","summary":"Store data","responsibilities":["Store data"],"status":"Not started"}},
            {"id":"api","type":"component","position":{"x":1,"y":0},"data":{"title":"API","summary":"Expose data","responsibilities":["Expose data"],"status":"Not started"}},
            {"id":"ui","type":"component","position":{"x":2,"y":0},"data":{"title":"UI","summary":"Display data","responsibilities":["Display data"],"status":"Completed"}}
        ]);
        let edges = json!([{"id":"api-storage","source":"api","target":"storage","data":{"relation":"depends_on"}}]);
        conn.execute(
            "INSERT INTO project_maps (project_id, nodes, edges, revision) VALUES (1, ?1, ?2, 1)",
            rusqlite::params![nodes.to_string(), edges.to_string()],
        )
        .unwrap();
        assert!(recommendation_snapshot(&conn, 1, 1).is_err());
        conn.execute("INSERT INTO design_drafts (project_id, base_revision, version, draft, state) VALUES (1, 0, 1, '{}', 'accepted')", []).unwrap();
        let ids = |snapshot: RecommendationSnapshot| {
            snapshot
                .candidates
                .into_iter()
                .map(|node| node["id"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(recommendation_snapshot(&conn, 1, 1).unwrap()),
            ["storage"]
        );
        let initial_snapshot = recommendation_snapshot(&conn, 1, 1).unwrap();
        let cached_recommendation = NextDevelopmentRecommendation {
            node_id: "storage".into(),
            rationale: "It has no unfinished prerequisites.".into(),
        };
        store_cached_recommendation(
            &mut conn,
            1,
            &initial_snapshot,
            Some(&cached_recommendation),
        )
        .unwrap();
        assert_eq!(
            load_cached_recommendation(&conn, 1, &initial_snapshot).unwrap(),
            Some(Some(cached_recommendation))
        );
        conn.execute("INSERT INTO development_units (id, project_id, map_revision, node_id, goal, completion_criteria) VALUES (1, 1, 1, 'storage', 'Store', '[\"Stored\"]')", []).unwrap();
        conn.execute(
            "INSERT INTO development_unit_runs (id, unit_id, state) VALUES (1, 1, 'finished')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO development_unit_run_reports (run_id, outcome, summary, report) VALUES (1, 'completed', 'Catalog lookup was implemented.', '{}')",
            [],
        )
        .unwrap();
        let unreviewed_report_snapshot = recommendation_snapshot(&conn, 1, 1).unwrap();
        assert_eq!(
            ids(unreviewed_report_snapshot.clone()),
            ["api"],
            "a finished completed report should move the recommendation beyond its node"
        );
        assert!(
            unreviewed_report_snapshot
                .reported_completed
                .contains("storage")
        );
        conn.execute("INSERT INTO development_unit_reviews (unit_id, run_id, decision) VALUES (1, 1, 'approved_completion')", []).unwrap();
        let completed_storage_snapshot = recommendation_snapshot(&conn, 1, 1).unwrap();
        assert_eq!(ids(completed_storage_snapshot.clone()), ["api"]);
        assert_eq!(
            load_cached_recommendation(&conn, 1, &completed_storage_snapshot).unwrap(),
            None,
            "a changed development state must miss the old cache"
        );
        conn.execute("INSERT INTO development_units (id, project_id, map_revision, node_id, goal, completion_criteria) VALUES (2, 1, 1, 'api', 'Expose', '[\"Exposed\"]')", []).unwrap();
        conn.execute(
            "INSERT INTO development_unit_runs (unit_id, state) VALUES (2, 'running')",
            [],
        )
        .unwrap();
        let no_candidate_snapshot = recommendation_snapshot(&conn, 1, 1).unwrap();
        assert!(ids(no_candidate_snapshot.clone()).is_empty());
        store_cached_recommendation(&mut conn, 1, &no_candidate_snapshot, None).unwrap();
        assert_eq!(
            load_cached_recommendation(&conn, 1, &no_candidate_snapshot).unwrap(),
            Some(None),
            "a no-candidate result is cached too"
        );
        assert!(recommendation_snapshot(&conn, 1, 0).is_err());
        conn.execute("INSERT INTO design_drafts (project_id, base_revision, version, draft, state) VALUES (1, 1, 2, '{}', 'pending')", []).unwrap();
        assert!(recommendation_snapshot(&conn, 1, 1).is_err());
    }

    #[test]
    fn workspace_snapshot_excludes_python_bytecode_caches() {
        let root = std::env::temp_dir().join(format!(
            "heim-snapshot-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cache = root.join("__pycache__");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("module.cpython-310.pyc"), [0, 159, 146, 150]).unwrap();
        std::fs::write(root.join("module.py"), "value = 1\n").unwrap();

        let snapshot = file_snapshot(&root);
        assert!(!snapshot.keys().any(|path| path.contains("__pycache__")));
        assert!(!snapshot.keys().any(|path| path.ends_with(".pyc")));
        assert!(snapshot.contains_key("module.py"));

        std::fs::remove_dir_all(root).unwrap();
    }
}
