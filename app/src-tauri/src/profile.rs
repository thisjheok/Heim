use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use tauri::AppHandle;

use crate::{database, ensure_project};

/// What the project produces and with which technologies. It is decided with the map
/// (from the user's words, the existing files, or an AI proposal) and confirmed only
/// when the user approves the map draft.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectProfile {
    pub output_form: String,
    #[serde(deserialize_with = "deserialize_stack")]
    pub stack: Vec<StackArea>,
    pub constraints: Vec<String>,
    pub source: String,
    pub rationale: String,
}

/// One part of the project (named freely by the Agent, e.g. "Frontend", "Backend", "Database")
/// and the technologies used there. A single-stack project has one area.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StackArea {
    pub area: String,
    pub items: Vec<String>,
}

/// Profiles stored before contract v4 kept a flat technology list; read it as one area.
fn deserialize_stack<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<StackArea>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Stack {
        Areas(Vec<StackArea>),
        Flat(Vec<String>),
    }
    Ok(match Stack::deserialize(deserializer)? {
        Stack::Areas(areas) => areas,
        Stack::Flat(items) if items.is_empty() => vec![],
        Stack::Flat(items) => vec![StackArea {
            area: "Stack".into(),
            items,
        }],
    })
}

pub(crate) const PROFILE_SOURCES: [&str; 3] = ["user_specified", "detected", "ai_proposed"];

pub(crate) fn validate_profile(profile: &ProjectProfile) -> Result<(), String> {
    if profile.output_form.trim().is_empty() || profile.output_form.chars().count() > 200 {
        return Err("Project output form must contain 1-200 characters".into());
    }
    if profile.stack.is_empty() || profile.stack.len() > 10 {
        return Err("Project stack must have 1-10 areas".into());
    }
    let mut area_names = std::collections::HashSet::new();
    for area in &profile.stack {
        if area.area.trim().is_empty()
            || area.area.chars().count() > 50
            || !area_names.insert(area.area.trim().to_lowercase())
        {
            return Err("Project stack areas need unique names of 1-50 characters".into());
        }
        if area.items.is_empty()
            || area.items.len() > 20
            || area
                .items
                .iter()
                .any(|item| item.trim().is_empty() || item.chars().count() > 100)
        {
            return Err(format!(
                "Project stack area {} must list 1-20 technologies of 1-100 characters",
                area.area
            ));
        }
    }
    if profile.constraints.len() > 20
        || profile
            .constraints
            .iter()
            .any(|item| item.trim().is_empty() || item.chars().count() > 500)
    {
        return Err("Project constraints must be at most 20 items of 1-500 characters".into());
    }
    if !PROFILE_SOURCES.contains(&profile.source.as_str()) {
        return Err("Project profile source is invalid".into());
    }
    if profile.rationale.chars().count() > 2_000
        || (profile.source != "user_specified" && profile.rationale.trim().is_empty())
    {
        return Err("Explain detected or proposed project profiles in 1-2000 characters".into());
    }
    Ok(())
}

pub(crate) fn parse_profile(value: &Value) -> Result<ProjectProfile, String> {
    let strings = |key: &str| -> Result<Vec<String>, String> {
        value[key]
            .as_array()
            .ok_or_else(|| format!("Project profile {key} must be an array"))?
            .iter()
            .map(|item| {
                item.as_str()
                    .map(|text| text.trim().to_owned())
                    .ok_or_else(|| format!("Project profile {key} must contain strings"))
            })
            .collect()
    };
    let text = |key: &str| -> Result<String, String> {
        value[key]
            .as_str()
            .map(|text| text.trim().to_owned())
            .ok_or_else(|| format!("Project profile {key} is missing"))
    };
    let stack = value["stack"]
        .as_array()
        .ok_or("Project profile stack must be an array of areas")?
        .iter()
        .map(|area| {
            let items = area["items"]
                .as_array()
                .ok_or("Project stack area items must be an array")?
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(|text| text.trim().to_owned())
                        .ok_or("Project stack area items must be strings")
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(StackArea {
                area: area["area"]
                    .as_str()
                    .ok_or("Project stack area name is missing")?
                    .trim()
                    .to_owned(),
                items,
            })
        })
        .collect::<Result<Vec<_>, &str>>()?;
    let profile = ProjectProfile {
        output_form: text("outputForm")?,
        stack,
        constraints: strings("constraints")?,
        source: text("source")?,
        rationale: text("rationale")?,
    };
    validate_profile(&profile)?;
    Ok(profile)
}

