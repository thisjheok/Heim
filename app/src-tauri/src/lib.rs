use std::{collections::HashSet, fs, path::Path};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
        if source == target || !ids.contains(source) || !ids.contains(target) {
            return Err("Edge references an invalid node".into());
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

#[tauri::command]
fn save_project_map(
    app: tauri::AppHandle,
    project_id: i64,
    map: ProjectMap,
) -> Result<i64, String> {
    let conn = database(&app)?;
    save_map(&conn, project_id, &map)
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
            load_project_map,
            save_project_map,
            agent::agent_status,
            agent::send_agent_message,
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
            nodes: vec![json!({"id":"a","type":"component","position":{"x":0,"y":0},"data":{}})],
            edges: vec![],
            revision: 0,
        }
    }

    #[test]
    fn invalid_edges_and_duplicate_nodes_are_rejected() {
        let mut map = sample_map();
        map.edges
            .push(json!({"id":"e","source":"a","target":"missing"}));
        assert!(validate_map(&map.nodes, &map.edges).is_err());
        map.edges.clear();
        map.nodes.push(map.nodes[0].clone());
        assert!(validate_map(&map.nodes, &map.edges).is_err());
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
