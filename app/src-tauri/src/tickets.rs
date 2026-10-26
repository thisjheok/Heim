use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::AppHandle;

use crate::{database, ensure_project, has_current_approved_map, node_has_unanswered_questions};

fn sql_error(error: rusqlite::Error) -> String {
    error.to_string()
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketInput {
    pub feature_id: Option<i64>,
    pub title: String,
    pub goal: String,
    pub completion_criteria: Vec<String>,
    pub node_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Feature {
    id: i64,
    project_id: i64,
    title: String,
    description: String,
    position: i64,
    created_at: String,
    ticket_count: i64,
    done_count: i64,
    completed: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ticket {
    pub id: i64,
    project_id: i64,
    feature_id: Option<i64>,
    title: String,
    goal: String,
    completion_criteria: Vec<String>,
    node_ids: Vec<String>,
    status: String,
    position: i64,
    source: String,
    created_at: String,
    updated_at: String,
    latest_run_id: Option<i64>,
    latest_run_state: Option<String>,
    latest_run_detail: Option<String>,
    latest_run_started_at: Option<String>,
    latest_run_finished_at: Option<String>,
    report_outcome: Option<String>,
    review_decision: Option<String>,
    review_note: Option<String>,
    reviewed_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketBoard {
    features: Vec<Feature>,
    tickets: Vec<Ticket>,
    next_ticket_id: Option<i64>,
}

pub(crate) fn normalize_ticket_input(input: TicketInput) -> Result<TicketInput, String> {
    let title = input.title.trim().to_owned();
    if title.is_empty() || title.chars().count() > 200 {
        return Err("Ticket title must contain 1-200 characters".into());
    }
    let goal = input.goal.trim().to_owned();
    if goal.is_empty() || goal.len() > 4_000 {
        return Err("Ticket goal must contain 1-4000 characters".into());
    }
    let completion_criteria = input
        .completion_criteria
        .iter()
        .map(|criterion| criterion.trim().to_owned())
        .collect::<Vec<_>>();
    if completion_criteria.is_empty()
        || completion_criteria.len() > 20
        || completion_criteria
            .iter()
            .any(|criterion| criterion.is_empty() || criterion.len() > 1_000)
    {
        return Err("Provide 1-20 completion criteria, each containing 1-1000 characters".into());
    }
    let mut seen = HashSet::new();
    let mut node_ids = Vec::new();
    for node_id in input.node_ids {
        let node_id = node_id.trim().to_owned();
        if node_id.is_empty() || node_id.len() > 128 {
            return Err("Invalid ticket node reference".into());
        }
        if seen.insert(node_id.clone()) {
            node_ids.push(node_id);
        }
    }
    if node_ids.len() > 20 {
        return Err("A ticket can reference at most 20 map nodes".into());
    }
    Ok(TicketInput {
        feature_id: input.feature_id,
        title,
        goal,
        completion_criteria,
        node_ids,
    })
}

fn validate_feature_text(title: &str, description: &str) -> Result<(), String> {
    if title.trim().is_empty() || title.trim().chars().count() > 200 {
        return Err("Feature title must contain 1-200 characters".into());
    }
    if description.trim().len() > 4_000 {
        return Err("Feature description must be at most 4000 characters".into());
    }
    Ok(())
}

fn load_map(conn: &Connection, project_id: i64) -> Result<(Vec<Value>, Vec<Value>, i64), String> {
    let (nodes, edges, revision): (String, String, i64) = conn
        .query_row(
            "SELECT nodes, edges, revision FROM project_maps WHERE project_id = ?1",
            [project_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(sql_error)?
        .unwrap_or_else(|| ("[]".into(), "[]".into(), 0));
    Ok((
        serde_json::from_str(&nodes).map_err(|error| error.to_string())?,
        serde_json::from_str(&edges).map_err(|error| error.to_string())?,
        revision,
    ))
}

fn component_ids(nodes: &[Value]) -> HashSet<String> {
    nodes
        .iter()
        .filter(|node| node["type"] == "component")
        .filter_map(|node| node["id"].as_str().map(str::to_owned))
        .collect()
}

pub(crate) fn map_component_ids(
    conn: &Connection,
    project_id: i64,
) -> Result<HashSet<String>, String> {
    Ok(component_ids(&load_map(conn, project_id)?.0))
}

fn ensure_known_nodes(known: &HashSet<String>, node_ids: &[String]) -> Result<(), String> {
    match node_ids.iter().find(|id| !known.contains(*id)) {
        Some(id) => Err(format!(
            "Map node '{id}' does not exist in the confirmed map"
        )),
        None => Ok(()),
    }
}

fn ensure_feature(
    conn: &Connection,
    project_id: i64,
    feature_id: Option<i64>,
) -> Result<(), String> {
    let Some(feature_id) = feature_id else {
        return Ok(());
    };
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM features WHERE id = ?1 AND project_id = ?2)",
            params![feature_id, project_id],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    if !exists {
        return Err("Feature not found".into());
    }
    Ok(())
}

fn ticket_has_active_run(conn: &Connection, ticket_id: i64) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM ticket_runs WHERE ticket_id = ?1 AND state IN ('starting', 'running'))",
        [ticket_id],
        |row| row.get(0),
    )
    .map_err(sql_error)
}

fn ensure_ticket(conn: &Connection, project_id: i64, ticket_id: i64) -> Result<(), String> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM tickets WHERE id = ?1 AND project_id = ?2)",
            params![ticket_id, project_id],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    if !exists {
        return Err("Ticket not found".into());
    }
    Ok(())
}

fn ensure_ticket_idle(conn: &Connection, project_id: i64, ticket_id: i64) -> Result<(), String> {
    ensure_ticket(conn, project_id, ticket_id)?;
    if ticket_has_active_run(conn, ticket_id)? {
        return Err("Wait for the ticket's Agent run to finish first".into());
    }
    Ok(())
}

fn next_ticket_position(
    conn: &Connection,
    project_id: i64,
    feature_id: Option<i64>,
) -> Result<i64, String> {
    conn.query_row(
        "SELECT COALESCE(MAX(position), 0) + 1 FROM tickets WHERE project_id = ?1 AND feature_id IS ?2",
        params![project_id, feature_id],
        |row| row.get(0),
    )
    .map_err(sql_error)
}