pub(crate) fn load_project_profile(
    conn: &Connection,
    project_id: i64,
) -> Result<Option<ProjectProfile>, String> {
    conn.query_row(
        "SELECT profile FROM project_profiles WHERE project_id = ?1",
        [project_id],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(|error| error.to_string())?
    .map(|profile| serde_json::from_str(&profile).map_err(|error| error.to_string()))
    .transpose()
}

pub(crate) fn store_project_profile(
    conn: &Connection,
    project_id: i64,
    profile: &ProjectProfile,
) -> Result<(), String> {
    validate_profile(profile)?;
    conn.execute(
        "INSERT INTO project_profiles (project_id, profile) VALUES (?1, ?2)
         ON CONFLICT(project_id) DO UPDATE SET profile = excluded.profile, updated_at = CURRENT_TIMESTAMP",
        params![
            project_id,
            serde_json::to_string(profile).map_err(|error| error.to_string())?
        ],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn project_profile(app: AppHandle, project_id: i64) -> Result<Option<ProjectProfile>, String> {
    let conn = database(&app)?;
    ensure_project(&conn, project_id)?;
    load_project_profile(&conn, project_id)
}

/// Prompt section that pins every Agent turn to the confirmed output form and stack.
pub(crate) fn profile_prompt_section(profile: Option<&ProjectProfile>) -> String {
    match profile {
        Some(profile) => format!(
            "Confirmed project profile (authoritative; the user approved it with the map). Build the output form below with exactly this stack and these constraints; work in each area uses that area's technologies. Do not introduce another language, framework, or runtime unless the ticket explicitly asks for it; if the work seems to need one, report it as a blocker instead.\n{}",
            serde_json::to_string_pretty(&json!({
                "outputForm": profile.output_form,
                "stack": profile.stack,
                "constraints": profile.constraints,
            }))
            .unwrap_or_default()
        ),
        None => "No confirmed project profile. Follow the language and frameworks already used in the workspace. If the workspace has no code yet, do not pick a stack silently: report the outcome as blocked with the blocker \"Project stack is not confirmed\".".into(),
    }
}

/// Instructions shared by initial and revised map drafts for the required `profile` field.
pub(crate) const PROFILE_DRAFT_RULES: &str = "The draft must include \"profile\":{\"outputForm\":string,\"stack\":[{\"area\":string,\"items\":[string]}],\"constraints\":[string],\"source\":\"user_specified\"|\"detected\"|\"ai_proposed\",\"rationale\":string}. outputForm is the kind of deliverable (for example \"Web frontend (single-page app in the browser)\", \"Full-stack web app\", \"Desktop app\", \"CLI tool\", \"HTTP API server\", \"Library\"). Decide it in this order: (1) if the user's words name the deliverable or technologies, use exactly those and set source user_specified, filling any unnamed parts with conventional choices that fit and saying so in rationale; (2) otherwise, if existing project files reveal the stack, use it and set source detected; (3) otherwise propose the stack that best fits the described product and set source ai_proposed, explaining why in rationale. Match the deliverable to what the user will see: a website or shop the user browses means a browser frontend, not server-side scripts that print HTML. Group the stack by area: name each area freely after the part of the project it covers (for example \"Frontend\", \"Backend\", \"Database\", \"Mobile\", \"Infrastructure\"), list that area's languages, frameworks, and tools in items, and use a single area when the whole project shares one stack. Never leave the stack undecided.";

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> ProjectProfile {
        ProjectProfile {
            output_form: "Web frontend (single-page app in the browser)".into(),
            stack: vec![
                StackArea {
                    area: "Frontend".into(),
                    items: vec!["React".into(), "TypeScript".into()],
                },
                StackArea {
                    area: "Backend".into(),
                    items: vec!["Java".into(), "Spring Boot".into()],
                },
            ],
            constraints: vec!["Mock product data, no backend".into()],
            source: "ai_proposed".into(),
            rationale: "A shopping site is browsed in a web browser.".into(),
        }
    }

    #[test]
    fn profile_requires_output_form_stack_and_a_reason_for_ai_choices() {
        assert!(validate_profile(&profile()).is_ok());
        assert!(
            validate_profile(&ProjectProfile {
                stack: vec![],
                ..profile()
            })
            .is_err()
        );
        let duplicate_area = ProjectProfile {
            stack: vec![profile().stack[0].clone(), profile().stack[0].clone()],
            ..profile()
        };
        assert!(validate_profile(&duplicate_area).is_err());
        let empty_area = ProjectProfile {
            stack: vec![StackArea {
                area: "Frontend".into(),
                items: vec![],
            }],
            ..profile()
        };
        assert!(validate_profile(&empty_area).is_err());
        assert!(
            validate_profile(&ProjectProfile {
                output_form: " ".into(),
                ..profile()
            })
            .is_err()
        );
        assert!(
            validate_profile(&ProjectProfile {
                source: "guess".into(),
                ..profile()
            })
            .is_err()
        );
        assert!(
            validate_profile(&ProjectProfile {
                rationale: "".into(),
                ..profile()
            })
            .is_err()
        );
        assert!(
            validate_profile(&ProjectProfile {
                source: "user_specified".into(),
                rationale: "".into(),
                ..profile()
            })
            .is_ok()
        );

        let parsed = parse_profile(&json!({
            "outputForm":" Web frontend ",
            "stack":[{"area":" Frontend ","items":["HTML","CSS","JavaScript"]}],
            "constraints":[],
            "source":"user_specified",
            "rationale":""
        }))
        .unwrap();
        assert_eq!(parsed.output_form, "Web frontend");
        assert_eq!(parsed.stack[0].area, "Frontend");
        assert!(parse_profile(&json!({"outputForm":"Web","stack":"React","constraints":[],"source":"detected","rationale":"x"})).is_err());
        assert!(parse_profile(&json!({"outputForm":"Web","stack":["React"],"constraints":[],"source":"detected","rationale":"x"})).is_err());
    }

    #[test]
    fn stored_flat_stack_reads_as_one_area() {
        let stored: ProjectProfile = serde_json::from_value(json!({
            "outputForm":"Web frontend",
            "stack":["React","TypeScript"],
            "constraints":[],
            "source":"ai_proposed",
            "rationale":"Browser app."
        }))
        .unwrap();
        assert_eq!(
            stored.stack,
            vec![StackArea {
                area: "Stack".into(),
                items: vec!["React".into(), "TypeScript".into()],
            }]
        );
        let round_trip: ProjectProfile =
            serde_json::from_str(&serde_json::to_string(&profile()).unwrap()).unwrap();
        assert_eq!(round_trip, profile());
    }

    #[test]
    fn prompt_section_pins_the_stack_or_blocks_silent_choices() {
        let pinned = profile_prompt_section(Some(&profile()));
        assert!(
            pinned.contains("\"Spring Boot\"")
                && pinned.contains("\"Backend\"")
                && pinned.contains("Do not introduce another language")
        );
        assert!(profile_prompt_section(None).contains("Project stack is not confirmed"));
    }
}
