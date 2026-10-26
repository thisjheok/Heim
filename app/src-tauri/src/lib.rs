use std::{collections::HashSet, fs, path::Path};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::Manager;

mod agent;
mod map_proposals;
mod tickets;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeInfo {
    version: String,
    platform: &'static str,
}

#[tauri::command]
fn runtime_info(app: tauri::AppHandle) -> RuntimeInfo {
    RuntimeInfo {
        version: app.package_info().version.to_string(),
        platform: std::env::consts::OS,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Project {
    id: i64,
    name: String,
    root: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectMap {
    nodes: Vec<Value>,
    edges: Vec<Value>,
    revision: i64,
}

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS projects (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        root TEXT NOT NULL UNIQUE,
        opened_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
    CREATE TABLE IF NOT EXISTS project_maps (
        project_id INTEGER PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
        nodes TEXT NOT NULL DEFAULT '[]',
        edges TEXT NOT NULL DEFAULT '[]',
        revision INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE IF NOT EXISTS map_proposals (
        id INTEGER PRIMARY KEY,
        project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
        base_revision INTEGER NOT NULL,
        changed_files TEXT NOT NULL,
        changes TEXT NOT NULL,
        state TEXT NOT NULL DEFAULT 'pending',
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
    CREATE TABLE IF NOT EXISTS proposal_sources (
        proposal_id INTEGER NOT NULL REFERENCES map_proposals(id) ON DELETE CASCADE,
        path TEXT NOT NULL,
        digest TEXT NOT NULL,
        PRIMARY KEY (proposal_id, path)
    );
    CREATE TABLE IF NOT EXISTS design_drafts (
        id INTEGER PRIMARY KEY,
        project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
        base_revision INTEGER NOT NULL,
        version INTEGER NOT NULL,
        draft TEXT NOT NULL,
        state TEXT NOT NULL DEFAULT 'pending',
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        UNIQUE(project_id, version)
    );
    DROP TABLE IF EXISTS next_development_recommendations;
    DROP TABLE IF EXISTS development_unit_reviews;
    DROP TABLE IF EXISTS development_unit_run_evidence;
    DROP TABLE IF EXISTS development_unit_run_reports;
    DROP TABLE IF EXISTS development_unit_specs;
    DROP TABLE IF EXISTS development_unit_runs;
    DROP TABLE IF EXISTS development_units;
    CREATE TABLE IF NOT EXISTS features (
        id INTEGER PRIMARY KEY,
        project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
        title TEXT NOT NULL,
        description TEXT NOT NULL DEFAULT '',
        position INTEGER NOT NULL,
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
    CREATE TABLE IF NOT EXISTS tickets (
        id INTEGER PRIMARY KEY,
        project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
        feature_id INTEGER REFERENCES features(id) ON DELETE SET NULL,
        title TEXT NOT NULL,
        goal TEXT NOT NULL,
        completion_criteria TEXT NOT NULL,
        status TEXT NOT NULL DEFAULT 'todo' CHECK(status IN ('todo', 'in_progress', 'done')),
        position INTEGER NOT NULL,
        source TEXT NOT NULL CHECK(source IN ('agent', 'user')),
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
    CREATE TABLE IF NOT EXISTS ticket_nodes (
        ticket_id INTEGER NOT NULL REFERENCES tickets(id) ON DELETE CASCADE,
        node_id TEXT NOT NULL,
        PRIMARY KEY (ticket_id, node_id)
    );
    CREATE TABLE IF NOT EXISTS ticket_runs (
        id INTEGER PRIMARY KEY,
        ticket_id INTEGER NOT NULL REFERENCES tickets(id) ON DELETE CASCADE,
        map_revision INTEGER NOT NULL,
        completion_criteria TEXT NOT NULL,
        state TEXT NOT NULL CHECK(state IN (
            'starting', 'running', 'finished', 'interrupted', 'failed', 'disconnected', 'start_failed'
        )),
        detail TEXT,
        started_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        finished_at TEXT
    );
    CREATE TABLE IF NOT EXISTS ticket_run_reports (
        run_id INTEGER PRIMARY KEY REFERENCES ticket_runs(id) ON DELETE CASCADE,
        outcome TEXT NOT NULL CHECK(outcome IN ('completed', 'incomplete', 'blocked')),
        summary TEXT NOT NULL,
        report TEXT NOT NULL,
        saved_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
    CREATE TABLE IF NOT EXISTS ticket_run_evidence (
        id INTEGER PRIMARY KEY,
        run_id INTEGER NOT NULL REFERENCES ticket_runs(id) ON DELETE CASCADE,
        kind TEXT NOT NULL CHECK(kind IN ('command_execution', 'workspace_snapshot')),
        evidence TEXT NOT NULL,
        observed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
    CREATE TABLE IF NOT EXISTS ticket_reviews (
        id INTEGER PRIMARY KEY,
        ticket_id INTEGER NOT NULL REFERENCES tickets(id) ON DELETE CASCADE,
        run_id INTEGER NOT NULL REFERENCES ticket_runs(id) ON DELETE CASCADE,
        decision TEXT NOT NULL CHECK(decision IN (
            'approved_completion', 'rework_requested', 'blocked', 'incomplete'
        )),
        note TEXT,
        reviewed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
";

fn database(app: &tauri::AppHandle) -> Result<Connection, String> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let conn =
        Connection::open(directory.join("heim.sqlite3")).map_err(|error| error.to_string())?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .and_then(|_| conn.execute_batch(SCHEMA))
        .map_err(|error| error.to_string())?;
    Ok(conn)
}

fn canonical_project(path: &Path) -> Result<String, String> {
    let canonical = path.canonicalize().map_err(|error| error.to_string())?;
    if !canonical.is_dir() {
        return Err("Project must be a directory".into());
    }
    canonical
        .to_str()
        .map(str::to_owned)
        .ok_or("Project path is not valid Unicode".into())
}

fn ensure_project(conn: &Connection, project_id: i64) -> Result<(), String> {
    let root: String = conn
        .query_row(
            "SELECT root FROM projects WHERE id = ?1",
            [project_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .ok_or("Project not found")?;
    if canonical_project(Path::new(&root))? != root {
        return Err("Project directory has changed".into());
    }
    Ok(())
}

fn project_by_id(conn: &Connection, id: i64) -> Result<Project, String> {
    conn.query_row(
        "SELECT id, name, root FROM projects WHERE id = ?1",
        [id],
        |row| {
            Ok(Project {
                id: row.get(0)?,
                name: row.get(1)?,
                root: row.get(2)?,
            })
        },
    )
    .map_err(|error| error.to_string())
}

fn register_project(app: &tauri::AppHandle, path: &Path) -> Result<Project, String> {
    let root = canonical_project(path)?;
    let canonical_path = Path::new(&root);
    let name = canonical_path
        .file_name()
        .map(|part| part.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.clone());
    let mut conn = database(app)?;
    let tx = conn.transaction().map_err(|error| error.to_string())?;
    tx.execute(
        "INSERT INTO projects (name, root) VALUES (?1, ?2)
         ON CONFLICT(root) DO UPDATE SET name = excluded.name, opened_at = CURRENT_TIMESTAMP",
        params![name, root],
    )
    .map_err(|error| error.to_string())?;
    let id: i64 = tx
        .query_row("SELECT id FROM projects WHERE root = ?1", [&root], |row| {
            row.get(0)
        })
        .map_err(|error| error.to_string())?;
    tx.execute(
        "INSERT OR IGNORE INTO project_maps (project_id) VALUES (?1)",
        [id],
    )
    .map_err(|error| error.to_string())?;
    tx.commit().map_err(|error| error.to_string())?;
    project_by_id(&conn, id)
}

#[tauri::command]
async fn pick_project(app: tauri::AppHandle) -> Result<Option<Project>, String> {
    let path = tauri::async_runtime::spawn_blocking(|| rfd::FileDialog::new().pick_folder())
        .await
        .map_err(|error| error.to_string())?;
    path.map(|path| register_project(&app, &path)).transpose()
}

#[tauri::command]
fn open_project(app: tauri::AppHandle, path: String) -> Result<Project, String> {
    register_project(&app, Path::new(&path))
}

#[tauri::command]
fn recent_projects(app: tauri::AppHandle) -> Result<Vec<Project>, String> {
    let conn = database(&app)?;
    let mut query = conn
        .prepare("SELECT id, name, root FROM projects ORDER BY opened_at DESC, id DESC LIMIT 12")
        .map_err(|error| error.to_string())?;
    query
        .query_map([], |row| {
            Ok(Project {
                id: row.get(0)?,
                name: row.get(1)?,
                root: row.get(2)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn reopen_project(app: tauri::AppHandle, project_id: i64) -> Result<Project, String> {
    let conn = database(&app)?;
    ensure_project(&conn, project_id)?;
    conn.execute(
        "UPDATE projects SET opened_at = CURRENT_TIMESTAMP WHERE id = ?1",
        [project_id],
    )
    .map_err(|error| error.to_string())?;
    project_by_id(&conn, project_id)
}

#[tauri::command]
fn project_has_content(app: tauri::AppHandle, project_id: i64) -> Result<bool, String> {
    let conn = database(&app)?;
    ensure_project(&conn, project_id)?;
    let root: String = conn
        .query_row(
            "SELECT root FROM projects WHERE id = ?1",
            [project_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let mut entries = fs::read_dir(root).map_err(|error| error.to_string())?;
    entries
        .next()
        .transpose()
        .map(|entry| entry.is_some())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn load_project_map(app: tauri::AppHandle, project_id: i64) -> Result<ProjectMap, String> {
    let conn = database(&app)?;
    ensure_project(&conn, project_id)?;
    let (nodes, edges, revision): (String, String, i64) = conn
        .query_row(
            "SELECT nodes, edges, revision FROM project_maps WHERE project_id = ?1",
            [project_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| error.to_string())?;
    Ok(ProjectMap {
        nodes: serde_json::from_str(&nodes).map_err(|error| error.to_string())?,
        edges: serde_json::from_str(&edges).map_err(|error| error.to_string())?,
        revision,
    })
}

fn validate_map(nodes: &[Value], edges: &[Value]) -> Result<(), String> {
    if nodes.len() > 1000 || edges.len() > 3000 {
        return Err("Map is too large".into());
    }
    let mut ids = HashSet::new();
    let mut component_ids = HashSet::new();
    for node in nodes {
        let id = node
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 128)
            .ok_or("Invalid node id")?;
        if !ids.insert(id) {
            return Err("Duplicate node id".into());
        }
        if !matches!(
            node.get("type").and_then(Value::as_str),
            Some("component" | "layer")
        ) || !node.get("data").is_some_and(Value::is_object)
            || !node.get("position").is_some_and(|position| {
                position.get("x").is_some_and(Value::is_number)
                    && position.get("y").is_some_and(Value::is_number)
            })
        {
            return Err("Invalid node".into());
        }
        if node.get("type").and_then(Value::as_str) == Some("component") {
            let data = node.get("data").ok_or("Invalid component data")?;
            if !data
                .get("title")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty() && value.len() <= 120)
                || !data
                    .get("summary")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.trim().is_empty() && value.len() <= 4_000)
                || !data
                    .get("responsibilities")
                    .and_then(Value::as_array)
                    .is_some_and(|items| {
                        !items.is_empty()
                            && items.len() <= 100
                            && items.iter().all(|item| {
                                item.as_str().is_some_and(|value| {
                                    !value.trim().is_empty() && value.len() <= 1_000
                                })
                            })
                    })
                || !data
                    .get("status")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.trim().is_empty() && value.len() <= 80)
            {
                return Err(
                    "Component must define its responsibility, scope, responsibilities, and status"
                        .into(),
                );
            }
            component_ids.insert(id);
        }
    }
    let mut edge_ids = HashSet::new();
    for edge in edges {
        let id = edge
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 128)
            .ok_or("Invalid edge id")?;
        if !edge_ids.insert(id) {
            return Err("Duplicate edge id".into());
        }
        let source = edge
            .get("source")
            .and_then(Value::as_str)
            .ok_or("Invalid edge source")?;
        let target = edge
            .get("target")
            .and_then(Value::as_str)
            .ok_or("Invalid edge target")?;
        if source == target || !component_ids.contains(source) || !component_ids.contains(target) {
            return Err("Edge must connect two distinct architecture nodes".into());
        }
        let relation = edge
            .pointer("/data/relation")
            .and_then(Value::as_str)
            .ok_or("Edge relationship type is missing")?;
        if !matches!(relation, "depends_on" | "interacts_with") {
            return Err("Edge relationship type must be depends_on or interacts_with".into());
        }
    }
    Ok(())
}

fn save_map(conn: &Connection, project_id: i64, map: &ProjectMap) -> Result<i64, String> {
    validate_map(&map.nodes, &map.edges)?;
    ensure_project(conn, project_id)?;
    let nodes = serde_json::to_string(&map.nodes).map_err(|error| error.to_string())?;
    let edges = serde_json::to_string(&map.edges).map_err(|error| error.to_string())?;
    let updated = conn
        .execute(
            "UPDATE project_maps SET nodes = ?1, edges = ?2, revision = revision + 1
         WHERE project_id = ?3 AND revision = ?4",
            params![nodes, edges, project_id, map.revision],
        )
        .map_err(|error| error.to_string())?;
    if updated != 1 {
        return Err("Map changed since it was opened; reload before saving".into());
    }
    Ok(map.revision + 1)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DesignDraft {
    goal: String,
    nodes: Vec<Value>,
    edges: Vec<Value>,
    questions: Vec<String>,
    assumptions: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredDesignDraft {
    id: i64,
    project_id: i64,
    base_revision: i64,
    version: i64,
    draft: DesignDraft,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MapEditDraftResult {
    draft: StoredDesignDraft,
    confirmed_map: ProjectMap,
}

fn validate_design_draft(draft: &DesignDraft) -> Result<(), String> {
    if draft.goal.trim().is_empty() || draft.goal.len() > 4_000 {
        return Err("Draft goal must contain 1-4000 characters".into());
    }
    if draft.nodes.is_empty()
        || draft.nodes.len() > 100
        || draft.questions.len() > 100
        || draft.assumptions.len() > 100
    {
        return Err("Draft node, question, or assumption count is invalid".into());
    }
    for node in &draft.nodes {
        match node.get("type").and_then(Value::as_str) {
            Some("component") => {
                if !node
                    .pointer("/data/title")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.trim().is_empty() && value.len() <= 120)
                    || !node
                        .pointer("/data/summary")
                        .and_then(Value::as_str)
                        .is_some_and(|value| !value.trim().is_empty() && value.len() <= 4_000)
                {
                    return Err("Design draft components must define a title and scope".into());
                }
                let questions = node
                    .pointer("/data/questions")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                for question in questions {
                    let prompt = question
                        .get("prompt")
                        .and_then(Value::as_str)
                        .filter(|prompt| !prompt.trim().is_empty() && prompt.len() <= 2_000)
                        .ok_or("Design draft question prompt is invalid")?;
                    let options = question
                        .get("options")
                        .and_then(Value::as_array)
                        .filter(|options| options.len() == 2)
                        .ok_or("Each design question must have exactly two options")?;
                    let mut labels = Vec::with_capacity(2);
                    let mut recommended = 0;
                    for option in options {
                        let label = option
                            .get("label")
                            .and_then(Value::as_str)
                            .filter(|label| !label.trim().is_empty() && label.len() <= 500)
                            .ok_or("Design question option is invalid")?;
                        labels.push(label);
                        recommended += usize::from(
                            option.get("recommended").and_then(Value::as_bool) == Some(true),
                        );
                    }
                    if labels[0] == labels[1] || recommended != 1 {
                        return Err(
                            "Design question options must be distinct with one recommendation"
                                .into(),
                        );
                    }
                    let _ = prompt;
                }
            }
            Some("layer") => {}
            _ => return Err("Design draft contains an unsupported node type".into()),
        }
    }
    if draft
        .questions
        .iter()
        .chain(&draft.assumptions)
        .any(|item| item.trim().is_empty() || item.len() > 4_000)
    {
        return Err("Draft questions and assumptions must contain 1-4000 characters".into());
    }
    validate_map(&draft.nodes, &draft.edges)
}

fn persist_map_edit_draft(
    conn: &mut Connection,
    project_id: i64,
    map: ProjectMap,
) -> Result<MapEditDraftResult, String> {
    ensure_project(conn, project_id)?;
    let tx = conn.transaction().map_err(|error| error.to_string())?;
    let (confirmed_nodes, confirmed_edges, current_revision): (String, String, i64) = tx
        .query_row(
            "SELECT nodes, edges, revision FROM project_maps WHERE project_id = ?1",
            [project_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| error.to_string())?;
    if map.revision != current_revision {
        return Err("Map changed since it was opened; reload before creating a draft".into());
    }
    let has_pending: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM design_drafts WHERE project_id = ?1 AND state = 'pending')",
            [project_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if has_pending {
        return Err("A design draft is already pending review".into());
    }
    let version: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(version), 0) + 1 FROM design_drafts WHERE project_id = ?1",
            [project_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let draft = DesignDraft {
        goal: "Edit the confirmed architecture map".into(),
        nodes: map.nodes,
        edges: map.edges,
        questions: Vec::new(),
        assumptions: Vec::new(),
    };
    validate_design_draft(&draft)?;
    let serialized = serde_json::to_string(&draft).map_err(|error| error.to_string())?;
    let inserted = tx
        .execute(
            "INSERT INTO design_drafts (project_id, base_revision, version, draft)
             SELECT ?1, ?2, ?3, ?4 FROM project_maps
             WHERE project_id = ?1 AND revision = ?2
               AND NOT EXISTS (SELECT 1 FROM design_drafts WHERE project_id = ?1 AND state = 'pending')",
            rusqlite::params![project_id, current_revision, version, serialized],
        )
        .map_err(|error| error.to_string())?;
    if inserted != 1 {
        return Err("Map or pending draft changed while saving the proposal".into());
    }
    let id = tx.last_insert_rowid();
    tx.commit().map_err(|error| error.to_string())?;
    Ok(MapEditDraftResult {
        draft: StoredDesignDraft {
            id,
            project_id,
            base_revision: current_revision,
            version,
            draft,
        },
        confirmed_map: ProjectMap {
            nodes: serde_json::from_str(&confirmed_nodes).map_err(|error| error.to_string())?,
            edges: serde_json::from_str(&confirmed_edges).map_err(|error| error.to_string())?,
            revision: current_revision,
        },
    })
}

#[tauri::command]
fn create_map_edit_draft(
    app: tauri::AppHandle,
    project_id: i64,
    map: ProjectMap,
) -> Result<MapEditDraftResult, String> {
    let mut conn = database(&app)?;
    persist_map_edit_draft(&mut conn, project_id, map)
}

fn edge_handles(source: &Value, target: &Value) -> (&'static str, &'static str) {
    let dx = target
        .pointer("/position/x")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
        - source
            .pointer("/position/x")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
    let dy = target
        .pointer("/position/y")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
        - source
            .pointer("/position/y")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
    if dx.abs() >= dy.abs() {
        if dx >= 0.0 {
            ("source-right", "target-left")
        } else {
            ("source-left", "target-right")
        }
    } else if dy >= 0.0 {
        ("source-bottom", "target-top")
    } else {
        ("source-top", "target-bottom")
    }
}

fn parse_design_response(
    response: &str,
    expected_revision: i64,
    expected_draft_id: Option<i64>,
    expected_draft_version: Option<i64>,
) -> Result<DesignDraft, String> {
    if response.len() > 200_000 {
        return Err("Agent design response is too large".into());
    }
    let envelope: Value = serde_json::from_str(response.trim())
        .map_err(|error| format!("Agent response was not JSON: {error}"))?;
    let context = &envelope["context"];
    if envelope["contractVersion"] != 1
        || envelope["mode"] != "design"
        || context["mapRevision"].as_i64() != Some(expected_revision)
        || context["draftId"].as_i64() != expected_draft_id
        || context["draftVersion"].as_i64() != expected_draft_version
        || !context["unitId"].is_null()
        || envelope.pointer("/result/kind").and_then(Value::as_str) != Some("map_draft")
        || envelope
            .pointer("/result/message")
            .and_then(Value::as_str)
            .map_or(true, |message| message.trim().is_empty())
    {
        return Err("Agent design response does not match the versioned map-draft contract".into());
    }
    let value = envelope
        .pointer("/result/draft")
        .ok_or("Structured map draft payload is missing")?;
    let goal = value
        .get("goal")
        .and_then(Value::as_str)
        .ok_or("Draft goal is missing")?
        .to_owned();
    let raw_nodes = value
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or("Draft nodes are missing")?;
    let mut nodes = Vec::with_capacity(raw_nodes.len());
    for (index, raw) in raw_nodes.iter().enumerate() {
        let id = raw
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty() && id.len() <= 128)
            .ok_or("Draft node ID is invalid")?;
        let title = raw
            .get("title")
            .and_then(Value::as_str)
            .filter(|title| !title.trim().is_empty() && title.len() <= 120)
            .ok_or("Draft node title is invalid")?;
        let summary = raw
            .get("summary")
            .and_then(Value::as_str)
            .filter(|summary| !summary.trim().is_empty() && summary.len() <= 4_000)
            .ok_or("Draft node summary is invalid")?;
        let responsibilities = raw
            .get("responsibilities")
            .and_then(Value::as_array)
            .ok_or("Draft node responsibilities are missing")?
            .iter()
            .map(|item| {
                item.as_str()
                    .filter(|value| !value.trim().is_empty() && value.len() <= 1_000)
                    .map(str::to_owned)
                    .ok_or("Draft node responsibility is invalid")
            })
            .collect::<Result<Vec<_>, _>>()?;
        let questions = raw
            .get("questions")
            .and_then(Value::as_array)
            .map(|items| -> Result<Vec<Value>, String> {
                items
                    .iter()
                    .map(|item| {
                        let prompt = item
                            .get("prompt")
                            .and_then(Value::as_str)
                            .filter(|prompt| !prompt.trim().is_empty() && prompt.len() <= 2_000)
                            .ok_or("Draft question prompt is invalid")?;
                        let options = item
                            .get("options")
                            .and_then(Value::as_array)
                            .filter(|options| options.len() == 2)
                            .ok_or("Draft questions must have exactly two options")?;
                        if prompt.trim().is_empty() || options.len() != 2 {
                            return Err("Draft question prompt or options are invalid".into());
                        }
                        let options = options
                            .iter()
                            .map(|option| -> Result<Value, String> {
                                let label = option
                                    .get("label")
                                    .and_then(Value::as_str)
                                    .filter(|label| !label.trim().is_empty() && label.len() <= 500)
                                    .ok_or("Draft question option is invalid")?;
                                let recommended = option
                                    .get("recommended")
                                    .and_then(Value::as_bool)
                                    .ok_or("Draft question recommendation flag is missing")?;
                                Ok(json!({"label": label, "recommended": recommended}))
                            })
                            .collect::<Result<Vec<_>, String>>()?;
                        if options[0]["label"] == options[1]["label"]
                            || options
                                .iter()
                                .filter(|option| option["recommended"] == true)
                                .count()
                                != 1
                        {
                            return Err(
                                "Draft question options must be distinct with one recommendation"
                                    .into(),
                            );
                        }
                        Ok(json!({"prompt": prompt, "options": options}))
                    })
                    .collect()
            })
            .transpose()?
            .unwrap_or_default();
        let position = json!({"x": (index % 3) as i32 * 380, "y": (index / 3) as i32 * 250});
        nodes.push(json!({"id":id,"type":"component","position":position,"data":{"title":title,"summary":summary,"kind":"System responsibility","technology":"To be decided","status":"Not started","icon":"map","responsibilities":responsibilities,"files":[],"questions":questions}}));
    }
    let edges = value
        .get("edges")
        .and_then(Value::as_array)
        .ok_or("Draft edges are missing")?
        .iter()
        .map(|edge| {
            let id = edge.get("id").and_then(Value::as_str).ok_or("Draft edge ID is missing")?;
            let source = edge.get("source").and_then(Value::as_str).ok_or("Draft edge source is missing")?;
            let target = edge.get("target").and_then(Value::as_str).ok_or("Draft edge target is missing")?;
            let relation = edge
                .get("relation")
                .and_then(Value::as_str)
                .filter(|relation| matches!(*relation, "depends_on" | "interacts_with"))
                .ok_or("Draft edge relationship type is invalid")?;
            if id.is_empty() || id.len() > 128 {
                return Err("Draft edge data is too long".into());
            }
            let source_node = nodes
                .iter()
                .find(|node| node.get("id").and_then(Value::as_str) == Some(source))
                .ok_or("Draft edge source references a missing node")?;
            let target_node = nodes
                .iter()
                .find(|node| node.get("id").and_then(Value::as_str) == Some(target))
                .ok_or("Draft edge target references a missing node")?;
            let (source_handle, target_handle) = edge_handles(source_node, target_node);
            let label = if relation == "depends_on" { "Depends on" } else { "Interacts with" };
            Ok(json!({"id":id,"source":source,"target":target,"sourceHandle":source_handle,"targetHandle":target_handle,"data":{"relation":relation},"label":label,"type":"smoothstep","animated":false,"markerEnd":{"type":"arrowclosed","color":"#aeb8c9"}}))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let strings = |key: &str| -> Result<Vec<String>, String> {
        value
            .get(key)
            .and_then(Value::as_array)
            .ok_or_else(|| format!("Draft {key} are missing"))?
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("Draft {key} must contain strings"))
            })
            .collect()
    };
    let draft = DesignDraft {
        goal,
        nodes,
        edges,
        questions: strings("questions")?,
        assumptions: strings("assumptions")?,
    };
    validate_design_draft(&draft)?;
    Ok(draft)
}

#[tauri::command]
async fn create_design_draft(
    app: tauri::AppHandle,
    manager: tauri::State<'_, agent::AgentManager>,
    project_id: i64,
    description: String,
) -> Result<StoredDesignDraft, String> {
    let conn = database(&app)?;
    ensure_project(&conn, project_id)?;
    let confirmed_nodes: String = conn
        .query_row(
            "SELECT nodes FROM project_maps WHERE project_id = ?1",
            [project_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !serde_json::from_str::<Vec<Value>>(&confirmed_nodes)
        .map_err(|error| error.to_string())?
        .is_empty()
    {
        return Err("Initial design drafts are only available for an empty confirmed map".into());
    }
    let has_pending: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM design_drafts WHERE project_id = ?1 AND state = 'pending')",
            [project_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if has_pending {
        return Err("A design draft is already pending review".into());
    }
    let response =
        agent::request_design_draft(app.clone(), &manager, project_id, &description, None, None)
            .await?;
    let draft = parse_design_response(&response.text, response.revision, None, None)?;
    let current_revision: i64 = conn
        .query_row(
            "SELECT revision FROM project_maps WHERE project_id = ?1",
            [project_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if response.revision != current_revision {
        return Err("Project map changed while the design draft was being generated".into());
    }
    let version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) + 1 FROM design_drafts WHERE project_id = ?1",
            [project_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let serialized = serde_json::to_string(&draft).map_err(|error| error.to_string())?;
    let inserted = conn
        .execute(
            "INSERT INTO design_drafts (project_id, base_revision, version, draft)
             SELECT ?1, ?2, ?3, ?4 FROM project_maps
             WHERE project_id = ?1 AND revision = ?2 AND nodes = '[]'
               AND NOT EXISTS (SELECT 1 FROM design_drafts WHERE project_id = ?1 AND state = 'pending')",
            rusqlite::params![project_id, current_revision, version, serialized],
        )
        .map_err(|error| error.to_string())?;
    if inserted != 1 {
        return Err("Project map or pending draft changed while saving the proposal".into());
    }
    Ok(StoredDesignDraft {
        id: conn.last_insert_rowid(),
        project_id,
        base_revision: current_revision,
        version,
        draft,
    })
}

#[tauri::command]
fn pending_design_draft(
    app: tauri::AppHandle,
    project_id: i64,
) -> Result<Option<StoredDesignDraft>, String> {
    let conn = database(&app)?;
    ensure_project(&conn, project_id)?;
    conn.query_row("SELECT id, base_revision, version, draft FROM design_drafts WHERE project_id = ?1 AND state = 'pending' ORDER BY version DESC LIMIT 1", [project_id], |row| Ok(StoredDesignDraft { id: row.get(0)?, project_id, base_revision: row.get(1)?, version: row.get(2)?, draft: serde_json::from_str(&row.get::<_, String>(3)?).map_err(|error| rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(error)))? })).optional().map_err(|error| error.to_string())
}

#[tauri::command]
fn development_ready(app: tauri::AppHandle, project_id: i64) -> Result<bool, String> {
    let conn = database(&app)?;
    ensure_project(&conn, project_id)?;
    has_current_approved_map(&conn, project_id)
}

pub(crate) fn has_current_approved_map(conn: &Connection, project_id: i64) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM project_maps m
            JOIN design_drafts d ON d.project_id = m.project_id
            WHERE m.project_id = ?1 AND m.revision >= d.base_revision + 1
              AND d.state = 'accepted' AND m.nodes != '[]'
        )",
        [project_id],
        |row| row.get(0),
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn node_has_unanswered_questions(node: &Value) -> bool {
    let Some(questions) = node.pointer("/data/questions").and_then(Value::as_array) else {
        return false;
    };
    questions.iter().any(|question| {
        let Some(question) = question.get("prompt").and_then(Value::as_str) else {
            return true;
        };
        !node
            .pointer("/data/answers")
            .and_then(Value::as_object)
            .and_then(|answers| answers.get(question))
            .and_then(Value::as_str)
            .is_some_and(|answer| !answer.trim().is_empty())
    })
}

#[tauri::command]
fn update_design_draft(
    app: tauri::AppHandle,
    project_id: i64,
    draft_id: i64,
    expected_version: i64,
    draft: DesignDraft,
) -> Result<StoredDesignDraft, String> {
    let conn = database(&app)?;
    persist_design_draft_edit(&conn, project_id, draft_id, expected_version, draft)
}

#[tauri::command]
fn resolve_design_draft(
    app: tauri::AppHandle,
    project_id: i64,
    draft_id: i64,
    expected_version: i64,
    accept: bool,
) -> Result<Option<ProjectMap>, String> {
    let mut conn = database(&app)?;
    resolve_design_draft_in_db(&mut conn, project_id, draft_id, expected_version, accept)
}

fn resolve_design_draft_in_db(
    conn: &mut Connection,
    project_id: i64,
    draft_id: i64,
    expected_version: i64,
    accept: bool,
) -> Result<Option<ProjectMap>, String> {
    ensure_project(&conn, project_id)?;
    let tx = conn.transaction().map_err(|error| error.to_string())?;
    let (base_revision, serialized): (i64, String) = tx
        .query_row(
            "SELECT base_revision, draft FROM design_drafts
             WHERE id = ?1 AND project_id = ?2 AND state = 'pending' AND version = ?3",
            rusqlite::params![draft_id, project_id, expected_version],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .ok_or("Design draft changed; reload before resolving it")?;
    let draft: DesignDraft =
        serde_json::from_str(&serialized).map_err(|error| error.to_string())?;
    if !accept {
        let updated = tx
            .execute(
                "UPDATE design_drafts SET state = 'rejected'
                 WHERE id = ?1 AND project_id = ?2 AND state = 'pending' AND version = ?3",
                rusqlite::params![draft_id, project_id, expected_version],
            )
            .map_err(|error| error.to_string())?;
        if updated != 1 {
            return Err("Design draft changed before it could be rejected".into());
        }
        tx.commit().map_err(|error| error.to_string())?;
        return Ok(None);
    }
    validate_design_draft(&draft)?;
    let nodes = serde_json::to_string(&draft.nodes).map_err(|error| error.to_string())?;
    let edges = serde_json::to_string(&draft.edges).map_err(|error| error.to_string())?;
    let updated_map = tx
        .execute(
            "UPDATE project_maps SET nodes = ?1, edges = ?2, revision = revision + 1
             WHERE project_id = ?3 AND revision = ?4",
            rusqlite::params![nodes, edges, project_id, base_revision],
        )
        .map_err(|error| error.to_string())?;
    if updated_map != 1 {
        return Err("Confirmed map changed; reload and review the draft again".into());
    }
    let updated_draft = tx
        .execute(
            "UPDATE design_drafts SET state = 'accepted'
             WHERE id = ?1 AND project_id = ?2 AND state = 'pending' AND version = ?3",
            rusqlite::params![draft_id, project_id, expected_version],
        )
        .map_err(|error| error.to_string())?;
    if updated_draft != 1 {
        return Err("Design draft changed before approval could be committed".into());
    }
    tx.commit().map_err(|error| error.to_string())?;
    Ok(Some(ProjectMap {
        nodes: draft.nodes,
        edges: draft.edges,
        revision: base_revision + 1,
    }))
}

#[tauri::command]
async fn revise_design_draft(
    app: tauri::AppHandle,
    manager: tauri::State<'_, agent::AgentManager>,
    project_id: i64,
    draft_id: i64,
    expected_version: i64,
    request: String,
) -> Result<StoredDesignDraft, String> {
    let request = request.trim();
    if request.is_empty() || request.len() > 20_000 {
        return Err("Invalid design revision request length".into());
    }
    let conn = database(&app)?;
    ensure_project(&conn, project_id)?;
    let (base_revision, stored_draft): (i64, String) = conn
        .query_row(
            "SELECT base_revision, draft FROM design_drafts
             WHERE id = ?1 AND project_id = ?2 AND state = 'pending' AND version = ?3",
            rusqlite::params![draft_id, project_id, expected_version],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .ok_or("Design draft changed; reload it before requesting a revision")?;
    let latest_draft: DesignDraft =
        serde_json::from_str(&stored_draft).map_err(|error| error.to_string())?;
    let current_revision: i64 = conn
        .query_row(
            "SELECT revision FROM project_maps WHERE project_id = ?1",
            [project_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if base_revision != current_revision {
        return Err("Confirmed map changed since this draft was created".into());
    }
    let latest_json = serde_json::to_string(&latest_draft).map_err(|error| error.to_string())?;
    let response = agent::request_design_draft(
        app.clone(),
        &manager,
        project_id,
        request,
        Some(&latest_json),
        Some((draft_id, expected_version)),
    )
    .await?;
    if response.revision != base_revision {
        return Err("Confirmed map changed while the revision was being generated".into());
    }
    let revised = parse_design_response(
        &response.text,
        base_revision,
        Some(draft_id),
        Some(expected_version),
    )?;
    persist_agent_revision(
        &conn,
        project_id,
        draft_id,
        expected_version,
        base_revision,
        revised,
    )
}

fn persist_design_draft_edit(
    conn: &Connection,
    project_id: i64,
    draft_id: i64,
    expected_version: i64,
    draft: DesignDraft,
) -> Result<StoredDesignDraft, String> {
    validate_design_draft(&draft)?;
    ensure_project(&conn, project_id)?;
    let current_revision: i64 = conn
        .query_row(
            "SELECT revision FROM project_maps WHERE project_id = ?1",
            [project_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let serialized = serde_json::to_string(&draft).map_err(|error| error.to_string())?;
    let updated = conn
        .execute(
            "UPDATE design_drafts SET draft = ?1, version = version + 1
             WHERE id = ?2 AND project_id = ?3 AND state = 'pending'
               AND version = ?4 AND base_revision = ?5
               AND EXISTS (SELECT 1 FROM project_maps WHERE project_id = ?3 AND revision = ?5)",
            rusqlite::params![
                serialized,
                draft_id,
                project_id,
                expected_version,
                current_revision
            ],
        )
        .map_err(|error| error.to_string())?;
    if updated != 1 {
        return Err("Draft or confirmed map changed; reload before saving edits".into());
    }
    Ok(StoredDesignDraft {
        id: draft_id,
        project_id,
        base_revision: current_revision,
        version: expected_version + 1,
        draft,
    })
}

fn persist_agent_revision(
    conn: &Connection,
    project_id: i64,
    draft_id: i64,
    expected_version: i64,
    base_revision: i64,
    revised: DesignDraft,
) -> Result<StoredDesignDraft, String> {
    validate_design_draft(&revised)?;
    let serialized = serde_json::to_string(&revised).map_err(|error| error.to_string())?;
    let updated = conn
        .execute(
            "UPDATE design_drafts SET draft = ?1, version = version + 1
             WHERE id = ?2 AND project_id = ?3 AND state = 'pending'
               AND version = ?4 AND base_revision = ?5
               AND EXISTS (SELECT 1 FROM project_maps WHERE project_id = ?3 AND revision = ?5)",
            rusqlite::params![
                serialized,
                draft_id,
                project_id,
                expected_version,
                base_revision
            ],
        )
        .map_err(|error| error.to_string())?;
    if updated != 1 {
        return Err(
            "Draft changed while the Agent was revising it; newer edits were preserved".into(),
        );
    }
    Ok(StoredDesignDraft {
        id: draft_id,
        project_id,
        base_revision,
        version: expected_version + 1,
        draft: revised,
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(agent::AgentManager::default())
        .invoke_handler(tauri::generate_handler![
            runtime_info,
            pick_project,
            open_project,
            recent_projects,
            reopen_project,
            project_has_content,
            load_project_map,
            create_map_edit_draft,
            create_design_draft,
            pending_design_draft,
            development_ready,
            tickets::project_ticket_board,
            tickets::create_feature,
            tickets::update_feature,
            tickets::delete_feature,
            tickets::move_feature,
            tickets::create_ticket,
            tickets::update_ticket,
            tickets::delete_ticket,
            tickets::move_ticket,
            tickets::set_ticket_status,
            tickets::ticket_run_review,
            tickets::save_ticket_review,
            update_design_draft,
            resolve_design_draft,
            revise_design_draft,
            agent::agent_status,
            agent::send_design_message,
            agent::plan_feature_tickets,
            agent::start_ticket_development,
            agent::cancel_agent_turn,
            agent::answer_agent_approval,
            map_proposals::propose_map_update,
            map_proposals::pending_map_proposals,
            map_proposals::resolve_map_proposal
        ])
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_map() -> ProjectMap {
        ProjectMap {
            nodes: vec![
                json!({"id":"a","type":"component","position":{"x":0,"y":0},"data":{"title":"A","summary":"Owns A within its defined scope","responsibilities":["Provide A"],"status":"Not started"}}),
            ],
            edges: vec![],
            revision: 0,
        }
    }

    #[test]
    fn invalid_edges_and_duplicate_nodes_are_rejected() {
        let mut map = sample_map();
        map.edges.push(
            json!({"id":"e","source":"a","target":"missing","data":{"relation":"depends_on"}}),
        );
        assert!(validate_map(&map.nodes, &map.edges).is_err());
        map.edges.clear();
        map.nodes.push(map.nodes[0].clone());
        assert!(validate_map(&map.nodes, &map.edges).is_err());
    }

    #[test]
    fn map_edges_require_a_supported_relation_between_components() {
        let mut map = sample_map();
        map.nodes.push(json!({"id":"b","type":"component","position":{"x":1,"y":0},"data":{"title":"B","summary":"Owns B within its defined scope","responsibilities":["Provide B"],"status":"Not started"}}));
        map.edges
            .push(json!({"id":"e","source":"a","target":"b","data":{"relation":"depends_on"}}));
        assert!(validate_map(&map.nodes, &map.edges).is_ok());

        map.edges[0]["data"]["relation"] = json!("related");
        assert!(validate_map(&map.nodes, &map.edges).is_err());
        map.edges[0]["data"]["relation"] = json!("interacts_with");
        assert!(validate_map(&map.nodes, &map.edges).is_ok());
        map.edges[0].as_object_mut().unwrap().remove("data");
        assert!(validate_map(&map.nodes, &map.edges).is_err());

        map.edges[0]["data"] = json!({"relation":"interacts_with"});
        map.edges[0]["target"] = json!("layer-a");
        map.nodes.push(json!({"id":"layer-a","type":"layer","position":{"x":0,"y":0},"data":{"label":"Group","tone":"blue"}}));
        assert!(validate_map(&map.nodes, &map.edges).is_err());
    }

    #[test]
    fn design_draft_requires_valid_nodes_and_edges() {
        let node = json!({"id":"n1","type":"component","position":{"x":0,"y":0},"data":{"title":"Notes","summary":"Manage notes within the note-taking scope","responsibilities":["Create and edit notes"],"status":"Not started"}});
        let mut draft = DesignDraft {
            goal: "Build a note app".into(),
            nodes: vec![node.clone()],
            edges: vec![],
            questions: vec![],
            assumptions: vec![],
        };
        assert!(validate_design_draft(&draft).is_ok());
        draft.edges.push(
            json!({"id":"e1","source":"n1","target":"missing","data":{"relation":"depends_on"}}),
        );
        assert!(validate_design_draft(&draft).is_err());
        draft.edges.clear();
        draft.nodes[0]["data"]["responsibilities"] = json!([]);
        assert!(validate_design_draft(&draft).is_err());
    }

    #[test]
    fn design_questions_require_two_distinct_options_and_one_recommendation() {
        let node = json!({
            "id":"n1",
            "type":"component",
            "position":{"x":0,"y":0},
            "data":{
                "title":"Storage",
                "summary":"Persist application records",
                "responsibilities":["Store records"],
                "status":"Not started",
                "questions":[{
                    "prompt":"Where should records be stored?",
                    "options":[
                        {"label":"Local database","recommended":true},
                        {"label":"Remote service","recommended":false}
                    ]
                }]
            }
        });
        let mut draft = DesignDraft {
            goal: "Build an application".into(),
            nodes: vec![node],
            edges: vec![],
            questions: vec![],
            assumptions: vec![],
        };
        assert!(validate_design_draft(&draft).is_ok());

        draft.nodes[0]["data"]["questions"][0]["options"][1]["recommended"] = json!(true);
        assert!(validate_design_draft(&draft).is_err());
        draft.nodes[0]["data"]["questions"][0]["options"][1]["recommended"] = json!(false);
        draft.nodes[0]["data"]["questions"][0]["options"][1]["label"] = json!("Local database");
        assert!(validate_design_draft(&draft).is_err());
    }

    #[test]
    fn unanswered_decision_blocks_development_until_an_option_or_custom_answer_is_saved() {
        let mut node = json!({
            "data": {
                "questions": [{"prompt":"Where should data live?","options":[
                    {"label":"Local","recommended":true},
                    {"label":"Remote","recommended":false}
                ]}]
            }
        });
        assert!(node_has_unanswered_questions(&node));
        node["data"]["answers"] = json!({"Where should data live?":"Remote"});
        assert!(!node_has_unanswered_questions(&node));
        node["data"]["answers"]["Where should data live?"] = json!("  ");
        assert!(node_has_unanswered_questions(&node));
        node["data"]["answers"]["Where should data live?"] =
            json!("Use the customer's existing database");
        assert!(!node_has_unanswered_questions(&node));
    }

    #[test]
    fn map_edits_do_not_change_confirmed_state_until_approved() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        let root = canonical_project(&std::env::current_dir().unwrap()).unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, root) VALUES (1, 'test', ?1)",
            [&root],
        )
        .unwrap();
        conn.execute("INSERT INTO project_maps (project_id) VALUES (1)", [])
            .unwrap();

        let initial = sample_map();
        assert_eq!(save_map(&conn, 1, &initial).unwrap(), 1);
        let accepted_design = DesignDraft {
            goal: "Architecture baseline".into(),
            nodes: initial.nodes.clone(),
            edges: initial.edges.clone(),
            questions: vec![],
            assumptions: vec![],
        };
        conn.execute(
            "INSERT INTO design_drafts (project_id, base_revision, version, draft, state) VALUES (1, 0, 1, ?1, 'accepted')",
            [serde_json::to_string(&accepted_design).unwrap()],
        )
        .unwrap();
        assert!(has_current_approved_map(&conn, 1).unwrap());

        let mut first_edit = ProjectMap {
            nodes: initial.nodes.clone(),
            edges: initial.edges.clone(),
            revision: 1,
        };
        first_edit.nodes[0]["data"]["summary"] = json!("A revised scope");
        let repeated_edit = ProjectMap {
            nodes: first_edit.nodes.clone(),
            edges: first_edit.edges.clone(),
            revision: first_edit.revision,
        };
        let pending = persist_map_edit_draft(&mut conn, 1, first_edit).unwrap();
        assert_eq!(pending.draft.base_revision, 1);
        assert_eq!(pending.confirmed_map.revision, 1);
        assert_eq!(
            pending.confirmed_map.nodes[0]["data"]["summary"],
            "Owns A within its defined scope"
        );
        assert_eq!(
            pending.draft.draft.nodes[0]["data"]["summary"],
            "A revised scope"
        );
        assert!(persist_map_edit_draft(&mut conn, 1, repeated_edit).is_err());
        assert!(has_current_approved_map(&conn, 1).unwrap());

        assert!(
            resolve_design_draft_in_db(
                &mut conn,
                1,
                pending.draft.id,
                pending.draft.version,
                false
            )
            .unwrap()
            .is_none()
        );
        assert!(has_current_approved_map(&conn, 1).unwrap());

        let mut second_edit = ProjectMap {
            nodes: initial.nodes,
            edges: initial.edges,
            revision: 1,
        };
        second_edit.nodes[0]["data"]["summary"] = json!("Approved scope");
        let pending = persist_map_edit_draft(&mut conn, 1, second_edit).unwrap();
        let approved =
            resolve_design_draft_in_db(&mut conn, 1, pending.draft.id, pending.draft.version, true)
                .unwrap()
                .unwrap();
        assert_eq!(approved.revision, 2);
        assert_eq!(approved.nodes[0]["data"]["summary"], "Approved scope");
        assert!(has_current_approved_map(&conn, 1).unwrap());
    }

    #[test]
    fn design_context_uses_persisted_revision_and_marks_pending_draft_unconfirmed() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        let root = canonical_project(&std::env::current_dir().unwrap()).unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, root) VALUES (1, 'test', ?1)",
            [&root],
        )
        .unwrap();
        conn.execute("INSERT INTO project_maps (project_id) VALUES (1)", [])
            .unwrap();
        let mut map = sample_map();
        map.nodes[0]["id"] = json!("confirmed-node");
        map.revision = save_map(&conn, 1, &map).unwrap();
        let draft = DesignDraft {
            goal: "Unconfirmed change".into(),
            nodes: vec![
                json!({"id":"draft-node","type":"component","data":{"title":"Draft","summary":"Not approved","responsibilities":["Draft responsibility"],"status":"Not started"}}),
            ],
            edges: vec![],
            questions: vec![],
            assumptions: vec![],
        };
        conn.execute(
            "INSERT INTO design_drafts (project_id, base_revision, version, draft, state) VALUES (1, ?1, 2, ?2, 'pending')",
            rusqlite::params![map.revision, serde_json::to_string(&draft).unwrap()],
        )
        .unwrap();

        let (serialized_context, context_revision) = agent::design_context(&conn, 1).unwrap();
        let context: Value = serde_json::from_str(&serialized_context).unwrap();
        assert_eq!(context_revision, map.revision);
        assert_eq!(context["mode"], "design");
        assert_eq!(context["confirmedMap"]["revision"], map.revision);
        assert_eq!(context["confirmedMap"]["nodes"][0]["id"], "confirmed-node");
        assert_eq!(context["pendingDraft"]["baseRevision"], map.revision);
        assert_eq!(context["pendingDraft"]["version"], 2);
        assert_eq!(context["pendingDraft"]["current"], true);
        assert_eq!(
            context["pendingDraft"]["draft"]["nodes"][0]["id"],
            "draft-node"
        );
        assert!(context["rules"].as_array().unwrap().iter().any(|rule| {
            rule.as_str()
                .is_some_and(|rule| rule.contains("unconfirmed"))
        }));
    }

    #[test]
    fn design_response_requires_responsibilities_and_typed_edges() {
        let response = json!({
            "contractVersion":1,
            "mode":"design",
            "context":{"mapRevision":0,"draftId":null,"draftVersion":null,"unitId":null},
            "result":{"kind":"map_draft","message":"Proposed a notes architecture.","draft":{
                "goal":"Build a note app",
                "nodes":[
                    {"id":"notes","title":"Notes","summary":"Owns creating and organizing notes, not account billing.","responsibilities":["Create and edit notes"],"questions":[]},
                    {"id":"storage","title":"Storage","summary":"Persists note data, not presentation.","responsibilities":["Persist notes"],"questions":[]}
                ],
                "edges":[{"id":"notes-storage","source":"notes","target":"storage","relation":"depends_on"}],
                "questions":[],
                "assumptions":[]
            }}
        }).to_string();
        let draft = parse_design_response(&response, 0, None, None).unwrap();
        assert_eq!(
            draft.nodes[0]["data"]["responsibilities"][0],
            "Create and edit notes"
        );
        assert_eq!(draft.nodes[1]["position"]["x"], 380);
        assert_eq!(draft.edges[0]["data"]["relation"], "depends_on");
        assert_eq!(draft.edges[0]["label"], "Depends on");
        assert_eq!(draft.edges[0]["sourceHandle"], "source-right");
        assert_eq!(draft.edges[0]["targetHandle"], "target-left");

        let invalid_response = response.replace("depends_on", "related");
        assert!(parse_design_response(&invalid_response, 0, None, None).is_err());
        assert!(parse_design_response(&response, 1, None, None).is_err());
    }

    #[test]
    fn design_drafts_persist_without_changing_confirmed_map() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        let root = canonical_project(&std::env::current_dir().unwrap()).unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, root) VALUES (1, 'test', ?1)",
            [&root],
        )
        .unwrap();
        conn.execute("INSERT INTO project_maps (project_id) VALUES (1)", [])
            .unwrap();
        assert!(!has_current_approved_map(&conn, 1).unwrap());
        let draft = DesignDraft {
            goal: "Build a note app".into(),
            nodes: vec![
                json!({"id":"n1","type":"component","position":{"x":0,"y":0},"data":{"title":"Notes","summary":"Manage notes within the note-taking scope","responsibilities":["Create and edit notes"],"status":"Not started"}}),
            ],
            edges: vec![],
            questions: vec![],
            assumptions: vec![],
        };
        conn.execute("INSERT INTO design_drafts (project_id, base_revision, version, draft) VALUES (1, 0, 1, ?1)", [serde_json::to_string(&draft).unwrap()]).unwrap();
        let (nodes, revision): (String, i64) = conn
            .query_row(
                "SELECT nodes, revision FROM project_maps WHERE project_id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(nodes, "[]");
        assert_eq!(revision, 0);
        let saved: String = conn
            .query_row(
                "SELECT draft FROM design_drafts WHERE project_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<DesignDraft>(&saved)
                .unwrap()
                .nodes
                .len(),
            1
        );
    }

    #[test]
    fn draft_edits_increment_version_and_reject_stale_writes() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        let root = canonical_project(&std::env::current_dir().unwrap()).unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, root) VALUES (1, 'test', ?1)",
            [&root],
        )
        .unwrap();
        conn.execute("INSERT INTO project_maps (project_id) VALUES (1)", [])
            .unwrap();
        let draft = DesignDraft {
            goal: "Build a note app".into(),
            nodes: vec![
                json!({"id":"n1","type":"component","position":{"x":0,"y":0},"data":{"title":"Notes","summary":"Manage notes within the note-taking scope","responsibilities":["Create and edit notes"],"status":"Not started"}}),
            ],
            edges: vec![],
            questions: vec![],
            assumptions: vec![],
        };
        conn.execute("INSERT INTO design_drafts (project_id, base_revision, version, draft) VALUES (1, 0, 1, ?1)", [serde_json::to_string(&draft).unwrap()]).unwrap();
        let id = conn.last_insert_rowid();
        let mut edited = draft.clone();
        edited.nodes[0]["data"]["title"] = json!("Personal notes");
        let saved = persist_design_draft_edit(&conn, 1, id, 1, edited.clone()).unwrap();
        assert_eq!(saved.version, 2);
        assert_eq!(saved.draft.nodes[0]["data"]["title"], "Personal notes");
        assert!(persist_design_draft_edit(&conn, 1, id, 1, edited).is_err());
        let mut stale_agent_response = draft.clone();
        stale_agent_response.nodes[0]["data"]["title"] = json!("Stale agent title");
        assert!(persist_agent_revision(&conn, 1, id, 1, 0, stale_agent_response).is_err());
        let saved_draft: String = conn
            .query_row(
                "SELECT draft FROM design_drafts WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<DesignDraft>(&saved_draft)
                .unwrap()
                .nodes[0]["data"]["title"],
            "Personal notes"
        );
        let (nodes, revision): (String, i64) = conn
            .query_row(
                "SELECT nodes, revision FROM project_maps WHERE project_id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(nodes, "[]");
        assert_eq!(revision, 0);
    }

    #[test]
    fn accepting_design_draft_commits_map_and_approval_together() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        let root = canonical_project(&std::env::current_dir().unwrap()).unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, root) VALUES (1, 'test', ?1)",
            [&root],
        )
        .unwrap();
        conn.execute("INSERT INTO project_maps (project_id) VALUES (1)", [])
            .unwrap();
        assert!(!has_current_approved_map(&conn, 1).unwrap());
        let draft = DesignDraft {
            goal: "Build a note app".into(),
            nodes: vec![
                json!({"id":"n1","type":"component","position":{"x":0,"y":0},"data":{"title":"Notes","summary":"Manage notes within the note-taking scope","responsibilities":["Create and edit notes"],"status":"Not started"}}),
            ],
            edges: vec![],
            questions: vec![],
            assumptions: vec![],
        };
        conn.execute("INSERT INTO design_drafts (id, project_id, base_revision, version, draft) VALUES (7, 1, 0, 3, ?1)", [serde_json::to_string(&draft).unwrap()]).unwrap();
        let accepted = resolve_design_draft_in_db(&mut conn, 1, 7, 3, true)
            .unwrap()
            .unwrap();
        assert_eq!(accepted.revision, 1);
        assert_eq!(accepted.nodes.len(), 1);
        let state: String = conn
            .query_row("SELECT state FROM design_drafts WHERE id = 7", [], |row| {
                row.get(0)
            })
            .unwrap();
        let (saved_nodes, revision): (String, i64) = conn
            .query_row(
                "SELECT nodes, revision FROM project_maps WHERE project_id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, "accepted");
        assert_eq!(revision, 1);
        assert_eq!(
            serde_json::from_str::<Vec<Value>>(&saved_nodes)
                .unwrap()
                .len(),
            1
        );
        assert!(has_current_approved_map(&conn, 1).unwrap());
        assert!(resolve_design_draft_in_db(&mut conn, 1, 7, 3, true).is_err());
        save_map(
            &conn,
            1,
            &ProjectMap {
                nodes: accepted.nodes,
                edges: accepted.edges,
                revision: accepted.revision,
            },
        )
        .unwrap();
        assert!(has_current_approved_map(&conn, 1).unwrap());
    }

    #[test]
    fn rejecting_design_draft_keeps_confirmed_map_empty() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        let root = canonical_project(&std::env::current_dir().unwrap()).unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, root) VALUES (1, 'test', ?1)",
            [&root],
        )
        .unwrap();
        conn.execute("INSERT INTO project_maps (project_id) VALUES (1)", [])
            .unwrap();
        let draft = DesignDraft {
            goal: "Build a note app".into(),
            nodes: vec![
                json!({"id":"n1","type":"component","position":{"x":0,"y":0},"data":{"title":"Notes","summary":"Manage notes within the note-taking scope","responsibilities":["Create and edit notes"],"status":"Not started"}}),
            ],
            edges: vec![],
            questions: vec![],
            assumptions: vec![],
        };
        conn.execute("INSERT INTO design_drafts (id, project_id, base_revision, version, draft) VALUES (7, 1, 0, 1, ?1)", [serde_json::to_string(&draft).unwrap()]).unwrap();
        assert!(
            resolve_design_draft_in_db(&mut conn, 1, 7, 1, false)
                .unwrap()
                .is_none()
        );
        let state: String = conn
            .query_row("SELECT state FROM design_drafts WHERE id = 7", [], |row| {
                row.get(0)
            })
            .unwrap();
        let (nodes, revision): (String, i64) = conn
            .query_row(
                "SELECT nodes, revision FROM project_maps WHERE project_id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, "rejected");
        assert_eq!(nodes, "[]");
        assert_eq!(revision, 0);
    }

    #[test]
    fn saving_checks_revision_without_overwriting_previous_data() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        let root = canonical_project(&std::env::current_dir().unwrap()).unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, root) VALUES (1, 'test', ?1)",
            [&root],
        )
        .unwrap();
        conn.execute("INSERT INTO project_maps (project_id) VALUES (1)", [])
            .unwrap();
        let mut map = sample_map();
        assert_eq!(save_map(&conn, 1, &map).unwrap(), 1);
        map.nodes.clear();
        assert!(save_map(&conn, 1, &map).is_err());
        let saved: String = conn
            .query_row(
                "SELECT nodes FROM project_maps WHERE project_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(serde_json::from_str::<Vec<Value>>(&saved).unwrap().len(), 1);
    }
}