fn replace_ticket_nodes(
    conn: &Connection,
    ticket_id: i64,
    node_ids: &[String],
) -> Result<(), String> {
    conn.execute("DELETE FROM ticket_nodes WHERE ticket_id = ?1", [ticket_id])
        .map_err(sql_error)?;
    for node_id in node_ids {
        conn.execute(
            "INSERT INTO ticket_nodes (ticket_id, node_id) VALUES (?1, ?2)",
            params![ticket_id, node_id],
        )
        .map_err(sql_error)?;
    }
    Ok(())
}

pub(crate) fn insert_ticket(
    conn: &Connection,
    project_id: i64,
    input: &TicketInput,
    source: &str,
) -> Result<i64, String> {
    ensure_feature(conn, project_id, input.feature_id)?;
    ensure_known_nodes(&map_component_ids(conn, project_id)?, &input.node_ids)?;
    let position = next_ticket_position(conn, project_id, input.feature_id)?;
    conn.execute(
        "INSERT INTO tickets (project_id, feature_id, title, goal, completion_criteria, position, source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            project_id,
            input.feature_id,
            input.title,
            input.goal,
            serde_json::to_string(&input.completion_criteria).map_err(|error| error.to_string())?,
            position,
            source
        ],
    )
    .map_err(sql_error)?;
    let id = conn.last_insert_rowid();
    replace_ticket_nodes(conn, id, &input.node_ids)?;
    Ok(id)
}

fn load_features(conn: &Connection, project_id: i64) -> Result<Vec<Feature>, String> {
    let mut statement = conn
        .prepare(
            "SELECT f.id, f.title, f.description, f.position, f.created_at,
                    (SELECT COUNT(*) FROM tickets t WHERE t.feature_id = f.id),
                    (SELECT COUNT(*) FROM tickets t WHERE t.feature_id = f.id AND t.status = 'done')
             FROM features f WHERE f.project_id = ?1 ORDER BY f.position, f.id",
        )
        .map_err(sql_error)?;
    statement
        .query_map([project_id], |row| {
            let ticket_count: i64 = row.get(5)?;
            let done_count: i64 = row.get(6)?;
            Ok(Feature {
                id: row.get(0)?,
                project_id,
                title: row.get(1)?,
                description: row.get(2)?,
                position: row.get(3)?,
                created_at: row.get(4)?,
                ticket_count,
                done_count,
                completed: ticket_count > 0 && ticket_count == done_count,
            })
        })
        .map_err(sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error)
}

