use std::{collections::HashSet, fs, path::Path};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::Manager;

mod agent;
mod map_proposals;

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
    CREATE TABLE IF NOT EXISTS development_units (
        id INTEGER PRIMARY KEY,
        project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
        map_revision INTEGER NOT NULL,
        node_id TEXT NOT NULL,
        goal TEXT NOT NULL,
        completion_criteria TEXT NOT NULL,
        status TEXT NOT NULL DEFAULT 'confirmed'
            CHECK(status IN ('confirmed', 'started', 'start_failed')),
        status_detail TEXT,
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        started_at TEXT
    );
    CREATE TABLE IF NOT EXISTS development_unit_runs (
        id INTEGER PRIMARY KEY,
        unit_id INTEGER NOT NULL REFERENCES development_units(id) ON DELETE CASCADE,
        state TEXT NOT NULL CHECK(state IN (
            'starting', 'running', 'finished', 'interrupted', 'failed', 'disconnected', 'start_failed'
        )),
        detail TEXT,
        started_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        finished_at TEXT
    );
    CREATE TABLE IF NOT EXISTS development_unit_specs (
        project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
        map_revision INTEGER NOT NULL,
        node_id TEXT NOT NULL,
        goal TEXT NOT NULL,
        completion_criteria TEXT NOT NULL,
        source TEXT NOT NULL CHECK(source IN ('agent', 'user')),
        version INTEGER NOT NULL DEFAULT 1,
        updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        PRIMARY KEY(project_id, map_revision, node_id)
    );
    CREATE TABLE IF NOT EXISTS development_unit_run_reports (
        run_id INTEGER PRIMARY KEY REFERENCES development_unit_runs(id) ON DELETE CASCADE,
        outcome TEXT NOT NULL CHECK(outcome IN ('completed', 'incomplete', 'blocked')),
        summary TEXT NOT NULL,
        report TEXT NOT NULL,
        saved_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
    CREATE TABLE IF NOT EXISTS development_unit_run_evidence (
        id INTEGER PRIMARY KEY,
        run_id INTEGER NOT NULL REFERENCES development_unit_runs(id) ON DELETE CASCADE,
        kind TEXT NOT NULL CHECK(kind IN ('command_execution', 'workspace_snapshot')),
        evidence TEXT NOT NULL,
        observed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
    CREATE TABLE IF NOT EXISTS development_unit_reviews (
        id INTEGER PRIMARY KEY,
        unit_id INTEGER NOT NULL REFERENCES development_units(id) ON DELETE CASCADE,
        run_id INTEGER NOT NULL REFERENCES development_unit_runs(id) ON DELETE CASCADE,
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
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
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

#[tauri::command]
fn save_development_unit_spec(
    app: tauri::AppHandle,
    project_id: i64,
    map_revision: i64,
    node_id: String,
    expected_version: i64,
    goal: String,
    completion_criteria: Vec<String>,
) -> Result<DevelopmentUnitSpec, String> {
    validate_development_unit_spec(&goal, &completion_criteria)?;
    let conn = database(&app)?;
    ensure_project(&conn, project_id)?;
    let (nodes_json, revision): (String, i64) = conn
        .query_row(
            "SELECT nodes, revision FROM project_maps WHERE project_id = ?1",
            [project_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| error.to_string())?;
    if revision != map_revision || !has_current_approved_map(&conn, project_id)? {
        return Err("Approved map changed; reload the node plan before saving".into());
    }
    let has_pending_draft: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM design_drafts WHERE project_id = ?1 AND state = 'pending')",
            [project_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if has_pending_draft {
        return Err("Resolve the pending design draft before editing a development plan".into());
    }
    let nodes: Vec<Value> = serde_json::from_str(&nodes_json).map_err(|error| error.to_string())?;
    let node = nodes
        .iter()
        .find(|node| {
            node.get("id").and_then(Value::as_str) == Some(node_id.as_str())
                && node.get("type").and_then(Value::as_str) == Some("component")
        })
        .ok_or("Selected responsibility is not in the confirmed map")?;
    if node_has_unanswered_questions(node) {
        return Err(
            "Answer the selected node's open questions before editing its development plan".into(),
        );
    }
    let serialized =
        serde_json::to_string(&completion_criteria).map_err(|error| error.to_string())?;
    let updated = conn
        .execute(
            "UPDATE development_unit_specs
             SET goal = ?1, completion_criteria = ?2, source = 'user',
                 version = version + 1, updated_at = CURRENT_TIMESTAMP
             WHERE project_id = ?3 AND map_revision = ?4 AND node_id = ?5 AND version = ?6",
            rusqlite::params![
                goal.trim(),
                serialized,
                project_id,
                map_revision,
                node_id,
                expected_version
            ],
        )
        .map_err(|error| error.to_string())?;
    if updated != 1 {
        return Err("Development plan changed; reload it before saving edits".into());
    }
    load_development_unit_spec(&conn, project_id, map_revision, &node_id)?
        .ok_or_else(|| "Saved development plan is unavailable".to_owned())
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

pub(crate) struct DevelopmentUnit {
    pub id: i64,
    pub map_revision: i64,
    pub node_id: String,
    pub goal: String,
    pub completion_criteria: Vec<String>,
    pub selected_node: Value,
    pub related_nodes: Vec<Value>,
    pub related_edges: Vec<Value>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevelopmentUnitSpec {
    pub project_id: i64,
    pub map_revision: i64,
    pub node_id: String,
    pub goal: String,
    pub completion_criteria: Vec<String>,
    pub source: String,
    pub version: i64,
    pub updated_at: String,
}

pub(crate) fn validate_development_unit_spec(
    goal: &str,
    criteria: &[String],
) -> Result<(), String> {
    if goal.trim().is_empty() || goal.trim().len() > 4_000 {
        return Err("Unit goal must contain 1-4000 characters".into());
    }
    if criteria.is_empty()
        || criteria.len() > 20
        || criteria
            .iter()
            .any(|criterion| criterion.trim().is_empty() || criterion.len() > 1_000)
    {
        return Err("Provide 1-20 completion criteria, each containing 1-1000 characters".into());
    }
    Ok(())
}

pub(crate) fn load_development_unit_spec(
    conn: &Connection,
    project_id: i64,
    map_revision: i64,
    node_id: &str,
) -> Result<Option<DevelopmentUnitSpec>, String> {
    conn.query_row(
        "SELECT goal, completion_criteria, source, version, updated_at
         FROM development_unit_specs WHERE project_id = ?1 AND map_revision = ?2 AND node_id = ?3",
        rusqlite::params![project_id, map_revision, node_id],
        |row| {
            let criteria: String = row.get(1)?;
            Ok(DevelopmentUnitSpec {
                project_id,
                map_revision,
                node_id: node_id.to_owned(),
                goal: row.get(0)?,
                completion_criteria: serde_json::from_str(&criteria).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
                source: row.get(2)?,
                version: row.get(3)?,
                updated_at: row.get(4)?,
            })
        },
    )
    .optional()
    .map_err(|error| error.to_string())
}

pub(crate) fn store_generated_development_unit_spec(
    conn: &mut Connection,
    spec: &DevelopmentUnitSpec,
) -> Result<DevelopmentUnitSpec, String> {
    validate_development_unit_spec(&spec.goal, &spec.completion_criteria)?;
    let criteria =
        serde_json::to_string(&spec.completion_criteria).map_err(|error| error.to_string())?;
    let tx = conn.transaction().map_err(|error| error.to_string())?;
    let current_revision: i64 = tx
        .query_row(
            "SELECT revision FROM project_maps WHERE project_id = ?1",
            [spec.project_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if current_revision != spec.map_revision || !has_current_approved_map(&tx, spec.project_id)? {
        return Err(
            "Approved map changed while generating the unit plan; select the node again".into(),
        );
    }
    let pending: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM design_drafts WHERE project_id = ?1 AND state = 'pending')",
            [spec.project_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if pending {
        return Err("Resolve the pending design draft before generating a development plan".into());
    }
    tx.execute(
        "INSERT INTO development_unit_specs
            (project_id, map_revision, node_id, goal, completion_criteria, source)
         VALUES (?1, ?2, ?3, ?4, ?5, 'agent')
         ON CONFLICT(project_id, map_revision, node_id) DO NOTHING",
        rusqlite::params![
            spec.project_id,
            spec.map_revision,
            spec.node_id,
            spec.goal.trim(),
            criteria
        ],
    )
    .map_err(|error| error.to_string())?;
    let stored =
        load_development_unit_spec(&tx, spec.project_id, spec.map_revision, &spec.node_id)?
            .ok_or("Generated unit plan was not stored")?;
    tx.commit().map_err(|error| error.to_string())?;
    Ok(stored)
}

pub(crate) fn node_has_unanswered_questions(node: &Value) -> bool {
    let Some(questions) = node.pointer("/data/questions").and_then(Value::as_array) else {
        return false;
    };
    questions.iter().any(|question| {
        let Some(question) = question.as_str() else {
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

pub(crate) fn record_development_unit(
    conn: &mut Connection,
    project_id: i64,
    expected_revision: i64,
    node_id: &str,
    goal: &str,
    completion_criteria: &[String],
) -> Result<DevelopmentUnit, String> {
    let goal = goal.trim();
    if goal.is_empty() || goal.len() > 4_000 {
        return Err("Development-unit goal must contain 1-4000 characters".into());
    }
    if completion_criteria.is_empty()
        || completion_criteria.len() > 20
        || completion_criteria
            .iter()
            .any(|criterion| criterion.trim().is_empty() || criterion.len() > 1_000)
    {
        return Err("Provide 1-20 completion criteria, each containing 1-1000 characters".into());
    }
    if node_id.trim().is_empty() || node_id.len() > 128 {
        return Err("Invalid development-unit node".into());
    }
    ensure_project(conn, project_id)?;
    let tx = conn.transaction().map_err(|error| error.to_string())?;
    let (nodes_json, edges_json, map_revision): (String, String, i64) = tx
        .query_row(
            "SELECT nodes, edges, revision FROM project_maps WHERE project_id = ?1",
            [project_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| error.to_string())?;
    if map_revision != expected_revision {
        return Err("Approved map changed; review it and define the unit again".into());
    }
    if !has_current_approved_map(&tx, project_id)? {
        return Err("Approve a design map before confirming a development unit".into());
    }
    let nodes: Vec<Value> = serde_json::from_str(&nodes_json).map_err(|error| error.to_string())?;
    let edges: Vec<Value> = serde_json::from_str(&edges_json).map_err(|error| error.to_string())?;
    let selected_node = nodes
        .iter()
        .find(|node| {
            node.get("id").and_then(Value::as_str) == Some(node_id)
                && node.get("type").and_then(Value::as_str) == Some("component")
        })
        .ok_or("Select a system-responsibility node from the approved map")?;
    if node_has_unanswered_questions(selected_node) {
        return Err("Answer all questions on this node before starting development".into());
    }
    let mut related_ids = HashSet::new();
    for edge in &edges {
        let source = edge.get("source").and_then(Value::as_str);
        let target = edge.get("target").and_then(Value::as_str);
        if source == Some(node_id) {
            if let Some(target) = target {
                related_ids.insert(target);
            }
        } else if target == Some(node_id) {
            if let Some(source) = source {
                related_ids.insert(source);
            }
        }
    }
    let related_nodes = nodes
        .iter()
        .filter(|node| {
            node.get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| related_ids.contains(id))
        })
        .cloned()
        .collect::<Vec<_>>();
    let related_edges = edges
        .iter()
        .filter(|edge| {
            edge.get("source").and_then(Value::as_str) == Some(node_id)
                || edge.get("target").and_then(Value::as_str) == Some(node_id)
        })
        .cloned()
        .collect::<Vec<_>>();
    let completion_criteria = completion_criteria
        .iter()
        .map(|criterion| criterion.trim().to_owned())
        .collect::<Vec<_>>();
    let serialized_criteria =
        serde_json::to_string(&completion_criteria).map_err(|error| error.to_string())?;
    tx.execute(
        "INSERT INTO development_units (project_id, map_revision, node_id, goal, completion_criteria)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![project_id, map_revision, node_id, goal, serialized_criteria],
    )
    .map_err(|error| error.to_string())?;
    let id = tx.last_insert_rowid();
    let unit = DevelopmentUnit {
        id,
        map_revision,
        node_id: node_id.to_owned(),
        goal: goal.to_owned(),
        completion_criteria,
        selected_node: selected_node.clone(),
        related_nodes,
        related_edges,
    };
    tx.commit().map_err(|error| error.to_string())?;
    Ok(unit)
}

pub(crate) fn update_development_unit_status(
    conn: &Connection,
    unit_id: i64,
    status: &str,
    detail: Option<&str>,
) -> Result<(), String> {
    if !matches!(status, "started" | "start_failed") {
        return Err("Invalid development-unit status transition".into());
    }
    let detail = detail.map(|value| value.chars().take(1_000).collect::<String>());
    let updated = conn
        .execute(
            "UPDATE development_units
             SET status = ?1, status_detail = ?2,
                 started_at = CASE WHEN ?1 = 'started' THEN CURRENT_TIMESTAMP ELSE started_at END
             WHERE id = ?3 AND status IN ('confirmed', 'started')",
            rusqlite::params![status, detail.as_deref(), unit_id],
        )
        .map_err(|error| error.to_string())?;
    if updated != 1 {
        return Err("Development unit is no longer awaiting start".into());
    }
    Ok(())
}

pub(crate) fn create_development_unit_run(conn: &Connection, unit_id: i64) -> Result<i64, String> {
    conn.execute(
        "INSERT INTO development_unit_runs (unit_id, state) VALUES (?1, 'starting')",
        [unit_id],
    )
    .map_err(|error| error.to_string())?;
    Ok(conn.last_insert_rowid())
}

pub(crate) fn transition_development_unit_run(
    conn: &Connection,
    run_id: i64,
    state: &str,
    detail: Option<&str>,
) -> Result<bool, String> {
    if !matches!(
        state,
        "running" | "finished" | "interrupted" | "failed" | "disconnected" | "start_failed"
    ) {
        return Err("Invalid development-run state".into());
    }
    let detail = detail.map(|value| value.chars().take(1_000).collect::<String>());
    let updated = if state == "running" {
        conn.execute(
            "UPDATE development_unit_runs SET state = 'running', detail = NULL
             WHERE id = ?1 AND state IN ('starting', 'running')",
            [run_id],
        )
    } else {
        conn.execute(
            "UPDATE development_unit_runs
             SET state = ?2, detail = ?3, finished_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND state IN ('starting', 'running')",
            rusqlite::params![run_id, state, detail.as_deref()],
        )
    }
    .map_err(|error| error.to_string())?;
    Ok(updated == 1)
}

pub(crate) fn save_development_unit_run_report(
    conn: &mut Connection,
    run_id: i64,
    report: &Value,
) -> Result<(), String> {
    let outcome = report
        .get("outcome")
        .and_then(Value::as_str)
        .filter(|outcome| matches!(*outcome, "completed" | "incomplete" | "blocked"))
        .ok_or("Invalid development report outcome")?;
    let summary = report
        .get("summary")
        .and_then(Value::as_str)
        .filter(|summary| !summary.trim().is_empty())
        .ok_or("Development report summary is missing")?;
    let serialized = serde_json::to_string(report).map_err(|error| error.to_string())?;
    if serialized.len() > 200_000 {
        return Err("Development report exceeds the 200000-byte storage limit".into());
    }
    let tx = conn.transaction().map_err(|error| error.to_string())?;
    let stored_criteria_json: Option<String> = tx
        .query_row(
            "SELECT u.completion_criteria FROM development_unit_runs r
             JOIN development_units u ON u.id = r.unit_id
             WHERE r.id = ?1 AND r.state IN ('starting', 'running')",
            [run_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some(stored_criteria_json) = stored_criteria_json else {
        return Err("Development report does not match an active run".into());
    };
    let stored_criteria: Vec<String> =
        serde_json::from_str(&stored_criteria_json).map_err(|error| error.to_string())?;
    let report_criteria = report
        .get("criteria")
        .and_then(Value::as_array)
        .ok_or("Development report completion criteria are missing")?;
    if report_criteria.len() != stored_criteria.len()
        || report_criteria
            .iter()
            .enumerate()
            .any(|(index, criterion)| {
                criterion.get("index").and_then(Value::as_u64) != Some(index as u64)
            })
    {
        return Err("Development report criteria do not match the confirmed unit".into());
    }
    tx.execute(
        "INSERT INTO development_unit_run_reports (run_id, outcome, summary, report)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(run_id) DO UPDATE SET outcome = excluded.outcome,
             summary = excluded.summary, report = excluded.report, saved_at = CURRENT_TIMESTAMP",
        rusqlite::params![run_id, outcome, summary, serialized],
    )
    .map_err(|error| error.to_string())?;
    tx.commit().map_err(|error| error.to_string())
}

pub(crate) fn record_development_unit_run_evidence(
    conn: &Connection,
    run_id: i64,
    kind: &str,
    evidence: &Value,
) -> Result<(), String> {
    if !matches!(kind, "command_execution" | "workspace_snapshot") {
        return Err("Invalid development-run evidence kind".into());
    }
    let serialized = serde_json::to_string(evidence).map_err(|error| error.to_string())?;
    if serialized.len() > 200_000 {
        return Err("Development-run evidence exceeds the 200000-byte storage limit".into());
    }
    let inserted = conn
        .execute(
            "INSERT INTO development_unit_run_evidence (run_id, kind, evidence)
         SELECT ?1, ?2, ?3 WHERE EXISTS (
             SELECT 1 FROM development_unit_runs WHERE id = ?1
         )",
            rusqlite::params![run_id, kind, serialized],
        )
        .map_err(|error| error.to_string())?;
    if inserted != 1 {
        return Err("Development-run evidence references an unknown run".into());
    }
    Ok(())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredDevelopmentUnitReviewDecision {
    id: i64,
    unit_id: i64,
    run_id: i64,
    decision: String,
    note: Option<String>,
    reviewed_at: String,
}

fn save_development_unit_review_in_db(
    conn: &mut Connection,
    project_id: i64,
    unit_id: i64,
    run_id: i64,
    decision: &str,
    note: Option<&str>,
) -> Result<StoredDevelopmentUnitReviewDecision, String> {
    if !matches!(
        decision,
        "approved_completion" | "rework_requested" | "blocked" | "incomplete"
    ) {
        return Err("Invalid development review decision".into());
    }
    let note = note.map(str::trim).filter(|note| !note.is_empty());
    if note.is_some_and(|note| note.chars().count() > 2_000) {
        return Err("Review note must be at most 2000 characters".into());
    }

    let tx = conn.transaction().map_err(|error| error.to_string())?;
    ensure_project(&tx, project_id)?;
    let run_state = tx
        .query_row(
            "SELECT r.state FROM development_unit_runs r
             JOIN development_units u ON u.id = r.unit_id
             WHERE r.id = ?1 AND u.id = ?2 AND u.project_id = ?3
               AND r.id = (
                   SELECT id FROM development_unit_runs WHERE unit_id = u.id ORDER BY id DESC LIMIT 1
               )",
            rusqlite::params![run_id, unit_id, project_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .ok_or("Review must reference the selected unit's latest run")?;
    if matches!(run_state.as_str(), "starting" | "running") {
        return Err("Wait for the Agent run to finish before reviewing it".into());
    }
    tx.execute(
        "INSERT INTO development_unit_reviews (unit_id, run_id, decision, note)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![unit_id, run_id, decision, note],
    )
    .map_err(|error| error.to_string())?;
    let review = StoredDevelopmentUnitReviewDecision {
        id: tx.last_insert_rowid(),
        unit_id,
        run_id,
        decision: decision.to_owned(),
        note: note.map(str::to_owned),
        reviewed_at: tx
            .query_row(
                "SELECT reviewed_at FROM development_unit_reviews WHERE id = ?1",
                [tx.last_insert_rowid()],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?,
    };
    tx.commit().map_err(|error| error.to_string())?;
    Ok(review)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredDevelopmentUnit {
    id: i64,
    project_id: i64,
    map_revision: i64,
    node_id: String,
    goal: String,
    completion_criteria: Vec<String>,
    status: String,
    status_detail: Option<String>,
    created_at: String,
    started_at: Option<String>,
    latest_run_id: Option<i64>,
    latest_run_state: Option<String>,
    latest_run_detail: Option<String>,
    latest_run_started_at: Option<String>,
    latest_run_finished_at: Option<String>,
    review_decision: Option<String>,
    review_note: Option<String>,
    reviewed_at: Option<String>,
}

#[tauri::command]
fn project_development_units(
    app: tauri::AppHandle,
    project_id: i64,
) -> Result<Vec<StoredDevelopmentUnit>, String> {
    let conn = database(&app)?;
    project_development_units_in_db(&conn, project_id)
}

fn project_development_units_in_db(
    conn: &Connection,
    project_id: i64,
) -> Result<Vec<StoredDevelopmentUnit>, String> {
    ensure_project(&conn, project_id)?;
    let mut statement = conn
        .prepare(
            "SELECT u.id, u.map_revision, u.node_id, u.goal, u.completion_criteria, u.status,
                    u.status_detail, u.created_at, u.started_at,
                    r.id, r.state, r.detail, r.started_at, r.finished_at,
                    v.decision, v.note, v.reviewed_at
             FROM development_units u
             LEFT JOIN development_unit_runs r ON r.id = (
                 SELECT id FROM development_unit_runs WHERE unit_id = u.id ORDER BY id DESC LIMIT 1
             )
             LEFT JOIN development_unit_reviews v ON v.id = (
                 SELECT id FROM development_unit_reviews WHERE run_id = r.id ORDER BY id DESC LIMIT 1
             )
             WHERE u.project_id = ?1 ORDER BY u.id DESC",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([project_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<i64>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, Option<String>>(13)?,
                row.get::<_, Option<String>>(14)?,
                row.get::<_, Option<String>>(15)?,
                row.get::<_, Option<String>>(16)?,
            ))
        })
        .map_err(|error| error.to_string())?;
    rows.map(|row| {
        let (
            id,
            map_revision,
            node_id,
            goal,
            criteria,
            status,
            status_detail,
            created_at,
            started_at,
            latest_run_id,
            latest_run_state,
            latest_run_detail,
            latest_run_started_at,
            latest_run_finished_at,
            review_decision,
            review_note,
            reviewed_at,
        ) = row.map_err(|error| error.to_string())?;
        Ok(StoredDevelopmentUnit {
            id,
            project_id,
            map_revision,
            node_id,
            goal,
            completion_criteria: serde_json::from_str(&criteria)
                .map_err(|error| error.to_string())?,
            status,
            status_detail,
            created_at,
            started_at,
            latest_run_id,
            latest_run_state,
            latest_run_detail,
            latest_run_started_at,
            latest_run_finished_at,
            review_decision,
            review_note,
            reviewed_at,
        })
    })
    .collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DevelopmentRunEvidence {
    id: i64,
    kind: String,
    evidence: Value,
    observed_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DevelopmentRunReview {
    run_id: i64,
    unit_id: i64,
    state: String,
    detail: Option<String>,
    started_at: String,
    finished_at: Option<String>,
    report: Option<Value>,
    evidence: Vec<DevelopmentRunEvidence>,
}

fn development_unit_run_review_in_db(
    conn: &Connection,
    project_id: i64,
    run_id: i64,
) -> Result<DevelopmentRunReview, String> {
    ensure_project(conn, project_id)?;
    let (unit_id, state, detail, started_at, finished_at, report_json): (
        i64,
        String,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
    ) = conn
        .query_row(
            "SELECT r.unit_id, r.state, r.detail, r.started_at, r.finished_at, p.report
             FROM development_unit_runs r
             JOIN development_units u ON u.id = r.unit_id
             LEFT JOIN development_unit_run_reports p ON p.run_id = r.id
             WHERE r.id = ?1 AND u.project_id = ?2",
            rusqlite::params![run_id, project_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .map_err(|error| error.to_string())?;
    let report = report_json
        .map(|report| serde_json::from_str(&report).map_err(|error| error.to_string()))
        .transpose()?;
    let mut statement = conn
        .prepare(
            "SELECT id, kind, evidence, observed_at FROM development_unit_run_evidence
             WHERE run_id = ?1 ORDER BY id",
        )
        .map_err(|error| error.to_string())?;
    let evidence = statement
        .query_map([run_id], |row| {
            let serialized: String = row.get(2)?;
            Ok(DevelopmentRunEvidence {
                id: row.get(0)?,
                kind: row.get(1)?,
                evidence: serde_json::from_str(&serialized).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
                observed_at: row.get(3)?,
            })
        })
        .map_err(|error| error.to_string())?
        .map(|row| row.map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DevelopmentRunReview {
        run_id,
        unit_id,
        state,
        detail,
        started_at,
        finished_at,
        report,
        evidence,
    })
}

#[tauri::command]
fn development_unit_run_review(
    app: tauri::AppHandle,
    project_id: i64,
    run_id: i64,
) -> Result<DevelopmentRunReview, String> {
    let conn = database(&app)?;
    development_unit_run_review_in_db(&conn, project_id, run_id)
}

#[tauri::command]
fn save_development_unit_review(
    app: tauri::AppHandle,
    project_id: i64,
    unit_id: i64,
    run_id: i64,
    decision: String,
    note: Option<String>,
) -> Result<StoredDevelopmentUnitReviewDecision, String> {
    let mut conn = database(&app)?;
    save_development_unit_review_in_db(
        &mut conn,
        project_id,
        unit_id,
        run_id,
        &decision,
        note.as_deref(),
    )
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
            save_development_unit_spec,
            project_development_units,
            development_unit_run_review,
            save_development_unit_review,
            update_design_draft,
            resolve_design_draft,
            revise_design_draft,
            agent::agent_status,
            agent::send_design_message,
            agent::get_or_generate_development_unit_spec,
            agent::send_development_message,
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
    fn development_unit_pins_approved_map_and_requires_completion_criteria() {
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

        let mut map = sample_map();
        map.nodes.push(json!({"id":"storage","type":"component","position":{"x":10,"y":0},"data":{"title":"Storage","summary":"Persists records","responsibilities":["Persist records"],"status":"Not started"}}));
        map.edges.push(json!({"id":"a-storage","source":"a","target":"storage","data":{"relation":"depends_on"}}));
        assert_eq!(save_map(&conn, 1, &map).unwrap(), 1);
        let criteria = vec![
            "A record can be created".into(),
            "The record remains after reload".into(),
        ];
        assert!(
            record_development_unit(&mut conn, 1, 1, "a", "Must not start yet", &criteria,)
                .is_err()
        );
        let approved_design = DesignDraft {
            goal: "Baseline".into(),
            nodes: map.nodes.clone(),
            edges: map.edges.clone(),
            questions: vec![],
            assumptions: vec![],
        };
        conn.execute(
            "INSERT INTO design_drafts (project_id, base_revision, version, draft, state) VALUES (1, 0, 1, ?1, 'accepted')",
            [serde_json::to_string(&approved_design).unwrap()],
        )
        .unwrap();

        let generated_spec = DevelopmentUnitSpec {
            project_id: 1,
            map_revision: 1,
            node_id: "a".into(),
            goal: "Create a persistent reading record".into(),
            completion_criteria: criteria.clone(),
            source: "agent".into(),
            version: 1,
            updated_at: String::new(),
        };
        let stored_spec =
            store_generated_development_unit_spec(&mut conn, &generated_spec).unwrap();
        assert_eq!(stored_spec.source, "agent");
        assert_eq!(stored_spec.version, 1);
        let conflicting_generation = DevelopmentUnitSpec {
            goal: "Must not overwrite a saved or edited plan".into(),
            ..generated_spec.clone()
        };
        assert_eq!(
            store_generated_development_unit_spec(&mut conn, &conflicting_generation)
                .unwrap()
                .goal,
            generated_spec.goal
        );
        assert!(
            load_development_unit_spec(&conn, 1, 2, "a")
                .unwrap()
                .is_none()
        );
        assert!(validate_development_unit_spec("", &criteria).is_err());

        let unit = record_development_unit(
            &mut conn,
            1,
            1,
            "a",
            "Add persistent record creation",
            &criteria,
        )
        .unwrap();
        assert_eq!(unit.map_revision, 1);
        assert_eq!(unit.selected_node["id"], "a");
        assert_eq!(unit.completion_criteria, criteria);
        assert_eq!(unit.related_nodes.len(), 1);
        assert_eq!(unit.related_nodes[0]["id"], "storage");
        assert_eq!(unit.related_edges[0]["source"], "a");
        assert_eq!(unit.related_edges[0]["target"], "storage");
        assert_eq!(unit.related_edges[0]["data"]["relation"], "depends_on");
        let (status, stored_goal, stored_criteria): (String, String, String) = conn
            .query_row(
                "SELECT status, goal, completion_criteria FROM development_units WHERE id = ?1",
                [unit.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, "confirmed");
        assert_eq!(stored_goal, "Add persistent record creation");
        assert_eq!(
            serde_json::from_str::<Vec<String>>(&stored_criteria).unwrap(),
            criteria
        );
        assert!(update_development_unit_status(&conn, unit.id, "started", None).is_ok());
        assert!(
            update_development_unit_status(&conn, unit.id, "start_failed", Some("CLI unavailable"))
                .is_ok()
        );

        let finished_run_id = create_development_unit_run(&conn, unit.id).unwrap();
        assert!(transition_development_unit_run(&conn, finished_run_id, "running", None).unwrap());
        assert!(transition_development_unit_run(&conn, finished_run_id, "finished", None).unwrap());
        assert!(
            !transition_development_unit_run(
                &conn,
                finished_run_id,
                "disconnected",
                Some("Late disconnect")
            )
            .unwrap()
        );
        let (run_unit_id, run_state, finished_at): (i64, String, Option<String>) = conn
            .query_row(
                "SELECT unit_id, state, finished_at FROM development_unit_runs WHERE id = ?1",
                [finished_run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(run_unit_id, unit.id);
        assert_eq!(run_state, "finished");
        assert!(finished_at.is_some());
        let report_run_id = create_development_unit_run(&conn, unit.id).unwrap();
        assert!(transition_development_unit_run(&conn, report_run_id, "running", None).unwrap());
        let report = json!({
            "kind":"development_report",
            "outcome":"incomplete",
            "summary":"One check was not run.",
            "criteria":[
                {"index":0,"status":"met","evidence":["Agent report"]},
                {"index":1,"status":"not_run","evidence":["No test evidence"]}
            ],
            "changes":[{"path":"src/lib.rs","summary":"Added persistence"}],
            "checks":[{"command":"cargo test","status":"not_run","summary":"Not run"}],
            "blockers":[]
        });
        save_development_unit_run_report(&mut conn, report_run_id, &report).unwrap();
        record_development_unit_run_evidence(
            &conn,
            report_run_id,
            "command_execution",
            &json!({"command":"cargo test","status":"completed","exitCode":0}),
        )
        .unwrap();
        record_development_unit_run_evidence(
            &conn,
            report_run_id,
            "workspace_snapshot",
            &json!({"available":true,"changedFiles":["src/lib.rs"]}),
        )
        .unwrap();
        let (report_outcome, report_json): (String, String) = conn
            .query_row(
                "SELECT outcome, report FROM development_unit_run_reports WHERE run_id = ?1",
                [report_run_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(report_outcome, "incomplete");
        assert_eq!(serde_json::from_str::<Value>(&report_json).unwrap(), report);
        let evidence_kinds: Vec<String> = conn
            .prepare(
                "SELECT kind FROM development_unit_run_evidence WHERE run_id = ?1 ORDER BY kind",
            )
            .unwrap()
            .query_map([report_run_id], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            evidence_kinds,
            vec!["command_execution", "workspace_snapshot"]
        );
        let review = development_unit_run_review_in_db(&conn, 1, report_run_id).unwrap();
        assert_eq!(review.unit_id, unit.id);
        assert_eq!(review.report.as_ref().unwrap()["outcome"], "incomplete");
        assert_eq!(review.evidence.len(), 2);
        assert!(development_unit_run_review_in_db(&conn, 2, report_run_id).is_err());
        assert!(
            save_development_unit_review_in_db(
                &mut conn,
                1,
                unit.id,
                report_run_id,
                "approved_completion",
                None
            )
            .is_err()
        );
        transition_development_unit_run(&conn, report_run_id, "finished", None).unwrap();
        assert!(save_development_unit_run_report(&mut conn, report_run_id, &report).is_err());
        for (decision, note) in [
            ("approved_completion", Some("Reviewed the implementation")),
            ("rework_requested", Some("Handle the missing verification")),
            ("blocked", None),
            ("incomplete", None),
        ] {
            let saved = save_development_unit_review_in_db(
                &mut conn,
                1,
                unit.id,
                report_run_id,
                decision,
                note,
            )
            .unwrap();
            assert_eq!(saved.unit_id, unit.id);
            assert_eq!(saved.run_id, report_run_id);
            assert_eq!(saved.decision, decision);
        }
        let (latest_decision, latest_note, review_count): (String, Option<String>, i64) = conn
            .query_row(
                "SELECT decision, note,
                    (SELECT COUNT(*) FROM development_unit_reviews WHERE run_id = ?1)
                 FROM development_unit_reviews WHERE run_id = ?1 ORDER BY id DESC LIMIT 1",
                [report_run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(latest_decision, "incomplete");
        assert_eq!(latest_note, None);
        assert_eq!(review_count, 4);
        let persisted_units = project_development_units_in_db(&conn, 1).unwrap();
        let persisted_unit = persisted_units
            .iter()
            .find(|item| item.id == unit.id)
            .unwrap();
        assert_eq!(persisted_unit.latest_run_id, Some(report_run_id));
        assert_eq!(
            persisted_unit.review_decision.as_deref(),
            Some("incomplete")
        );
        assert_eq!(persisted_unit.review_note, None);
        assert!(persisted_unit.reviewed_at.is_some());
        assert!(
            save_development_unit_review_in_db(
                &mut conn,
                1,
                unit.id,
                report_run_id,
                "completed",
                None
            )
            .is_err()
        );
        assert!(
            save_development_unit_review_in_db(
                &mut conn,
                2,
                unit.id,
                report_run_id,
                "blocked",
                None
            )
            .is_err()
        );
        let confirmed_nodes: String = conn
            .query_row(
                "SELECT nodes FROM project_maps WHERE project_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&confirmed_nodes).unwrap()[0]["data"]["status"],
            "Not started"
        );
        for terminal_state in ["interrupted", "failed", "disconnected", "start_failed"] {
            let run_id = create_development_unit_run(&conn, unit.id).unwrap();
            assert!(
                transition_development_unit_run(
                    &conn,
                    run_id,
                    terminal_state,
                    Some("Observed terminal state")
                )
                .unwrap()
            );
        }
        let unit_status: String = conn
            .query_row(
                "SELECT status FROM development_units WHERE id = ?1",
                [unit.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unit_status, "start_failed");
        let unit_after_new_run = project_development_units_in_db(&conn, 1)
            .unwrap()
            .into_iter()
            .find(|item| item.id == unit.id)
            .unwrap();
        assert_eq!(
            unit_after_new_run.latest_run_state.as_deref(),
            Some("start_failed")
        );
        assert_eq!(unit_after_new_run.review_decision, None);
        assert!(
            save_development_unit_review_in_db(
                &mut conn,
                1,
                unit.id,
                report_run_id,
                "incomplete",
                None
            )
            .is_err()
        );
        let too_long_note = "x".repeat(2_001);
        let latest_run_id = unit_after_new_run.latest_run_id.unwrap();
        assert!(
            save_development_unit_review_in_db(
                &mut conn,
                1,
                unit.id,
                latest_run_id,
                "blocked",
                Some(&too_long_note)
            )
            .is_err()
        );

        assert!(record_development_unit(&mut conn, 1, 0, "a", "A stale unit", &criteria,).is_err());
        map.revision = 1;
        map.nodes[0]["data"]["questions"] = json!(["Which format?"]);
        assert_eq!(save_map(&conn, 1, &map).unwrap(), 2);
        assert!(
            record_development_unit(&mut conn, 1, 2, "a", "Resolve the format", &criteria,)
                .is_err()
        );
        assert!(record_development_unit(&mut conn, 1, 2, "a", "Missing criteria", &[]).is_err());
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
