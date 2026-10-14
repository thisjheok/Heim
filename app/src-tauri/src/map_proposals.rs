use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};
use tokio::{io::AsyncWriteExt, process::Command};

use crate::{ProjectMap, agent::codex_executable, database, ensure_project, validate_map};

const OUTPUT_SCHEMA: &str = r#"{
  "type":"object","additionalProperties":false,
  "properties":{
    "updates":{"type":"array","items":{"type":"object","additionalProperties":false,"properties":{
      "nodeId":{"type":"string"},"summary":{"type":"string"},"status":{"type":"string"},
      "files":{"type":"array","items":{"type":"string"}},"removeFiles":{"type":"array","items":{"type":"string"}},"reason":{"type":"string"},"evidence":{"type":"string"}
    },"required":["nodeId","summary","status","files","removeFiles","reason","evidence"]}},
    "additions":{"type":"array","items":{"type":"object","additionalProperties":false,"properties":{
      "title":{"type":"string"},"summary":{"type":"string"},"status":{"type":"string"},
      "files":{"type":"array","items":{"type":"string"}},"reason":{"type":"string"},"evidence":{"type":"string"}
    },"required":["title","summary","status","files","reason","evidence"]}},
    "removals":{"type":"array","items":{"type":"object","additionalProperties":false,"properties":{
      "nodeId":{"type":"string"},"deletedFile":{"type":"string"},"reason":{"type":"string"}
    },"required":["nodeId","deletedFile","reason"]}}
  },"required":["updates","additions","removals"]
}"#;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NodeUpdate {
    node_id: String,
    summary: String,
    status: String,
    files: Vec<String>,
    #[serde(default, rename = "removeFiles")]
    remove_files: Vec<String>,
    reason: String,
    evidence: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct NodeAddition {
    title: String,
    summary: String,
    status: String,
    files: Vec<String>,
    reason: String,
    evidence: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NodeRemoval {
    node_id: String,
    deleted_file: String,
    reason: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct MapChanges {
    updates: Vec<NodeUpdate>,
    additions: Vec<NodeAddition>,
    #[serde(default)]
    removals: Vec<NodeRemoval>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapProposal {
    id: i64,
    project_id: i64,
    base_revision: i64,
    changed_files: Vec<String>,
    changes: MapChanges,
}

fn source_contents(
    root: &Path,
    files: &[String],
) -> Result<(HashMap<String, String>, HashSet<String>), String> {
    let mut contents = HashMap::new();
    let mut deleted = HashSet::new();
    let mut total = 0usize;
    for file in files {
        let candidate = root.join(file);
        if !candidate.exists() {
            let mut parent = candidate.parent().ok_or("Invalid changed path")?;
            while !parent.exists() {
                parent = parent.parent().ok_or("Invalid changed path")?;
            }
            let parent = parent.canonicalize().map_err(|error| error.to_string())?;
            if !parent.starts_with(root) {
                return Err(format!("Deleted source is outside the project: {file}"));
            }
            deleted.insert(file.clone());
            continue;
        }
        let path = candidate
            .canonicalize()
            .map_err(|_| format!("Cannot inspect changed source: {file}"))?;
        if !path.starts_with(root) || !path.is_file() {
            return Err(format!("Changed source is outside the project: {file}"));
        }
        let bytes =
            std::fs::read(&path).map_err(|_| format!("Cannot read changed source: {file}"))?;
        total += bytes.len();
        if bytes.len() > 64_000 || total > 180_000 {
            return Err("Changed source is too large for map review".into());
        }
        contents.insert(
            file.clone(),
            String::from_utf8(bytes).map_err(|_| format!("Changed source is not UTF-8: {file}"))?,
        );
    }
    Ok((contents, deleted))
}

fn digest(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn verify_changes(
    map: &ProjectMap,
    changes: &MapChanges,
    contents: &HashMap<String, String>,
    deleted: &HashSet<String>,
) -> Result<(), String> {
    if changes.updates.len() + changes.additions.len() + changes.removals.len() > 50 {
        return Err("Too many proposed nodes".into());
    }
    let mut seen = HashSet::new();
    for update in &changes.updates {
        if !seen.insert(update.node_id.as_str())
            || !map
                .nodes
                .iter()
                .any(|node| node["id"] == update.node_id && node["type"] == "component")
        {
            return Err("Proposal references an unknown or duplicate node".into());
        }
    }
    for removal in &changes.removals {
        if !seen.insert(removal.node_id.as_str())
            || !deleted.contains(&removal.deleted_file)
            || removal.reason.trim().is_empty()
            || !map.nodes.iter().any(|node| {
                node["id"] == removal.node_id
                    && node["type"] == "component"
                    && node["data"]["files"]
                        .as_array()
                        .is_some_and(|files| files.len() == 1 && files[0] == removal.deleted_file)
            })
        {
            return Err("Invalid node removal".into());
        }
    }
    for update in &changes.updates {
        if update.remove_files.iter().any(|file| {
            !deleted.contains(file)
                || !map.nodes.iter().any(|node| {
                    node["id"] == update.node_id
                        && node["data"]["files"]
                            .as_array()
                            .is_some_and(|files| files.iter().any(|item| item == file))
                })
        }) {
            return Err("Node removal references a file that is not deleted".into());
        }
        if !update.remove_files.is_empty()
            && update.files.is_empty()
            && map.nodes.iter().any(|node| {
                node["id"] == update.node_id
                    && node["data"]["files"].as_array().is_some_and(|files| {
                        files
                            .iter()
                            .all(|file| update.remove_files.iter().any(|removed| file == removed))
                    })
            })
        {
            return Err("Remove the node instead of leaving it without source files".into());
        }
    }
    for (summary, status, files, reason, evidence, is_update) in changes
        .updates
        .iter()
        .map(|change| {
            (
                &change.summary,
                &change.status,
                &change.files,
                &change.reason,
                &change.evidence,
                true,
            )
        })
        .chain(changes.additions.iter().map(|change| {
            (
                &change.summary,
                &change.status,
                &change.files,
                &change.reason,
                &change.evidence,
                false,
            )
        }))
    {
        if summary.trim().is_empty()
            || summary.len() > 2000
            || status.trim().is_empty()
            || status.len() > 80
            || reason.trim().is_empty()
            || reason.len() > 2000
        {
            return Err("Invalid proposal text".into());
        }
        if files.len() > 50
            || files.iter().any(|file| !contents.contains_key(file))
            || (!is_update && files.is_empty())
        {
            return Err("Proposal references files outside this work result".into());
        }
        let deleted_evidence = is_update
            && changes.updates.iter().any(|update| {
                &update.evidence == evidence
                    && update
                        .remove_files
                        .iter()
                        .any(|file| evidence == &format!("DELETED:{file}"))
            });
        if evidence.trim().is_empty()
            || evidence.len() > 500
            || (!deleted_evidence
                && !files.iter().any(|file| {
                    contents
                        .get(file)
                        .is_some_and(|text| text.contains(evidence.as_str()))
                }))
        {
            return Err("Proposal lacks a matching source excerpt".into());
        }
    }
    if changes
        .additions
        .iter()
        .any(|node| node.title.trim().is_empty() || node.title.len() > 120)
    {
        return Err("Invalid new node title".into());
    }
    Ok(())
}

fn apply_changes(
    map: &mut ProjectMap,
    changes: &MapChanges,
    proposal_id: i64,
) -> Result<(), String> {
    for update in &changes.updates {
        let node = map
            .nodes
            .iter_mut()
            .find(|node| node["id"] == update.node_id && node["type"] == "component")
            .ok_or("Map node was removed")?;
        let data = node
            .get_mut("data")
            .and_then(Value::as_object_mut)
            .ok_or("Invalid node data")?;
        data.insert("summary".into(), json!(update.summary));
        data.insert("status".into(), json!(update.status));
        let mut files = data
            .get("files")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        files.retain(|file| !update.remove_files.iter().any(|removed| file == removed));
        for path in &update.files {
            if !files.iter().any(|file| file == path) {
                files.push(json!(path));
            }
        }
        data.insert("files".into(), Value::Array(files));
    }
    for (index, addition) in changes.additions.iter().enumerate() {
        map.nodes.push(json!({
            "id": format!("agent-{proposal_id}-{index}"), "type": "component",
            "position": {"x": 80 + (index % 4) * 240, "y": 120 + (index / 4) * 130},
            "data": {"title": addition.title, "summary": addition.summary, "status": addition.status,
                "files": addition.files, "kind": "Component", "technology": "Not specified",
                "icon": "map", "responsibilities": []}
        }));
    }
    let removed: HashSet<&str> = changes
        .removals
        .iter()
        .map(|removal| removal.node_id.as_str())
        .collect();
    map.nodes
        .retain(|node| !node["id"].as_str().is_some_and(|id| removed.contains(id)));
    map.edges.retain(|edge| {
        !edge["source"]
            .as_str()
            .is_some_and(|id| removed.contains(id))
            && !edge["target"]
                .as_str()
                .is_some_and(|id| removed.contains(id))
    });
    validate_map(&map.nodes, &map.edges)
}

#[tauri::command]
pub async fn propose_map_update(
    app: AppHandle,
    project_id: i64,
    changed_files: Vec<String>,
) -> Result<Option<MapProposal>, String> {
    if changed_files.is_empty() || changed_files.len() > 100 {
        return Err("Invalid changed file list".into());
    }
    let conn = database(&app)?;
    ensure_project(&conn, project_id)?;
    if conn
        .query_row(
            "SELECT id FROM map_proposals WHERE project_id = ?1 AND state = 'pending' LIMIT 1",
            [project_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .is_some()
    {
        return Err("Review the pending map update before requesting another".into());
    }
    let root: String = conn
        .query_row(
            "SELECT root FROM projects WHERE id = ?1",
            [project_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let map: ProjectMap = conn
        .query_row(
            "SELECT nodes, edges, revision FROM project_maps WHERE project_id = ?1",
            [project_id],
            |row| {
                let nodes: String = row.get(0)?;
                let edges: String = row.get(1)?;
                let revision = row.get(2)?;
                Ok((nodes, edges, revision))
            },
        )
        .map_err(|error| error.to_string())
        .and_then(|(nodes, edges, revision)| {
            Ok(ProjectMap {
                nodes: serde_json::from_str(&nodes).map_err(|error| error.to_string())?,
                edges: serde_json::from_str(&edges).map_err(|error| error.to_string())?,
                revision,
            })
        })?;
    let changed_files_len = changed_files.len();
    let allowed: Vec<String> = changed_files
        .into_iter()
        .filter(|file| {
            let path = Path::new(file);
            !path.is_absolute()
                && path
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_)))
        })
        .collect();
    if allowed.len() != changed_files_len {
        return Err("Invalid changed file path".into());
    }
    let (contents, deleted) = source_contents(Path::new(&root), &allowed)?;
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    std::fs::write(directory.join("map-proposal-schema.json"), OUTPUT_SCHEMA)
        .map_err(|error| error.to_string())?;
    let prompt = format!(
        "Do not call tools. Heim has already read the changed source below. Maintain the project map so it reflects implemented source, not just the prior plan. If source exports a new function or implements a feature absent from the map, you MUST return a new component in additions (a generic goal node is not a match). Update an existing matching component by nodeId instead. For each source-backed addition/update copy a short EXACT excerpt into evidence and link its changed file. On deletion, remove a node only if its sole linked file was deleted: use removals with nodeId, deletedFile and reason. For a node with other linked files, use updates with removeFiles containing the deleted path, files empty when no other changed sources are relevant, and evidence exactly DELETED:<path>. Never propose an inability-to-inspect placeholder. Return empty arrays only if no behavior or relevant node changed. Do not modify files.\nChanged source: {}\nDeleted paths: {}\nCurrent map nodes: {}",
        json!(contents),
        json!(deleted),
        json!(map.nodes)
    );
    let mut child = Command::new(codex_executable()?)
        .args([
            "exec",
            "--json",
            "--ephemeral",
            "--ignore-user-config",
            "--sandbox",
            "read-only",
            "--skip-git-repo-check",
            "--output-schema",
        ])
        .arg(directory.join("map-proposal-schema.json"))
        .arg("-C")
        .arg(&root)
        .arg("-")
        .current_dir(&root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| error.to_string())?;
    child
        .stdin
        .take()
        .ok_or("Codex stdin unavailable")?
        .write_all(prompt.as_bytes())
        .await
        .map_err(|error| error.to_string())?;
    let output = child
        .wait_with_output()
        .await
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Map review failed: {}",
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(500)
                .collect::<String>()
        ));
    }
    let events: Vec<Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect();
    if events
        .iter()
        .any(|event| event["type"] == "turn.failed" || event["type"] == "error")
    {
        return Err(
            "Agent could not complete the map review; retry after checking Codex availability"
                .into(),
        );
    }
    let response = events
        .iter()
        .filter(|event| {
            event["type"] == "item.completed" && event["item"]["type"] == "agent_message"
        })
        .filter_map(|event| event["item"]["text"].as_str())
        .last()
        .ok_or("Map review returned no result")?;
    let changes: MapChanges = serde_json::from_str(&response)
        .map_err(|error| format!("Invalid map proposal: {error}"))?;
    verify_changes(&map, &changes, &contents, &deleted)?;
    if changes.updates.is_empty() && changes.additions.is_empty() && changes.removals.is_empty() {
        return Ok(None);
    }
    let mut conn = conn;
    let tx = conn.transaction().map_err(|error| error.to_string())?;
    tx.execute("INSERT INTO map_proposals (project_id, base_revision, changed_files, changes) VALUES (?1, ?2, ?3, ?4)",
        params![project_id, map.revision, json!(allowed).to_string(), json!(changes).to_string()]).map_err(|error| error.to_string())?;
    let id = tx.last_insert_rowid();
    for (file, text) in &contents {
        tx.execute(
            "INSERT INTO proposal_sources (proposal_id, path, digest) VALUES (?1, ?2, ?3)",
            params![id, file, digest(text)],
        )
        .map_err(|error| error.to_string())?;
    }
    for file in &deleted {
        tx.execute(
            "INSERT INTO proposal_sources (proposal_id, path, digest) VALUES (?1, ?2, ?3)",
            params![id, file, "DELETED"],
        )
        .map_err(|error| error.to_string())?;
    }
    tx.commit().map_err(|error| error.to_string())?;
    Ok(Some(MapProposal {
        id,
        project_id,
        base_revision: map.revision,
        changed_files: allowed,
        changes,
    }))
}

#[tauri::command]
pub fn pending_map_proposals(app: AppHandle, project_id: i64) -> Result<Vec<MapProposal>, String> {
    let conn = database(&app)?;
    ensure_project(&conn, project_id)?;
    let mut query = conn.prepare("SELECT id, base_revision, changed_files, changes FROM map_proposals WHERE project_id = ?1 AND state = 'pending' ORDER BY id")
        .map_err(|error| error.to_string())?;
    query
        .query_map([project_id], |row| {
            let files: String = row.get(2)?;
            let changes: String = row.get(3)?;
            Ok((row.get(0)?, row.get(1)?, files, changes))
        })
        .map_err(|error| error.to_string())?
        .map(|row| {
            let (id, base_revision, files, changes): (i64, i64, String, String) =
                row.map_err(|error| error.to_string())?;
            Ok(MapProposal {
                id,
                project_id,
                base_revision,
                changed_files: serde_json::from_str(&files).map_err(|error| error.to_string())?,
                changes: serde_json::from_str(&changes).map_err(|error| error.to_string())?,
            })
        })
        .collect()
}

#[tauri::command]
pub fn resolve_map_proposal(
    app: AppHandle,
    project_id: i64,
    proposal_id: i64,
    accept: bool,
) -> Result<Option<ProjectMap>, String> {
    let mut conn = database(&app)?;
    ensure_project(&conn, project_id)?;
    let tx = conn.transaction().map_err(|error| error.to_string())?;
    let proposal: (i64, String, String) = tx.query_row(
        "SELECT base_revision, changed_files, changes FROM map_proposals WHERE id = ?1 AND project_id = ?2 AND state = 'pending'",
        params![proposal_id, project_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .optional().map_err(|error| error.to_string())?.ok_or("Proposal is no longer pending")?;
    if !accept {
        tx.execute(
            "UPDATE map_proposals SET state = 'rejected' WHERE id = ?1",
            [proposal_id],
        )
        .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
        return Ok(None);
    }
    let (nodes, edges, revision): (String, String, i64) = tx
        .query_row(
            "SELECT nodes, edges, revision FROM project_maps WHERE project_id = ?1",
            [project_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| error.to_string())?;
    if revision != proposal.0 {
        return Err("Map changed since this proposal; review again".into());
    }
    let mut map = ProjectMap {
        nodes: serde_json::from_str(&nodes).map_err(|error| error.to_string())?,
        edges: serde_json::from_str(&edges).map_err(|error| error.to_string())?,
        revision,
    };
    let files: Vec<String> =
        serde_json::from_str(&proposal.1).map_err(|error| error.to_string())?;
    let changes: MapChanges =
        serde_json::from_str(&proposal.2).map_err(|error| error.to_string())?;
    let root: String = tx
        .query_row(
            "SELECT root FROM projects WHERE id = ?1",
            [project_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let (contents, deleted) = source_contents(Path::new(&root), &files)?;
    let mut query = tx
        .prepare("SELECT path, digest FROM proposal_sources WHERE proposal_id = ?1")
        .map_err(|error| error.to_string())?;
    let fingerprints = query
        .query_map([proposal_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<HashMap<_, _>, _>>()
        .map_err(|error| error.to_string())?;
    drop(query);
    if fingerprints.len() != contents.len() + deleted.len()
        || contents
            .iter()
            .any(|(file, text)| fingerprints.get(file) != Some(&digest(text)))
        || deleted
            .iter()
            .any(|file| fingerprints.get(file).map(String::as_str) != Some("DELETED"))
    {
        return Err("Source files changed since this proposal; reject it and review again".into());
    }
    verify_changes(&map, &changes, &contents, &deleted)?;
    apply_changes(&mut map, &changes, proposal_id)?;
    let updated = tx.execute("UPDATE project_maps SET nodes = ?1, edges = ?2, revision = revision + 1 WHERE project_id = ?3 AND revision = ?4",
        params![json!(map.nodes).to_string(), json!(map.edges).to_string(), project_id, revision]).map_err(|error| error.to_string())?;
    if updated != 1 {
        return Err("Map was changed concurrently".into());
    }
    tx.execute(
        "UPDATE map_proposals SET state = 'accepted' WHERE id = ?1",
        [proposal_id],
    )
    .map_err(|error| error.to_string())?;
    tx.commit().map_err(|error| error.to_string())?;
    map.revision += 1;
    Ok(Some(map))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_nodes_and_unrelated_files() {
        let map = ProjectMap {
            nodes: vec![json!({"id":"a","type":"component","position":{"x":0,"y":0},"data":{}})],
            edges: vec![],
            revision: 0,
        };
        let contents = HashMap::from([("src/a.rs".into(), "export function a() {}".into())]);
        let mut changes = MapChanges {
            updates: vec![NodeUpdate {
                node_id: "missing".into(),
                summary: "done".into(),
                status: "Complete".into(),
                files: vec!["src/a.rs".into()],
                remove_files: vec![],
                reason: "evidence".into(),
                evidence: "export function a()".into(),
            }],
            additions: vec![],
            removals: vec![],
        };
        assert!(verify_changes(&map, &changes, &contents, &HashSet::new()).is_err());
        changes.updates[0].node_id = "a".into();
        changes.updates[0].files = vec!["other.rs".into()];
        assert!(verify_changes(&map, &changes, &contents, &HashSet::new()).is_err());
        changes.updates[0].files = vec!["src/a.rs".into()];
        changes.updates[0].evidence = "invented code".into();
        assert!(verify_changes(&map, &changes, &contents, &HashSet::new()).is_err());
    }

    #[test]
    fn applies_existing_and_new_nodes_without_discarding_existing_files() {
        let mut map = ProjectMap {
            nodes: vec![
                json!({"id":"a","type":"component","position":{"x":0,"y":0},"data":{"summary":"old","files":["existing.rs"]}}),
            ],
            edges: vec![],
            revision: 0,
        };
        let changes = MapChanges {
            updates: vec![NodeUpdate {
                node_id: "a".into(),
                summary: "implemented".into(),
                status: "Complete".into(),
                files: vec!["src/a.rs".into()],
                remove_files: vec![],
                reason: "export exists".into(),
                evidence: "export const a".into(),
            }],
            additions: vec![NodeAddition {
                title: "New module".into(),
                summary: "new behavior".into(),
                status: "Complete".into(),
                files: vec!["src/new.rs".into()],
                reason: "new export".into(),
                evidence: "export const b".into(),
            }],
            removals: vec![],
        };
        verify_changes(
            &map,
            &changes,
            &HashMap::from([
                ("src/a.rs".into(), "export const a = 1".into()),
                ("src/new.rs".into(), "export const b = 2".into()),
            ]),
            &HashSet::new(),
        )
        .unwrap();
        apply_changes(&mut map, &changes, 7).unwrap();
        assert_eq!(
            map.nodes[0]["data"]["files"],
            json!(["existing.rs", "src/a.rs"])
        );
        assert_eq!(map.nodes[0]["data"]["summary"], "implemented");
        assert_eq!(map.nodes[1]["id"], "agent-7-0");
        assert_eq!(map.nodes[1]["data"]["files"], json!(["src/new.rs"]));
    }

    #[test]
    fn source_digest_changes_when_content_changes() {
        assert_ne!(
            digest("export const greet = 1"),
            digest("export const greet = 2")
        );
        assert_eq!(digest("same text"), digest("same text"));
    }

    #[test]
    fn deleted_feature_removes_node_and_its_edges() {
        let mut map = ProjectMap {
            nodes: vec![
                json!({"id":"a","type":"component","position":{"x":0,"y":0},"data":{"files":["old.js"]}}),
                json!({"id":"b","type":"component","position":{"x":1,"y":1},"data":{"files":["keep.js"]}}),
            ],
            edges: vec![json!({"id":"ab","source":"a","target":"b"})],
            revision: 0,
        };
        let changes = MapChanges {
            updates: vec![],
            additions: vec![],
            removals: vec![NodeRemoval {
                node_id: "a".into(),
                deleted_file: "old.js".into(),
                reason: "Source removed".into(),
            }],
        };
        verify_changes(
            &map,
            &changes,
            &HashMap::new(),
            &HashSet::from(["old.js".into()]),
        )
        .unwrap();
        apply_changes(&mut map, &changes, 1).unwrap();
        assert_eq!(map.nodes.len(), 1);
        assert_eq!(map.nodes[0]["id"], "b");
        assert!(map.edges.is_empty());
    }
}