fn load_tickets(conn: &Connection, project_id: i64) -> Result<Vec<Ticket>, String> {
    let mut nodes = HashMap::<i64, Vec<String>>::new();
    {
        let mut statement = conn
            .prepare(
                "SELECT n.ticket_id, n.node_id FROM ticket_nodes n
                 JOIN tickets t ON t.id = n.ticket_id
                 WHERE t.project_id = ?1 ORDER BY n.rowid",
            )
            .map_err(sql_error)?;
        let rows = statement
            .query_map([project_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(sql_error)?;
        for row in rows {
            let (ticket_id, node_id) = row.map_err(sql_error)?;
            nodes.entry(ticket_id).or_default().push(node_id);
        }
    }
    let mut statement = conn
        .prepare(
            "SELECT t.id, t.feature_id, t.title, t.goal, t.completion_criteria, t.status,
                    t.position, t.source, t.created_at, t.updated_at,
                    r.id, r.state, r.detail, r.started_at, r.finished_at,
                    p.outcome, v.decision, v.note, v.reviewed_at
             FROM tickets t
             LEFT JOIN features f ON f.id = t.feature_id
             LEFT JOIN ticket_runs r ON r.id = (
                 SELECT id FROM ticket_runs WHERE ticket_id = t.id ORDER BY id DESC LIMIT 1
             )
             LEFT JOIN ticket_run_reports p ON p.run_id = r.id
             LEFT JOIN ticket_reviews v ON v.id = (
                 SELECT id FROM ticket_reviews WHERE run_id = r.id ORDER BY id DESC LIMIT 1
             )
             WHERE t.project_id = ?1
             ORDER BY f.position IS NULL, f.position, t.feature_id, t.position, t.id",
        )
        .map_err(sql_error)?;
    let rows = statement
        .query_map([project_id], |row| {
            let criteria: String = row.get(4)?;
            Ok(Ticket {
                id: row.get(0)?,
                project_id,
                feature_id: row.get(1)?,
                title: row.get(2)?,
                goal: row.get(3)?,
                completion_criteria: serde_json::from_str(&criteria).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        4,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
                node_ids: Vec::new(),
                status: row.get(5)?,
                position: row.get(6)?,
                source: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
                latest_run_id: row.get(10)?,
                latest_run_state: row.get(11)?,
                latest_run_detail: row.get(12)?,
                latest_run_started_at: row.get(13)?,
                latest_run_finished_at: row.get(14)?,
                report_outcome: row.get(15)?,
                review_decision: row.get(16)?,
                review_note: row.get(17)?,
                reviewed_at: row.get(18)?,
            })
        })
        .map_err(sql_error)?;
    rows.map(|row| {
        let mut ticket = row.map_err(sql_error)?;
        ticket.node_ids = nodes.remove(&ticket.id).unwrap_or_default();
        Ok(ticket)
    })
    .collect()
}

pub(crate) fn load_ticket(
    conn: &Connection,
    project_id: i64,
    ticket_id: i64,
) -> Result<Ticket, String> {
    load_tickets(conn, project_id)?
        .into_iter()
        .find(|ticket| ticket.id == ticket_id)
        .ok_or_else(|| "Ticket not found".to_owned())
}

/// The next ticket is the first open one in feature order, then ticket order. No LLM involved.
pub(crate) fn next_ticket_id(conn: &Connection, project_id: i64) -> Result<Option<i64>, String> {
    conn.query_row(
        "SELECT t.id FROM tickets t LEFT JOIN features f ON f.id = t.feature_id
         WHERE t.project_id = ?1 AND t.status = 'todo'
         ORDER BY f.position IS NULL, f.position, t.feature_id, t.position, t.id LIMIT 1",
        [project_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(sql_error)
}

pub(crate) fn ticket_board_in_db(
    conn: &Connection,
    project_id: i64,
) -> Result<TicketBoard, String> {
    ensure_project(conn, project_id)?;
    Ok(TicketBoard {
        features: load_features(conn, project_id)?,
        tickets: load_tickets(conn, project_id)?,
        next_ticket_id: next_ticket_id(conn, project_id)?,
    })
}

#[tauri::command]
pub fn project_ticket_board(app: AppHandle, project_id: i64) -> Result<TicketBoard, String> {
    ticket_board_in_db(&database(&app)?, project_id)
}

fn create_feature_in_db(
    conn: &Connection,
    project_id: i64,
    title: &str,
    description: &str,
) -> Result<i64, String> {
    ensure_project(conn, project_id)?;
    validate_feature_text(title, description)?;
    conn.execute(
        "INSERT INTO features (project_id, title, description, position)
         VALUES (?1, ?2, ?3, (SELECT COALESCE(MAX(position), 0) + 1 FROM features WHERE project_id = ?1))",
        params![project_id, title.trim(), description.trim()],
    )
    .map_err(sql_error)?;
    Ok(conn.last_insert_rowid())
}

#[tauri::command]
pub fn create_feature(
    app: AppHandle,
    project_id: i64,
    title: String,
    description: String,
) -> Result<TicketBoard, String> {
    let conn = database(&app)?;
    create_feature_in_db(&conn, project_id, &title, &description)?;
    ticket_board_in_db(&conn, project_id)
}

#[tauri::command]
pub fn update_feature(
    app: AppHandle,
    project_id: i64,
    feature_id: i64,
    title: String,
    description: String,
) -> Result<TicketBoard, String> {
    let conn = database(&app)?;
    ensure_project(&conn, project_id)?;
    validate_feature_text(&title, &description)?;
    let updated = conn
        .execute(
            "UPDATE features SET title = ?1, description = ?2 WHERE id = ?3 AND project_id = ?4",
            params![title.trim(), description.trim(), feature_id, project_id],
        )
        .map_err(sql_error)?;
    if updated != 1 {
        return Err("Feature not found".into());
    }
    ticket_board_in_db(&conn, project_id)
}

fn delete_feature_in_db(
    conn: &mut Connection,
    project_id: i64,
    feature_id: i64,
) -> Result<(), String> {
    ensure_project(conn, project_id)?;
    let tx = conn.transaction().map_err(sql_error)?;
    ensure_feature(&tx, project_id, Some(feature_id))?;
    // Keep the feature's tickets (and their history) as unassigned tickets after existing ones.
    let offset = next_ticket_position(&tx, project_id, None)?;
    tx.execute(
        "UPDATE tickets SET feature_id = NULL, position = position + ?1 WHERE feature_id = ?2",
        params![offset, feature_id],
    )
    .map_err(sql_error)?;
    tx.execute("DELETE FROM features WHERE id = ?1", [feature_id])
        .map_err(sql_error)?;
    tx.commit().map_err(sql_error)
}

#[tauri::command]
pub fn delete_feature(
    app: AppHandle,
    project_id: i64,
    feature_id: i64,
) -> Result<TicketBoard, String> {
    let mut conn = database(&app)?;
    delete_feature_in_db(&mut conn, project_id, feature_id)?;
    ticket_board_in_db(&conn, project_id)
}

fn swap_position(
    conn: &mut Connection,
    table: &str,
    group_filter: &str,
    group_params: &[&dyn rusqlite::ToSql],
    id: i64,
    up: bool,
) -> Result<(), String> {
    let tx = conn.transaction().map_err(sql_error)?;
    let ids = {
        let mut statement = tx
            .prepare(&format!(
                "SELECT id FROM {table} WHERE {group_filter} ORDER BY position, id"
            ))
            .map_err(sql_error)?;
        statement
            .query_map(group_params, |row| row.get::<_, i64>(0))
            .map_err(sql_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql_error)?
    };
    let index = ids
        .iter()
        .position(|item| *item == id)
        .ok_or("Item not found")?;
    let target = if up {
        index.checked_sub(1)
    } else {
        Some(index + 1).filter(|target| *target < ids.len())
    };
    let Some(target) = target else {
        return Ok(());
    };
    let mut ordered = ids;
    ordered.swap(index, target);
    for (position, item) in ordered.iter().enumerate() {
        tx.execute(
            &format!("UPDATE {table} SET position = ?1 WHERE id = ?2"),
            params![position as i64 + 1, item],
        )
        .map_err(sql_error)?;
    }
    tx.commit().map_err(sql_error)
}

fn move_feature_in_db(
    conn: &mut Connection,
    project_id: i64,
    feature_id: i64,
    up: bool,
) -> Result<(), String> {
    ensure_project(conn, project_id)?;
    ensure_feature(conn, project_id, Some(feature_id))?;
    swap_position(
        conn,
        "features",
        "project_id = ?1",
        &[&project_id],
        feature_id,
        up,
    )
}

fn move_ticket_in_db(
    conn: &mut Connection,
    project_id: i64,
    ticket_id: i64,
    up: bool,
) -> Result<(), String> {
    ensure_project(conn, project_id)?;
    ensure_ticket(conn, project_id, ticket_id)?;
    let feature_id: Option<i64> = conn
        .query_row(
            "SELECT feature_id FROM tickets WHERE id = ?1",
            [ticket_id],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    swap_position(
        conn,
        "tickets",
        "project_id = ?1 AND feature_id IS ?2",
        &[&project_id, &feature_id],
        ticket_id,
        up,
    )
}

fn parse_direction(direction: &str) -> Result<bool, String> {
    match direction {
        "up" => Ok(true),
        "down" => Ok(false),
        _ => Err("Direction must be up or down".into()),
    }
}

#[tauri::command]
pub fn move_feature(
    app: AppHandle,
    project_id: i64,
    feature_id: i64,
    direction: String,
) -> Result<TicketBoard, String> {
    let mut conn = database(&app)?;
    move_feature_in_db(
        &mut conn,
        project_id,
        feature_id,
        parse_direction(&direction)?,
    )?;
    ticket_board_in_db(&conn, project_id)
}

#[tauri::command]
pub fn move_ticket(
    app: AppHandle,
    project_id: i64,
    ticket_id: i64,
    direction: String,
) -> Result<TicketBoard, String> {
    let mut conn = database(&app)?;
    move_ticket_in_db(
        &mut conn,
        project_id,
        ticket_id,
        parse_direction(&direction)?,
    )?;
    ticket_board_in_db(&conn, project_id)
}

fn create_ticket_in_db(
    conn: &mut Connection,
    project_id: i64,
    input: TicketInput,
    source: &str,
) -> Result<i64, String> {
    ensure_project(conn, project_id)?;
    let input = normalize_ticket_input(input)?;
    let tx = conn.transaction().map_err(sql_error)?;
    let id = insert_ticket(&tx, project_id, &input, source)?;
    tx.commit().map_err(sql_error)?;
    Ok(id)
}

#[tauri::command]
pub fn create_ticket(
    app: AppHandle,
    project_id: i64,
    input: TicketInput,
) -> Result<TicketBoard, String> {
    let mut conn = database(&app)?;
    create_ticket_in_db(&mut conn, project_id, input, "user")?;
    ticket_board_in_db(&conn, project_id)
}

fn update_ticket_in_db(
    conn: &mut Connection,
    project_id: i64,
    ticket_id: i64,
    input: TicketInput,
) -> Result<(), String> {
    ensure_project(conn, project_id)?;
    let input = normalize_ticket_input(input)?;
    let tx = conn.transaction().map_err(sql_error)?;
    ensure_ticket_idle(&tx, project_id, ticket_id)?;
    ensure_feature(&tx, project_id, input.feature_id)?;
    let known = map_component_ids(&tx, project_id)?;
    // Links to nodes that were later removed from the map may stay; new links must exist.
    let existing = {
        let mut statement = tx
            .prepare("SELECT node_id FROM ticket_nodes WHERE ticket_id = ?1")
            .map_err(sql_error)?;
        statement
            .query_map([ticket_id], |row| row.get::<_, String>(0))
            .map_err(sql_error)?
            .collect::<Result<HashSet<_>, _>>()
            .map_err(sql_error)?
    };
    let added = input
        .node_ids
        .iter()
        .filter(|id| !existing.contains(*id))
        .cloned()
        .collect::<Vec<_>>();
    ensure_known_nodes(&known, &added)?;
    let current_feature: Option<i64> = tx
        .query_row(
            "SELECT feature_id FROM tickets WHERE id = ?1",
            [ticket_id],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    if current_feature != input.feature_id {
        let position = next_ticket_position(&tx, project_id, input.feature_id)?;
        tx.execute(
            "UPDATE tickets SET feature_id = ?1, position = ?2 WHERE id = ?3",
            params![input.feature_id, position, ticket_id],
        )
        .map_err(sql_error)?;
    }
    tx.execute(
        "UPDATE tickets SET title = ?1, goal = ?2, completion_criteria = ?3,
             updated_at = CURRENT_TIMESTAMP
         WHERE id = ?4",
        params![
            input.title,
            input.goal,
            serde_json::to_string(&input.completion_criteria).map_err(|error| error.to_string())?,
            ticket_id
        ],
    )
    .map_err(sql_error)?;
    replace_ticket_nodes(&tx, ticket_id, &input.node_ids)?;
    tx.commit().map_err(sql_error)
}

#[tauri::command]
pub fn update_ticket(
    app: AppHandle,
    project_id: i64,
    ticket_id: i64,
    input: TicketInput,
) -> Result<TicketBoard, String> {
    let mut conn = database(&app)?;
    update_ticket_in_db(&mut conn, project_id, ticket_id, input)?;
    ticket_board_in_db(&conn, project_id)
}

fn delete_ticket_in_db(conn: &Connection, project_id: i64, ticket_id: i64) -> Result<(), String> {
    ensure_project(conn, project_id)?;
    ensure_ticket_idle(conn, project_id, ticket_id)?;
    conn.execute("DELETE FROM tickets WHERE id = ?1", [ticket_id])
        .map_err(sql_error)?;
    Ok(())
}

#[tauri::command]
pub fn delete_ticket(
    app: AppHandle,
    project_id: i64,
    ticket_id: i64,
) -> Result<TicketBoard, String> {
    let conn = database(&app)?;
    delete_ticket_in_db(&conn, project_id, ticket_id)?;
    ticket_board_in_db(&conn, project_id)
}

fn set_ticket_status_in_db(
    conn: &Connection,
    project_id: i64,
    ticket_id: i64,
    status: &str,
) -> Result<(), String> {
    // in_progress is owned by Agent runs; users can only reopen or close a ticket.
    if !matches!(status, "todo" | "done") {
        return Err("Ticket status can only be set to todo or done".into());
    }
    ensure_project(conn, project_id)?;
    ensure_ticket_idle(conn, project_id, ticket_id)?;
    conn.execute(
        "UPDATE tickets SET status = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
        params![status, ticket_id],
    )
    .map_err(sql_error)?;
    Ok(())
}

#[tauri::command]
pub fn set_ticket_status(
    app: AppHandle,
    project_id: i64,
    ticket_id: i64,
    status: String,
) -> Result<TicketBoard, String> {
    let conn = database(&app)?;
    set_ticket_status_in_db(&conn, project_id, ticket_id, &status)?;
    ticket_board_in_db(&conn, project_id)
}

/// Map context sent to the Agent for one ticket: the whole map at a glance plus the
/// linked nodes in detail. The map is shared vocabulary, never additional scope.
pub(crate) struct TicketDevelopmentContext {
    pub ticket_id: i64,
    pub run_id: i64,
    pub map_revision: i64,
    pub title: String,
    pub goal: String,
    pub completion_criteria: Vec<String>,
    pub map_overview: Vec<Value>,
    pub target_nodes: Vec<Value>,
    pub related_nodes: Vec<Value>,
    pub related_edges: Vec<Value>,
    pub missing_node_ids: Vec<String>,
}

pub(crate) fn map_overview(nodes: &[Value]) -> Vec<Value> {
    nodes
        .iter()
        .filter(|node| node["type"] == "component")
        .map(|node| {
            json!({
                "id": node["id"],
                "title": node.pointer("/data/title"),
                "summary": node.pointer("/data/summary"),
            })
        })
        .collect()
}

pub(crate) fn begin_ticket_run(
    conn: &mut Connection,
    project_id: i64,
    ticket_id: i64,
) -> Result<TicketDevelopmentContext, String> {
    ensure_project(conn, project_id)?;
    let tx = conn.transaction().map_err(sql_error)?;
    let ticket = load_ticket(&tx, project_id, ticket_id)?;
    if ticket.status == "done" {
        return Err(
            "This ticket is done. Reopen it or create a new ticket for further changes".into(),
        );
    }
    if ticket_has_active_run(&tx, ticket_id)? {
        return Err("This ticket already has an active Agent run".into());
    }
    if !has_current_approved_map(&tx, project_id)? {
        return Err("Approve a design map before starting development".into());
    }
    let (nodes, edges, map_revision) = load_map(&tx, project_id)?;
    let linked = ticket
        .node_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let target_nodes = nodes
        .iter()
        .filter(|node| {
            node["type"] == "component" && node["id"].as_str().is_some_and(|id| linked.contains(id))
        })
        .cloned()
        .collect::<Vec<_>>();
    if let Some(node) = target_nodes
        .iter()
        .find(|node| node_has_unanswered_questions(node))
    {
        return Err(format!(
            "Answer the open questions on '{}' before starting this ticket",
            node.pointer("/data/title")
                .and_then(Value::as_str)
                .unwrap_or("a linked node")
        ));
    }
    let found = target_nodes
        .iter()
        .filter_map(|node| node["id"].as_str())
        .collect::<HashSet<_>>();
    let missing_node_ids = ticket
        .node_ids
        .iter()
        .filter(|id| !found.contains(id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let related_edges = edges
        .iter()
        .filter(|edge| {
            [edge["source"].as_str(), edge["target"].as_str()]
                .into_iter()
                .flatten()
                .any(|id| found.contains(id))
        })
        .cloned()
        .collect::<Vec<_>>();
    let neighbor_ids = related_edges
        .iter()
        .flat_map(|edge| [edge["source"].as_str(), edge["target"].as_str()])
        .flatten()
        .filter(|id| !found.contains(id))
        .collect::<HashSet<_>>();
    let related_nodes = nodes
        .iter()
        .filter(|node| {
            node["id"]
                .as_str()
                .is_some_and(|id| neighbor_ids.contains(id))
        })
        .cloned()
        .collect::<Vec<_>>();
    tx.execute(
        "INSERT INTO ticket_runs (ticket_id, map_revision, completion_criteria, state)
         VALUES (?1, ?2, ?3, 'starting')",
        params![
            ticket_id,
            map_revision,
            serde_json::to_string(&ticket.completion_criteria).map_err(|error| error.to_string())?
        ],
    )
    .map_err(sql_error)?;
    let run_id = tx.last_insert_rowid();
    tx.execute(
        "UPDATE tickets SET status = 'in_progress', updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
        [ticket_id],
    )
    .map_err(sql_error)?;
    tx.commit().map_err(sql_error)?;
    Ok(TicketDevelopmentContext {
        ticket_id,
        run_id,
        map_revision,
        title: ticket.title,
        goal: ticket.goal,
        completion_criteria: ticket.completion_criteria,
        map_overview: map_overview(&nodes),
        target_nodes,
        related_nodes,
        related_edges,
        missing_node_ids,
    })
}

pub(crate) fn transition_ticket_run(
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
            "UPDATE ticket_runs SET state = 'running', detail = NULL
             WHERE id = ?1 AND state IN ('starting', 'running')",
            [run_id],
        )
    } else {
        conn.execute(
            "UPDATE ticket_runs
             SET state = ?2, detail = ?3, finished_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND state IN ('starting', 'running')",
            params![run_id, state, detail.as_deref()],
        )
    }
    .map_err(sql_error)?;
    // A finished run waits for review; any other ending returns the ticket to the queue.
    if updated == 1 && !matches!(state, "running" | "finished") {
        conn.execute(
            "UPDATE tickets SET status = 'todo', updated_at = CURRENT_TIMESTAMP
             WHERE id = (SELECT ticket_id FROM ticket_runs WHERE id = ?1) AND status = 'in_progress'",
            [run_id],
        )
        .map_err(sql_error)?;
    }
    Ok(updated == 1)
}

pub(crate) fn save_ticket_run_report(
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
    let tx = conn.transaction().map_err(sql_error)?;
    let stored_criteria_json: Option<String> = tx
        .query_row(
            "SELECT completion_criteria FROM ticket_runs
             WHERE id = ?1 AND state IN ('starting', 'running')",
            [run_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_error)?;
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
        return Err("Development report criteria do not match the ticket".into());
    }
    tx.execute(
        "INSERT INTO ticket_run_reports (run_id, outcome, summary, report)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(run_id) DO UPDATE SET outcome = excluded.outcome,
             summary = excluded.summary, report = excluded.report, saved_at = CURRENT_TIMESTAMP",
        params![run_id, outcome, summary, serialized],
    )
    .map_err(sql_error)?;
    tx.commit().map_err(sql_error)
}

pub(crate) fn record_ticket_run_evidence(
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
            "INSERT INTO ticket_run_evidence (run_id, kind, evidence)
             SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM ticket_runs WHERE id = ?1)",
            params![run_id, kind, serialized],
        )
        .map_err(sql_error)?;
    if inserted != 1 {
        return Err("Development-run evidence references an unknown run".into());
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketReviewDecision {
    id: i64,
    ticket_id: i64,
    run_id: i64,
    decision: String,
    note: Option<String>,
    reviewed_at: String,
    ticket_status: String,
}

fn save_ticket_review_in_db(
    conn: &mut Connection,
    project_id: i64,
    ticket_id: i64,
    run_id: i64,
    decision: &str,
    note: Option<&str>,
) -> Result<TicketReviewDecision, String> {
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
    let tx = conn.transaction().map_err(sql_error)?;
    ensure_project(&tx, project_id)?;
    let run_state = tx
        .query_row(
            "SELECT r.state FROM ticket_runs r
             JOIN tickets t ON t.id = r.ticket_id
             WHERE r.id = ?1 AND t.id = ?2 AND t.project_id = ?3
               AND r.id = (SELECT id FROM ticket_runs WHERE ticket_id = t.id ORDER BY id DESC LIMIT 1)",
            params![run_id, ticket_id, project_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(sql_error)?
        .ok_or("Review must reference the ticket's latest run")?;
    if matches!(run_state.as_str(), "starting" | "running") {
        return Err("Wait for the Agent run to finish before reviewing it".into());
    }
    tx.execute(
        "INSERT INTO ticket_reviews (ticket_id, run_id, decision, note) VALUES (?1, ?2, ?3, ?4)",
        params![ticket_id, run_id, decision, note],
    )
    .map_err(sql_error)?;
    let id = tx.last_insert_rowid();
    // The user's review, not the Agent's report, decides whether the ticket is done.
    let ticket_status = if decision == "approved_completion" {
        "done"
    } else {
        "todo"
    };
    tx.execute(
        "UPDATE tickets SET status = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
        params![ticket_status, ticket_id],
    )
    .map_err(sql_error)?;
    let reviewed_at = tx
        .query_row(
            "SELECT reviewed_at FROM ticket_reviews WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    tx.commit().map_err(sql_error)?;
    Ok(TicketReviewDecision {
        id,
        ticket_id,
        run_id,
        decision: decision.to_owned(),
        note: note.map(str::to_owned),
        reviewed_at,
        ticket_status: ticket_status.to_owned(),
    })
}

#[tauri::command]
pub fn save_ticket_review(
    app: AppHandle,
    project_id: i64,
    ticket_id: i64,
    run_id: i64,
    decision: String,
    note: Option<String>,
) -> Result<TicketReviewDecision, String> {
    let mut conn = database(&app)?;
    save_ticket_review_in_db(
        &mut conn,
        project_id,
        ticket_id,
        run_id,
        &decision,
        note.as_deref(),
    )
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketRunEvidence {
    id: i64,
    kind: String,
    evidence: Value,
    observed_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketRunReview {
    run_id: i64,
    ticket_id: i64,
    map_revision: i64,
    completion_criteria: Vec<String>,
    state: String,
    detail: Option<String>,
    started_at: String,
    finished_at: Option<String>,
    report: Option<Value>,
    evidence: Vec<TicketRunEvidence>,
}

fn ticket_run_review_in_db(
    conn: &Connection,
    project_id: i64,
    run_id: i64,
) -> Result<TicketRunReview, String> {
    ensure_project(conn, project_id)?;
    let (ticket_id, map_revision, criteria, state, detail, started_at, finished_at, report): (
        i64,
        i64,
        String,
        String,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
    ) = conn
        .query_row(
            "SELECT r.ticket_id, r.map_revision, r.completion_criteria, r.state, r.detail,
                    r.started_at, r.finished_at, p.report
             FROM ticket_runs r
             JOIN tickets t ON t.id = r.ticket_id
             LEFT JOIN ticket_run_reports p ON p.run_id = r.id
             WHERE r.id = ?1 AND t.project_id = ?2",
            params![run_id, project_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .map_err(sql_error)?;
    let mut statement = conn
        .prepare(
            "SELECT id, kind, evidence, observed_at FROM ticket_run_evidence
             WHERE run_id = ?1 ORDER BY id",
        )
        .map_err(sql_error)?;
    let evidence = statement
        .query_map([run_id], |row| {
            let serialized: String = row.get(2)?;
            Ok(TicketRunEvidence {
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
        .map_err(sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error)?;
    Ok(TicketRunReview {
        run_id,
        ticket_id,
        map_revision,
        completion_criteria: serde_json::from_str(&criteria).map_err(|error| error.to_string())?,
        state,
        detail,
        started_at,
        finished_at,
        report: report
            .map(|report| serde_json::from_str(&report).map_err(|error| error.to_string()))
            .transpose()?,
        evidence,
    })
}

#[tauri::command]
pub fn ticket_run_review(
    app: AppHandle,
    project_id: i64,
    run_id: i64,
) -> Result<TicketRunReview, String> {
    ticket_run_review_in_db(&database(&app)?, project_id, run_id)
}

/// Feature and map context for AI ticket planning.
pub(crate) struct FeaturePlanContext {
    pub map_revision: i64,
    pub feature: Value,
    pub existing_tickets: Vec<Value>,
    pub nodes: Vec<Value>,
    pub edges: Vec<Value>,
}

pub(crate) fn feature_plan_context(
    conn: &Connection,
    project_id: i64,
    feature_id: i64,
) -> Result<FeaturePlanContext, String> {
    ensure_project(conn, project_id)?;
    if !has_current_approved_map(conn, project_id)? {
        return Err("Approve a design map before planning tickets".into());
    }
    let feature = load_features(conn, project_id)?
        .into_iter()
        .find(|feature| feature.id == feature_id)
        .ok_or("Feature not found")?;
    let existing_tickets = load_tickets(conn, project_id)?
        .into_iter()
        .filter(|ticket| ticket.feature_id == Some(feature_id))
        .map(|ticket| {
            json!({
                "title": ticket.title,
                "goal": ticket.goal,
                "status": ticket.status,
                "nodeIds": ticket.node_ids,
            })
        })
        .collect();
    let (nodes, edges, map_revision) = load_map(conn, project_id)?;
    Ok(FeaturePlanContext {
        map_revision,
        feature: json!({"id": feature.id, "title": feature.title, "description": feature.description}),
        existing_tickets,
        nodes,
        edges,
    })
}

pub(crate) fn store_planned_tickets(
    conn: &mut Connection,
    project_id: i64,
    feature_id: i64,
    tickets: Vec<TicketInput>,
) -> Result<Vec<i64>, String> {
    if tickets.is_empty() || tickets.len() > 20 {
        return Err("A ticket plan must contain 1-20 tickets".into());
    }
    let tickets = tickets
        .into_iter()
        .map(|ticket| {
            normalize_ticket_input(TicketInput {
                feature_id: Some(feature_id),
                ..ticket
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let tx = conn.transaction().map_err(sql_error)?;
    let ids = tickets
        .iter()
        .map(|ticket| insert_ticket(&tx, project_id, ticket, "agent"))
        .collect::<Result<Vec<_>, _>>()?;
    tx.commit().map_err(sql_error)?;
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SCHEMA, canonical_project};

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        let root = canonical_project(&std::env::current_dir().unwrap()).unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, root) VALUES (1, 'test', ?1)",
            [&root],
        )
        .unwrap();
        let nodes = json!([
            {"id":"a","type":"component","position":{"x":0,"y":0},"data":{"title":"A","summary":"Owns A","responsibilities":["Provide A"],"status":"Not started"}},
            {"id":"storage","type":"component","position":{"x":10,"y":0},"data":{"title":"Storage","summary":"Persists records","responsibilities":["Persist records"],"status":"Not started"}},
            {"id":"ui","type":"component","position":{"x":20,"y":0},"data":{"title":"UI","summary":"Shows records","responsibilities":["Render"],"status":"Not started"}}
        ]);
        let edges = json!([
            {"id":"a-storage","source":"a","target":"storage","data":{"relation":"depends_on"}}
        ]);
        conn.execute(
            "INSERT INTO project_maps (project_id, nodes, edges, revision) VALUES (1, ?1, ?2, 1)",
            [nodes.to_string(), edges.to_string()],
        )
        .unwrap();
        conn
    }

    fn approve_map(conn: &Connection) {
        conn.execute(
            "INSERT INTO design_drafts (project_id, base_revision, version, draft, state) VALUES (1, 0, 1, '{}', 'accepted')",
            [],
        )
        .unwrap();
    }

    fn input(feature_id: Option<i64>, title: &str, node_ids: &[&str]) -> TicketInput {
        TicketInput {
            feature_id,
            title: title.into(),
            goal: format!("Implement {title}"),
            completion_criteria: vec!["It works".into(), "It persists".into()],
            node_ids: node_ids.iter().map(|id| (*id).to_owned()).collect(),
        }
    }

    fn report(outcome: &str) -> Value {
        json!({
            "kind":"development_report",
            "outcome":outcome,
            "summary":"Done.",
            "criteria":[
                {"index":0,"status":"met","evidence":["Agent report"]},
                {"index":1,"status":"met","evidence":["Agent report"]}
            ],
            "changes":[],
            "checks":[],
            "blockers":[]
        })
    }

    #[test]
    fn ticket_input_is_validated_and_node_links_must_exist() {
        let mut conn = setup();
        assert!(
            normalize_ticket_input(TicketInput {
                title: " ".into(),
                ..input(None, "x", &[])
            })
            .is_err()
        );
        assert!(
            normalize_ticket_input(TicketInput {
                completion_criteria: vec![],
                ..input(None, "x", &[])
            })
            .is_err()
        );
        let normalized =
            normalize_ticket_input(input(None, " Trim ", &["a", "a", "storage"])).unwrap();
        assert_eq!(normalized.title, "Trim");
        assert_eq!(normalized.node_ids, vec!["a", "storage"]);
        assert!(
            create_ticket_in_db(&mut conn, 1, input(None, "Bad", &["missing"]), "user").is_err()
        );
        assert!(create_ticket_in_db(&mut conn, 1, input(Some(99), "Bad", &[]), "user").is_err());
        let id =
            create_ticket_in_db(&mut conn, 1, input(None, "Good", &["a", "ui"]), "user").unwrap();
        let ticket = load_ticket(&conn, 1, id).unwrap();
        assert_eq!(ticket.node_ids, vec!["a", "ui"]);
        assert_eq!(ticket.status, "todo");
        assert_eq!(ticket.source, "user");
    }

    #[test]
    fn next_ticket_follows_feature_then_ticket_order_without_the_map() {
        let mut conn = setup();
        let first = create_feature_in_db(&conn, 1, "Accounts", "").unwrap();
        let second = create_feature_in_db(&conn, 1, "Records", "").unwrap();
        let loose = create_ticket_in_db(&mut conn, 1, input(None, "Loose", &[]), "user").unwrap();
        let r1 = create_ticket_in_db(
            &mut conn,
            1,
            input(Some(second), "R1", &["storage"]),
            "user",
        )
        .unwrap();
        let a1 =
            create_ticket_in_db(&mut conn, 1, input(Some(first), "A1", &["a"]), "user").unwrap();
        let a2 =
            create_ticket_in_db(&mut conn, 1, input(Some(first), "A2", &["a"]), "user").unwrap();
        assert_eq!(next_ticket_id(&conn, 1).unwrap(), Some(a1));

        move_ticket_in_db(&mut conn, 1, a2, true).unwrap();
        assert_eq!(next_ticket_id(&conn, 1).unwrap(), Some(a2));
        move_feature_in_db(&mut conn, 1, second, true).unwrap();
        assert_eq!(next_ticket_id(&conn, 1).unwrap(), Some(r1));

        // Map revisions do not affect the queue.
        conn.execute(
            "UPDATE project_maps SET revision = revision + 5 WHERE project_id = 1",
            [],
        )
        .unwrap();
        assert_eq!(next_ticket_id(&conn, 1).unwrap(), Some(r1));

        for id in [r1, a2, a1] {
            set_ticket_status_in_db(&conn, 1, id, "done").unwrap();
        }
        assert_eq!(next_ticket_id(&conn, 1).unwrap(), Some(loose));
        let board = ticket_board_in_db(&conn, 1).unwrap();
        assert!(board.features.iter().all(|feature| feature.completed));
        assert!(set_ticket_status_in_db(&conn, 1, loose, "in_progress").is_err());

        // A done ticket can be reopened; deleting a feature keeps its tickets.
        set_ticket_status_in_db(&conn, 1, a1, "todo").unwrap();
        assert!(!ticket_board_in_db(&conn, 1).unwrap().features[1].completed);
        delete_feature_in_db(&mut conn, 1, first).unwrap();
        let ticket = load_ticket(&conn, 1, a1).unwrap();
        assert_eq!(ticket.feature_id, None);
        assert_eq!(next_ticket_id(&conn, 1).unwrap(), Some(loose));
    }

    #[test]
    fn ticket_run_injects_map_context_and_review_decides_status() {
        let mut conn = setup();
        let id = create_ticket_in_db(&mut conn, 1, input(None, "Create", &["a"]), "user").unwrap();
        assert!(
            begin_ticket_run(&mut conn, 1, id).is_err(),
            "map must be approved"
        );
        approve_map(&conn);

        let context = begin_ticket_run(&mut conn, 1, id).unwrap();
        assert_eq!(context.map_revision, 1);
        assert_eq!(context.target_nodes[0]["id"], "a");
        assert_eq!(context.related_nodes[0]["id"], "storage");
        assert_eq!(context.related_edges[0]["data"]["relation"], "depends_on");
        assert_eq!(context.map_overview.len(), 3);
        assert_eq!(load_ticket(&conn, 1, id).unwrap().status, "in_progress");
        assert!(
            begin_ticket_run(&mut conn, 1, id).is_err(),
            "one active run per ticket"
        );
        assert!(update_ticket_in_db(&mut conn, 1, id, input(None, "Edit", &[])).is_err());
        assert!(delete_ticket_in_db(&conn, 1, id).is_err());

        let run_id = context.run_id;
        assert!(transition_ticket_run(&conn, run_id, "running", None).unwrap());
        let mut bad = report("completed");
        bad["criteria"].as_array_mut().unwrap().pop();
        assert!(save_ticket_run_report(&mut conn, run_id, &bad).is_err());
        save_ticket_run_report(&mut conn, run_id, &report("completed")).unwrap();
        record_ticket_run_evidence(
            &conn,
            run_id,
            "workspace_snapshot",
            &json!({"available":true,"changedFiles":["src/a.rs"]}),
        )
        .unwrap();
        assert!(
            save_ticket_review_in_db(&mut conn, 1, id, run_id, "approved_completion", None)
                .is_err(),
            "cannot review a running run"
        );
        assert!(transition_ticket_run(&conn, run_id, "finished", None).unwrap());
        assert_eq!(load_ticket(&conn, 1, id).unwrap().status, "in_progress");
        assert!(!transition_ticket_run(&conn, run_id, "disconnected", None).unwrap());

        let review = ticket_run_review_in_db(&conn, 1, run_id).unwrap();
        assert_eq!(review.ticket_id, id);
        assert_eq!(review.report.unwrap()["outcome"], "completed");
        assert_eq!(review.evidence.len(), 1);
        assert!(ticket_run_review_in_db(&conn, 2, run_id).is_err());

        let rework =
            save_ticket_review_in_db(&mut conn, 1, id, run_id, "rework_requested", Some("Fix it"))
                .unwrap();
        assert_eq!(rework.ticket_status, "todo");
        assert_eq!(next_ticket_id(&conn, 1).unwrap(), Some(id));
        let approved =
            save_ticket_review_in_db(&mut conn, 1, id, run_id, "approved_completion", None)
                .unwrap();
        assert_eq!(approved.ticket_status, "done");
        let ticket = load_ticket(&conn, 1, id).unwrap();
        assert_eq!(
            ticket.review_decision.as_deref(),
            Some("approved_completion")
        );
        assert_eq!(ticket.report_outcome.as_deref(), Some("completed"));
        assert!(
            begin_ticket_run(&mut conn, 1, id).is_err(),
            "done tickets do not restart"
        );

        // The confirmed map is never touched by ticket progress.
        let nodes: String = conn
            .query_row(
                "SELECT nodes FROM project_maps WHERE project_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&nodes).unwrap()[0]["data"]["status"],
            "Not started"
        );
    }

    #[test]
    fn failed_runs_return_the_ticket_to_the_queue_and_removed_nodes_are_reported() {
        let mut conn = setup();
        approve_map(&conn);
        let id =
            create_ticket_in_db(&mut conn, 1, input(None, "Create", &["a", "ui"]), "user").unwrap();
        for state in ["start_failed", "failed", "interrupted", "disconnected"] {
            let context = begin_ticket_run(&mut conn, 1, id).unwrap();
            assert!(transition_ticket_run(&conn, context.run_id, state, Some("x")).unwrap());
            assert_eq!(load_ticket(&conn, 1, id).unwrap().status, "todo");
        }

        conn.execute(
            "UPDATE project_maps SET nodes = json_remove(nodes, '$[2]') WHERE project_id = 1",
            [],
        )
        .unwrap();
        let context = begin_ticket_run(&mut conn, 1, id).unwrap();
        assert_eq!(context.missing_node_ids, vec!["ui"]);
        transition_ticket_run(&conn, context.run_id, "failed", None).unwrap();
        // Keeping a link to a removed node is allowed; adding a new unknown link is not.
        assert!(update_ticket_in_db(&mut conn, 1, id, input(None, "Edit", &["a", "ui"])).is_ok());
        assert!(
            update_ticket_in_db(&mut conn, 1, id, input(None, "Edit", &["a", "nope"])).is_err()
        );

        conn.execute(
            "UPDATE project_maps SET nodes = json_set(nodes, '$[0].data.questions', json('[{\"prompt\":\"Q?\",\"options\":[]}]')) WHERE project_id = 1",
            [],
        )
        .unwrap();
        assert!(begin_ticket_run(&mut conn, 1, id).is_err());
    }

    #[test]
    fn planned_tickets_are_appended_to_the_feature_as_agent_tickets() {
        let mut conn = setup();
        approve_map(&conn);
        let feature = create_feature_in_db(&conn, 1, "Records", "Users keep records").unwrap();
        create_ticket_in_db(&mut conn, 1, input(Some(feature), "Existing", &[]), "user").unwrap();
        let context = feature_plan_context(&conn, 1, feature).unwrap();
        assert_eq!(context.existing_tickets.len(), 1);
        assert_eq!(context.feature["title"], "Records");
        assert!(store_planned_tickets(&mut conn, 1, feature, vec![]).is_err());
        assert!(
            store_planned_tickets(
                &mut conn,
                1,
                feature,
                vec![input(None, "Bad", &["missing"])]
            )
            .is_err()
        );
        let ids = store_planned_tickets(
            &mut conn,
            1,
            feature,
            vec![
                input(None, "First", &["storage"]),
                input(None, "Second", &["a"]),
            ],
        )
        .unwrap();
        let board = ticket_board_in_db(&conn, 1).unwrap();
        let planned = board
            .tickets
            .iter()
            .filter(|ticket| ids.contains(&ticket.id))
            .collect::<Vec<_>>();
        assert_eq!(planned.len(), 2);
        assert!(
            planned
                .iter()
                .all(|ticket| ticket.source == "agent" && ticket.feature_id == Some(feature))
        );
        assert_eq!(planned[0].title, "First");
        assert_eq!(board.features[0].ticket_count, 3);
    }
}
