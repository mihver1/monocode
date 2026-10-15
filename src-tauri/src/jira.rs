use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
const DEFAULT_LIMIT: u32 = 40;
const COMMENT_LIMIT: u32 = 50;
const ISSUE_FIELDS: [&str; 6] = [
    "summary", "status", "updated", "labels", "assignee", "project",
];

/// Cloud uses REST API v3 and an Atlassian API token. Data Center uses REST API
/// v2 and a personal access token. Missing configs stay on Cloud.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum JiraDeployment {
    #[default]
    Cloud,
    DataCenter,
}

impl JiraDeployment {
    fn parse(raw: Option<&str>) -> Result<Self, String> {
        match raw.unwrap_or("cloud").trim().to_ascii_lowercase().as_str() {
            "" | "cloud" => Ok(Self::Cloud),
            "datacenter" => Ok(Self::DataCenter),
            _ => Err("Jira deployment must be cloud or datacenter".into()),
        }
    }

    fn api_base(self) -> &'static str {
        match self {
            Self::Cloud => "/rest/api/3",
            Self::DataCenter => "/rest/api/2",
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JiraStatus {
    pub connected: bool,
    pub site: String,
    pub email: String,
    pub deployment: JiraDeployment,
}

#[derive(Serialize, Deserialize, Clone)]
struct JiraConfig {
    site: String,
    email: String,
    token: String,
    #[serde(default)]
    deployment: JiraDeployment,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JiraProject {
    pub id: String,
    pub key: String,
    pub name: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JiraLabel {
    pub name: String,
    pub color: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JiraAssignee {
    pub login: String,
    pub avatar_url: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JiraIssue {
    pub provider: String,
    pub kind: String,
    pub id: String,
    pub identifier: String,
    pub number: i64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub state_type: String,
    pub updated_at: String,
    pub labels: Vec<JiraLabel>,
    pub assignees: Vec<JiraAssignee>,
    pub draft: bool,
    pub repo: String,
    pub team_id: String,
    pub team_name: String,
    pub project_path: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JiraIssueDetails {
    pub body: String,
    pub author: String,
    pub author_avatar_url: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JiraIssueComment {
    pub id: String,
    pub kind: String,
    pub author: String,
    pub author_avatar_url: String,
    pub body: String,
    pub created_at: String,
    pub url: String,
    pub state: String,
    pub path: String,
    pub line: Option<i64>,
    pub resolved: bool,
    pub thread_id: String,
    pub replies: Vec<JiraIssueComment>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JiraIssueThread {
    pub comments: Vec<JiraIssueComment>,
    pub truncated: bool,
    pub review_decision: String,
    pub base_ref_name: String,
    pub head_ref_name: String,
}

#[tauri::command(async)]
pub fn jira_status(app: AppHandle) -> Result<JiraStatus, String> {
    let config = read_config(&app)?;
    Ok(status_for(config.as_ref()))
}

#[tauri::command]
pub async fn jira_set_config(
    app: AppHandle,
    site: String,
    email: String,
    token: String,
    deployment: Option<String>,
) -> Result<JiraStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let token = token.trim().to_string();
        if token.is_empty() {
            delete_config(&app)?;
            return Ok(status_for(None));
        }
        if token.chars().any(char::is_control) {
            return Err("Jira token is invalid".into());
        }
        let deployment = JiraDeployment::parse(deployment.as_deref())?;
        let email = email.trim().to_string();
        if deployment == JiraDeployment::Cloud && (email.is_empty() || !email.contains('@')) {
            return Err("Enter the email address of your Atlassian account".into());
        }
        let mut config = JiraConfig {
            site: normalize_jira_site(&site, deployment)?,
            email,
            token,
            deployment,
        };
        let myself = jira_get(&config, &format!("{}/myself", deployment.api_base()))?;
        match deployment {
            JiraDeployment::Cloud => {
                if string_field(&myself, "accountId")
                    .unwrap_or_default()
                    .is_empty()
                {
                    return Err("Jira did not return the current user".into());
                }
            }
            JiraDeployment::DataCenter => {
                config.email = datacenter_account(&myself)?;
            }
        }
        write_config(&app, &config)?;
        Ok(status_for(Some(&config)))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn jira_list_projects(app: AppHandle) -> Result<Vec<JiraProject>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let config = require_config(&app)?;
        fetch_jira_projects(|start| {
            jira_get(
                &config,
                &format!(
                    "{}/project/search?maxResults=100&orderBy=name&startAt={start}",
                    config.deployment.api_base()
                ),
            )
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn jira_list_issues(
    app: AppHandle,
    assigned_to_me: bool,
    state: String,
    project_ids: Vec<String>,
    limit: Option<u32>,
) -> Result<Vec<JiraIssue>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(config) = read_config(&app)? else {
            return Ok(Vec::new());
        };
        let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, 100);
        let jql = issue_jql(assigned_to_me, &state, &project_ids);
        let (path, body) = issue_search_request(config.deployment, &jql, limit);
        let data = jira_post(&config, path, &body)?;
        parse_jira_issues(&data, &config.site)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn jira_issue_details(app: AppHandle, key: String) -> Result<JiraIssueDetails, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let config = require_config(&app)?;
        let key = require_issue_key(&key)?;
        let data = jira_get(
            &config,
            &format!(
                "{}/issue/{key}?fields=description,reporter,creator,assignee",
                config.deployment.api_base()
            ),
        )?;
        parse_jira_issue_details(&data, &config.site, config.deployment)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn jira_issue_thread(app: AppHandle, key: String) -> Result<JiraIssueThread, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let config = require_config(&app)?;
        let key = require_issue_key(&key)?;
        // Newest first so a long thread keeps its latest comments; re-sorted below.
        let data = jira_get(
            &config,
            &format!(
                "{}/issue/{key}/comment?maxResults={COMMENT_LIMIT}&orderBy=-created",
                config.deployment.api_base()
            ),
        )?;
        parse_jira_issue_thread(&data, &config.site, key, config.deployment)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn jira_issue_comment(
    app: AppHandle,
    key: String,
    body: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let config = require_config(&app)?;
        let key = require_issue_key(&key)?;
        let body = body.trim();
        if body.is_empty() {
            return Err("Comment cannot be empty".into());
        }
        let data = jira_post(
            &config,
            &format!("{}/issue/{key}/comment", config.deployment.api_base()),
            &comment_request_body(config.deployment, body),
        )?;
        let id = string_field(&data, "id").unwrap_or_default();
        if id.is_empty() {
            return Err("Could not post Jira comment".into());
        }
        Ok(comment_url(&config.site, key, &id))
    })
    .await
    .map_err(|error| error.to_string())?
}

fn status_for(config: Option<&JiraConfig>) -> JiraStatus {
    JiraStatus {
        connected: config.is_some(),
        site: config.map(|config| config.site.clone()).unwrap_or_default(),
        email: config
            .map(|config| config.email.clone())
            .unwrap_or_default(),
        deployment: config.map(|config| config.deployment).unwrap_or_default(),
    }
}

/// Data Center identifies the user by username. `name` is the login; older
/// responses sometimes only include `key` or `displayName`.
fn datacenter_account(myself: &Value) -> Result<String, String> {
    string_field(myself, "name")
        .filter(|name| !name.is_empty())
        .or_else(|| string_field(myself, "displayName").filter(|name| !name.is_empty()))
        .or_else(|| string_field(myself, "key").filter(|name| !name.is_empty()))
        .ok_or_else(|| "Jira did not return the current user".to_string())
}

fn issue_search_request(
    deployment: JiraDeployment,
    jql: &str,
    limit: u32,
) -> (&'static str, Value) {
    match deployment {
        // Cloud's enhanced search replaced `/rest/api/3/search`.
        JiraDeployment::Cloud => (
            "/rest/api/3/search/jql",
            json!({ "jql": jql, "maxResults": limit, "fields": ISSUE_FIELDS }),
        ),
        JiraDeployment::DataCenter => (
            "/rest/api/2/search",
            json!({ "jql": jql, "startAt": 0, "maxResults": limit, "fields": ISSUE_FIELDS }),
        ),
    }
}

fn comment_request_body(deployment: JiraDeployment, body: &str) -> Value {
    match deployment {
        JiraDeployment::Cloud => json!({ "body": text_to_adf(body) }),
        // Data Center stores comments as wiki markup. Plain text is valid wiki.
        JiraDeployment::DataCenter => json!({ "body": body }),
    }
}

fn issue_jql(assigned_to_me: bool, state: &str, project_ids: &[String]) -> String {
    let mut clauses = Vec::new();
    if assigned_to_me {
        clauses.push("assignee = currentUser()".to_string());
    }
    let ids: Vec<&str> = project_ids
        .iter()
        .map(|id| id.trim())
        .filter(|id| !id.is_empty() && id.chars().all(|ch| ch.is_ascii_digit()))
        .collect();
    if !ids.is_empty() {
        clauses.push(format!("project in ({})", ids.join(", ")));
    }
    if !state.trim().eq_ignore_ascii_case("all") {
        clauses.push("statusCategory != Done".to_string());
    }
    // The search endpoint rejects unbounded JQL, so an unfiltered query is
    // limited to the last year of activity.
    if clauses.is_empty() {
        clauses.push("updated >= -365d".to_string());
    }
    format!("{} ORDER BY updated DESC", clauses.join(" AND "))
}

fn jira_authorization(config: &JiraConfig) -> String {
    match config.deployment {
        JiraDeployment::DataCenter => format!("Bearer {}", config.token.trim()),
        JiraDeployment::Cloud => {
            let encoded = base64::engine::general_purpose::STANDARD.encode(format!(
                "{}:{}",
                config.email.trim(),
                config.token.trim()
            ));
            format!("Basic {encoded}")
        }
    }
}

fn jira_get(config: &JiraConfig, path: &str) -> Result<Value, String> {
    let agent = ureq::AgentBuilder::new().timeout(HTTP_TIMEOUT).build();
    let result = agent
        .get(&format!("{}{path}", config.site))
        .set("Authorization", &jira_authorization(config))
        .set("Accept", "application/json")
        .call();
    read_jira_response(config.deployment, result)
}

fn jira_post(config: &JiraConfig, path: &str, body: &Value) -> Result<Value, String> {
    let agent = ureq::AgentBuilder::new().timeout(HTTP_TIMEOUT).build();
    let payload = serde_json::to_string(body).map_err(|error| error.to_string())?;
    let result = agent
        .post(&format!("{}{path}", config.site))
        .set("Authorization", &jira_authorization(config))
        .set("Accept", "application/json")
        .set("Content-Type", "application/json")
        .send_string(&payload);
    read_jira_response(config.deployment, result)
}

fn read_jira_response(
    deployment: JiraDeployment,
    result: Result<ureq::Response, ureq::Error>,
) -> Result<Value, String> {
    let response = match result {
        Ok(response) => response,
        Err(ureq::Error::Status(401, _)) => return Err(unauthorized_message(deployment)),
        Err(ureq::Error::Status(status, response)) => {
            let body = response.into_string().unwrap_or_default();
            return Err(jira_http_error(deployment, status, &body));
        }
        Err(_) => return Err("Could not reach Jira".into()),
    };
    let status = response.status();
    let body = response
        .into_string()
        .map_err(|_| "Jira returned an unreadable response".to_string())?;
    if !(200..300).contains(&status) {
        return Err(jira_http_error(deployment, status, &body));
    }
    serde_json::from_str(&body).map_err(|_| "Jira returned invalid JSON".to_string())
}

fn unauthorized_message(deployment: JiraDeployment) -> String {
    match deployment {
        JiraDeployment::Cloud => "Jira email or API token is invalid".into(),
        JiraDeployment::DataCenter => "Jira personal access token is invalid".into(),
    }
}

fn jira_http_error(deployment: JiraDeployment, status: u16, body: &str) -> String {
    if let Some(message) = jira_error_message(body) {
        return message;
    }
    match status {
        403 => match deployment {
            JiraDeployment::Cloud => "Jira denied access. Check the API token's permissions".into(),
            JiraDeployment::DataCenter => {
                "Jira denied access. Check the personal access token's permissions".into()
            }
        },
        404 => "Jira could not find that issue".into(),
        _ => format!("Jira request failed ({status})"),
    }
}

fn jira_error_message(body: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(body).ok()?;
    let from_list = parsed
        .get("errorMessages")
        .and_then(Value::as_array)
        .and_then(|messages| messages.iter().filter_map(Value::as_str).next())
        .map(str::to_string);
    let from_map = || {
        parsed
            .get("errors")
            .and_then(Value::as_object)
            .and_then(|errors| errors.values().filter_map(Value::as_str).next())
            .map(str::to_string)
    };
    from_list
        .or_else(from_map)
        .map(|message| message.trim().to_string())
        .filter(|message| !message.is_empty())
}

fn parse_jira_projects(data: &Value) -> Result<Vec<JiraProject>, String> {
    let values = data
        .get("values")
        .and_then(Value::as_array)
        .ok_or_else(|| "Jira did not return projects".to_string())?;
    Ok(values
        .iter()
        .filter_map(|value| {
            let id = string_field(value, "id").filter(|id| !id.is_empty())?;
            let key = string_field(value, "key").unwrap_or_default();
            let name = string_field(value, "name")
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| key.clone());
            Some(JiraProject { id, key, name })
        })
        .collect())
}

// Follow every page: a partial project list would silently exclude visible
// projects when converting hidden projects into the JQL allowlist.
fn fetch_jira_projects(
    mut fetch: impl FnMut(u64) -> Result<Value, String>,
) -> Result<Vec<JiraProject>, String> {
    let mut start = 0;
    let mut projects = Vec::new();
    loop {
        let data = fetch(start)?;
        let page = parse_jira_projects(&data)?;
        let count = data["values"].as_array().map_or(0, Vec::len) as u64;
        projects.extend(page);
        if data["isLast"].as_bool() == Some(true) {
            break;
        }
        let next = data["startAt"].as_u64().unwrap_or(start) + count;
        if data["total"].as_u64().is_some_and(|total| next >= total) {
            break;
        }
        if count == 0 || next <= start {
            if data["isLast"].as_bool() == Some(false) {
                return Err("Jira project pagination did not advance".into());
            }
            break;
        }
        start = next;
    }
    let mut seen = std::collections::HashSet::new();
    projects.retain(|project| seen.insert(project.id.clone()));
    Ok(projects)
}

fn parse_jira_issues(data: &Value, site: &str) -> Result<Vec<JiraIssue>, String> {
    let issues = data
        .get("issues")
        .and_then(Value::as_array)
        .ok_or_else(|| "Jira did not return issues".to_string())?;
    Ok(issues
        .iter()
        .filter_map(|issue| parse_jira_issue(issue, site))
        .collect())
}

fn parse_jira_issue(node: &Value, site: &str) -> Option<JiraIssue> {
    let id = string_field(node, "id").filter(|id| !id.is_empty())?;
    let key = string_field(node, "key").filter(|key| !key.is_empty())?;
    let fields = node.get("fields").cloned().unwrap_or(Value::Null);
    let project = fields.get("project");
    let status = fields.get("status");
    let labels = fields
        .get("labels")
        .and_then(Value::as_array)
        .map(|labels| {
            labels
                .iter()
                .filter_map(Value::as_str)
                .map(|name| JiraLabel {
                    name: name.to_string(),
                    color: String::new(),
                })
                .collect()
        })
        .unwrap_or_default();
    let (assignee, avatar_url) = person_fields(fields.get("assignee"), site);
    Some(JiraIssue {
        provider: "jira".into(),
        kind: "jira".into(),
        number: issue_number(&key),
        url: format!("{site}/browse/{key}"),
        title: string_field(&fields, "summary").unwrap_or_default(),
        state: status
            .and_then(|value| string_field(value, "name"))
            .unwrap_or_else(|| "Open".into()),
        state_type: status
            .and_then(|value| value.pointer("/statusCategory/key"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        updated_at: normalize_jira_time(&string_field(&fields, "updated").unwrap_or_default()),
        labels,
        assignees: assignee
            .map(|login| vec![JiraAssignee { login, avatar_url }])
            .unwrap_or_default(),
        draft: false,
        repo: project
            .and_then(|value| string_field(value, "key"))
            .unwrap_or_default(),
        team_id: project
            .and_then(|value| string_field(value, "id"))
            .unwrap_or_default(),
        team_name: project
            .and_then(|value| string_field(value, "name"))
            .unwrap_or_default(),
        project_path: String::new(),
        id,
        identifier: key,
    })
}

fn parse_jira_issue_details(
    data: &Value,
    site: &str,
    deployment: JiraDeployment,
) -> Result<JiraIssueDetails, String> {
    let fields = data
        .get("fields")
        .ok_or_else(|| "Jira did not return that issue".to_string())?;
    let (author, author_avatar_url) = [fields.get("reporter"), fields.get("creator")]
        .into_iter()
        .map(|person| person_fields(person, site))
        .find(|(name, _)| name.is_some())
        .unwrap_or_else(|| person_fields(fields.get("assignee"), site));
    Ok(JiraIssueDetails {
        body: rich_text(fields.get("description"), deployment),
        author: author.unwrap_or_default(),
        author_avatar_url,
    })
}

fn parse_jira_issue_thread(
    data: &Value,
    site: &str,
    key: &str,
    deployment: JiraDeployment,
) -> Result<JiraIssueThread, String> {
    let comments = data
        .get("comments")
        .and_then(Value::as_array)
        .ok_or_else(|| "Jira did not return comments".to_string())?;
    let total = data
        .get("total")
        .and_then(Value::as_u64)
        .unwrap_or(comments.len() as u64);
    let mut parsed: Vec<JiraIssueComment> = comments
        .iter()
        .filter_map(|comment| parse_jira_comment(comment, site, key, deployment))
        .collect();
    parsed.sort_by(|left, right| left.created_at.cmp(&right.created_at));
    Ok(JiraIssueThread {
        truncated: total > comments.len() as u64,
        comments: parsed,
        review_decision: String::new(),
        base_ref_name: String::new(),
        head_ref_name: String::new(),
    })
}

fn parse_jira_comment(
    node: &Value,
    site: &str,
    key: &str,
    deployment: JiraDeployment,
) -> Option<JiraIssueComment> {
    let id = string_field(node, "id").filter(|id| !id.is_empty())?;
    let (author, author_avatar_url) = person_fields(node.get("author"), site);
    Some(JiraIssueComment {
        url: comment_url(site, key, &id),
        kind: "comment".into(),
        author: author.unwrap_or_default(),
        author_avatar_url,
        body: rich_text(node.get("body"), deployment),
        created_at: normalize_jira_time(&string_field(node, "created").unwrap_or_default()),
        state: String::new(),
        path: String::new(),
        line: None,
        resolved: false,
        thread_id: String::new(),
        replies: Vec::new(),
        id,
    })
}

fn comment_url(site: &str, key: &str, id: &str) -> String {
    format!("{site}/browse/{key}?focusedCommentId={id}")
}

fn issue_number(key: &str) -> i64 {
    key.rsplit('-')
        .next()
        .and_then(|number| number.parse().ok())
        .unwrap_or(0)
}

/// Jira sends `+0000` offsets, which WebKit's `Date.parse` rejects.
fn normalize_jira_time(value: &str) -> String {
    let bytes = value.as_bytes();
    let len = bytes.len();
    if len > 5
        && matches!(bytes[len - 5], b'+' | b'-')
        && bytes[len - 4..].iter().all(u8::is_ascii_digit)
    {
        return format!("{}:{}", &value[..len - 2], &value[len - 2..]);
    }
    value.to_string()
}

fn require_issue_key(key: &str) -> Result<&str, String> {
    let key = key.trim();
    let valid = !key.is_empty()
        && key.len() < 64
        && key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'));
    if valid {
        Ok(key)
    } else {
        Err("Missing Jira issue".into())
    }
}

fn person_fields(node: Option<&Value>, site: &str) -> (Option<String>, String) {
    let Some(node) = node.filter(|node| node.is_object()) else {
        return (None, String::new());
    };
    let avatar = node
        .pointer("/avatarUrls/48x48")
        .and_then(Value::as_str)
        .unwrap_or_default();
    (
        string_field(node, "displayName").filter(|name| !name.is_empty()),
        absolute_asset_url(site, avatar),
    )
}

fn absolute_asset_url(site: &str, url: &str) -> String {
    let url = url.trim();
    if url.is_empty() || url.contains("://") {
        return url.to_string();
    }
    if let Some(host) = url.strip_prefix("//") {
        return format!("https://{host}");
    }
    let path = url.trim_start_matches('/');
    format!("{site}/{path}")
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(|text| text.trim().to_string())
}

// ---- Atlassian Document Format ----

/// Cloud sends Atlassian Document Format. Data Center sends wiki markup. The
/// inbox renders Markdown either way.
fn rich_text(value: Option<&Value>, deployment: JiraDeployment) -> String {
    match value {
        Some(Value::String(text)) if deployment == JiraDeployment::DataCenter => {
            wiki_to_markdown(text)
        }
        Some(Value::String(text)) => text.trim().to_string(),
        Some(doc @ Value::Object(_)) => adf_to_markdown(doc),
        _ => String::new(),
    }
}

fn adf_to_markdown(doc: &Value) -> String {
    let mut blocks = Vec::new();
    for node in children(doc) {
        let block = adf_block(node, "");
        if !block.trim().is_empty() {
            blocks.push(block);
        }
    }
    blocks.join("\n\n").trim().to_string()
}

fn children(node: &Value) -> &[Value] {
    node.get("content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn node_type(node: &Value) -> &str {
    node.get("type").and_then(Value::as_str).unwrap_or_default()
}

fn attr<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    node.pointer(&format!("/attrs/{name}"))
}

fn attr_str<'a>(node: &'a Value, name: &str) -> &'a str {
    attr(node, name).and_then(Value::as_str).unwrap_or_default()
}

fn adf_block(node: &Value, indent: &str) -> String {
    match node_type(node) {
        "paragraph" => adf_inline(children(node)),
        "heading" => {
            let level = attr(node, "level")
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .clamp(1, 6) as usize;
            format!("{} {}", "#".repeat(level), adf_inline(children(node)))
        }
        "bulletList" => adf_list(node, indent, None),
        "orderedList" => {
            let start = attr(node, "order").and_then(Value::as_u64).unwrap_or(1);
            adf_list(node, indent, Some(start))
        }
        "codeBlock" => {
            let code: String = children(node)
                .iter()
                .filter_map(|child| child.get("text").and_then(Value::as_str))
                .collect();
            format!("```{}\n{}\n```", attr_str(node, "language"), code)
        }
        "blockquote" | "panel" => adf_blocks(children(node), indent)
            .lines()
            .map(|line| format!("> {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
        "rule" => "---".into(),
        "table" => adf_table(node),
        "mediaSingle" | "mediaGroup" | "media" => String::new(),
        _ => {
            if children(node).is_empty() {
                adf_inline(std::slice::from_ref(node))
            } else {
                adf_blocks(children(node), indent)
            }
        }
    }
}

fn adf_blocks(nodes: &[Value], indent: &str) -> String {
    nodes
        .iter()
        .map(|node| adf_block(node, indent))
        .filter(|block| !block.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn adf_list(node: &Value, indent: &str, start: Option<u64>) -> String {
    let mut lines = Vec::new();
    for (index, item) in children(node).iter().enumerate() {
        let marker = match start {
            Some(start) => format!("{}. ", start + index as u64),
            None => "- ".into(),
        };
        let nested = format!("{indent}{}", " ".repeat(marker.len()));
        let mut first = true;
        for child in children(item) {
            let text = adf_block(child, &nested);
            if text.trim().is_empty() {
                continue;
            }
            if matches!(node_type(child), "bulletList" | "orderedList") {
                lines.push(text);
            } else if first {
                lines.push(format!(
                    "{indent}{marker}{}",
                    text.replace('\n', &format!("\n{nested}"))
                ));
                first = false;
            } else {
                lines.push(format!(
                    "{nested}{}",
                    text.replace('\n', &format!("\n{nested}"))
                ));
            }
        }
    }
    lines.join("\n")
}

fn adf_table(node: &Value) -> String {
    let rows: Vec<Vec<String>> = children(node)
        .iter()
        .map(|row| {
            children(row)
                .iter()
                .map(|cell| {
                    adf_blocks(children(cell), "")
                        .replace('\n', " ")
                        .replace('|', "\\|")
                })
                .collect()
        })
        .filter(|row: &Vec<String>| !row.is_empty())
        .collect();
    let Some(width) = rows.iter().map(Vec::len).max() else {
        return String::new();
    };
    let line = |cells: &[String]| {
        let mut padded = cells.to_vec();
        padded.resize(width, String::new());
        format!("| {} |", padded.join(" | "))
    };
    let mut lines = vec![line(&rows[0]), format!("|{}", " --- |".repeat(width))];
    lines.extend(rows[1..].iter().map(|row| line(row)));
    lines.join("\n")
}

fn adf_inline(nodes: &[Value]) -> String {
    let mut out = String::new();
    for node in nodes {
        match node_type(node) {
            "text" => out.push_str(&adf_marked_text(node)),
            "hardBreak" => out.push('\n'),
            "mention" => {
                let text = attr_str(node, "text");
                if text.starts_with('@') {
                    out.push_str(text);
                } else if !text.is_empty() {
                    out.push('@');
                    out.push_str(text);
                }
            }
            "emoji" => {
                let text = attr_str(node, "text");
                out.push_str(if text.is_empty() {
                    attr_str(node, "shortName")
                } else {
                    text
                });
            }
            "inlineCard" | "blockCard" | "embedCard" => {
                let url = attr_str(node, "url");
                if !url.is_empty() {
                    out.push_str(&format!("<{url}>"));
                }
            }
            "status" => out.push_str(&format!("`{}`", attr_str(node, "text"))),
            "date" => {}
            _ => out.push_str(&adf_inline(children(node))),
        }
    }
    out
}

fn adf_marked_text(node: &Value) -> String {
    let mut text = node
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if text.is_empty() {
        return text;
    }
    let marks = node
        .get("marks")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if marks.iter().any(|mark| node_type(mark) == "code") {
        return format!("`{text}`");
    }
    for mark in marks {
        text = match node_type(mark) {
            "strong" => format!("**{text}**"),
            "em" => format!("_{text}_"),
            "strike" => format!("~~{text}~~"),
            "link" => {
                let href = attr_str(mark, "href");
                if href.is_empty() {
                    text
                } else {
                    format!("[{text}]({href})")
                }
            }
            _ => text,
        };
    }
    text
}

/// Posts plain text as ADF: blank lines split paragraphs, single newlines become hard breaks.
fn text_to_adf(text: &str) -> Value {
    let paragraphs: Vec<Value> = text
        .replace("\r\n", "\n")
        .split("\n\n")
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
        .map(|paragraph| {
            let mut content = Vec::new();
            for (index, line) in paragraph.lines().enumerate() {
                if index > 0 {
                    content.push(json!({ "type": "hardBreak" }));
                }
                if !line.is_empty() {
                    content.push(json!({ "type": "text", "text": line }));
                }
            }
            json!({ "type": "paragraph", "content": content })
        })
        .collect();
    json!({ "type": "doc", "version": 1, "content": paragraphs })
}

// ---- Jira wiki markup (Data Center) ----

fn wiki_to_markdown(input: &str) -> String {
    let text = input.replace("\r\n", "\n").replace('\r', "\n");
    let mut blocks = Vec::new();
    let mut cursor = 0;
    while cursor < text.len() {
        if let Some(at) = next_wiki_macro(&text, cursor) {
            push_wiki_plain(&mut blocks, &text[cursor..at]);
            if let Some((rendered, next)) = take_wiki_macro(&text, at) {
                if !rendered.trim().is_empty() {
                    blocks.push(rendered);
                }
                cursor = next;
                continue;
            }
            push_wiki_plain(&mut blocks, &text[at..at + 1]);
            cursor = at + 1;
            continue;
        }
        push_wiki_plain(&mut blocks, &text[cursor..]);
        break;
    }
    blocks.join("\n\n").trim().to_string()
}

fn push_wiki_plain(blocks: &mut Vec<String>, text: &str) {
    let rendered = wiki_lines_to_markdown(text);
    if !rendered.trim().is_empty() {
        blocks.push(rendered);
    }
}

fn next_wiki_macro(text: &str, from: usize) -> Option<usize> {
    let lower = text[from..].to_ascii_lowercase();
    let mut best = None;
    for name in ["code", "noformat", "quote", "panel"] {
        let needle = format!("{{{name}");
        let mut search = 0;
        while let Some(rel) = lower[search..].find(&needle) {
            let index = from + search + rel;
            if wiki_macro_opener(&text[index..], name).is_some() {
                best = Some(best.map_or(index, |current: usize| current.min(index)));
                break;
            }
            search += rel + 1;
        }
    }
    best
}

/// Byte index of the opener's closing `}`, when `text` starts with `{name`.
fn wiki_macro_opener(text: &str, name: &str) -> Option<usize> {
    let lower = text.to_ascii_lowercase();
    let open = format!("{{{name}");
    if !lower.starts_with(&open) {
        return None;
    }
    let after = open.len();
    if !matches!(lower.as_bytes().get(after), Some(b'}' | b':' | b'|') | None) {
        return None;
    }
    let end = text.find('}')?;
    if end < after {
        return None;
    }
    Some(end)
}

fn take_wiki_macro(text: &str, at: usize) -> Option<(String, usize)> {
    let slice = &text[at..];
    let kind = ["code", "noformat", "quote", "panel"]
        .into_iter()
        .find(|name| wiki_macro_opener(slice, name).is_some())?;
    let opener_end = wiki_macro_opener(slice, kind)?;
    let header = &slice[1..opener_end];
    let mut body_start = at + opener_end + 1;
    if text[body_start..].starts_with('\n') {
        body_start += 1;
    }
    let close = format!("{{{kind}}}");
    let rel = text[body_start..].to_ascii_lowercase().find(&close)?;
    let body = &text[body_start..body_start + rel];
    let next = body_start + rel + close.len();
    let rendered = if kind == "code" || kind == "noformat" {
        let lang = wiki_code_lang(header, kind);
        format!("```{lang}\n{}\n```", body.trim_end_matches('\n'))
    } else {
        wiki_lines_to_markdown(body)
            .lines()
            .map(|line| format!("> {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    Some((rendered, next))
}

fn wiki_code_lang(header: &str, kind: &str) -> String {
    if kind != "code" {
        return String::new();
    }
    let header = header.to_ascii_lowercase();
    let rest = header[kind.len()..].trim_start_matches([':', '|']);
    let token = rest.split(['|', ' ']).next().unwrap_or("").trim();
    if token.is_empty()
        || token.contains('=')
        || !token
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '+'))
    {
        String::new()
    } else {
        token.to_string()
    }
}

fn wiki_lines_to_markdown(text: &str) -> String {
    let mut lines = text.split('\n').peekable();
    let mut blocks = Vec::new();
    let mut paragraph = Vec::new();
    let flush = |paragraph: &mut Vec<String>, blocks: &mut Vec<String>| {
        if paragraph.is_empty() {
            return;
        }
        let rendered = paragraph
            .iter()
            .map(|line| wiki_inline(line))
            .collect::<Vec<_>>()
            .join("\n");
        if !rendered.trim().is_empty() {
            blocks.push(rendered);
        }
        paragraph.clear();
    };
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            flush(&mut paragraph, &mut blocks);
            continue;
        }
        if let Some(heading) = wiki_heading(trimmed) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(heading);
            continue;
        }
        if trimmed.chars().all(|ch| ch == '-') && trimmed.len() >= 4 {
            flush(&mut paragraph, &mut blocks);
            blocks.push("---".into());
            continue;
        }
        if wiki_table_line(trimmed) {
            flush(&mut paragraph, &mut blocks);
            let mut table = vec![trimmed.to_string()];
            while lines
                .peek()
                .is_some_and(|next| wiki_table_line(next.trim()))
            {
                table.push(lines.next().unwrap_or_default().trim().to_string());
            }
            blocks.push(wiki_table(&table));
            continue;
        }
        if let Some(item) = wiki_list_item(trimmed) {
            flush(&mut paragraph, &mut blocks);
            let mut items = vec![item];
            while let Some(next) = lines.peek().and_then(|next| wiki_list_item(next.trim())) {
                items.push(next);
                lines.next();
            }
            blocks.push(wiki_list(&items));
            continue;
        }
        if let Some(quote) = trimmed
            .get(..4)
            .filter(|prefix| prefix.eq_ignore_ascii_case("bq. "))
            .map(|_| &trimmed[4..])
        {
            flush(&mut paragraph, &mut blocks);
            blocks.push(format!("> {}", wiki_inline(quote.trim())));
            continue;
        }
        paragraph.push(trimmed.to_string());
    }
    flush(&mut paragraph, &mut blocks);
    blocks.join("\n\n")
}

fn wiki_heading(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    for level in 1..=6 {
        let prefix = format!("h{level}. ");
        if lower.starts_with(&prefix) {
            return Some(format!(
                "{} {}",
                "#".repeat(level),
                wiki_inline(line[prefix.len()..].trim())
            ));
        }
    }
    None
}

fn wiki_table_line(line: &str) -> bool {
    let line = line.trim();
    line.starts_with('|') && line.ends_with('|') && line.len() > 1
}

fn wiki_table(lines: &[String]) -> String {
    let rows: Vec<Vec<String>> = lines
        .iter()
        .map(|line| wiki_cells(line.trim()))
        .filter(|row| !row.is_empty())
        .collect();
    let Some(width) = rows.iter().map(Vec::len).max() else {
        return String::new();
    };
    let line = |cells: &[String]| {
        let mut padded = cells.to_vec();
        padded.resize(width, String::new());
        format!("| {} |", padded.join(" | "))
    };
    let mut lines = vec![line(&rows[0]), format!("|{}", " --- |".repeat(width))];
    lines.extend(rows[1..].iter().map(|row| line(row)));
    lines.join("\n")
}

fn wiki_cells(line: &str) -> Vec<String> {
    let header = line.starts_with("||");
    let separator = if header { "||" } else { "|" };
    line.trim_matches('|')
        .split(separator)
        .map(|cell| wiki_inline(cell.trim()).replace('|', "\\|"))
        .collect()
}

struct WikiListItem {
    depth: usize,
    ordered: bool,
    text: String,
}

fn wiki_list_item(line: &str) -> Option<WikiListItem> {
    let bytes = line.as_bytes();
    let mut depth = 0;
    while depth < bytes.len() && matches!(bytes[depth], b'*' | b'#') {
        depth += 1;
    }
    if depth == 0 || bytes.get(depth) != Some(&b' ') {
        return None;
    }
    let marker = &line[..depth];
    Some(WikiListItem {
        ordered: marker.contains('#'),
        text: line[depth + 1..].trim().to_string(),
        depth,
    })
}

fn wiki_list(items: &[WikiListItem]) -> String {
    items
        .iter()
        .map(|item| {
            let indent = "  ".repeat(item.depth.saturating_sub(1));
            let marker = if item.ordered { "1. " } else { "- " };
            format!("{indent}{marker}{}", wiki_inline(&item.text))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn wiki_inline(input: &str) -> String {
    let mut out = String::new();
    let mut index = 0;
    while index < input.len() {
        let rest = &input[index..];
        let prev = input[..index].chars().next_back();
        if let Some(rendered) = wiki_break(rest) {
            out.push('\n');
            index += rendered;
            continue;
        }
        if let Some(ch) = wiki_escape(rest) {
            out.push(ch);
            index += 1 + ch.len_utf8();
            continue;
        }
        if let Some(skip) = wiki_color_tag(rest) {
            index += skip;
            continue;
        }
        if let Some((content, len)) = wiki_delimited(rest, "{{", "}}") {
            out.push('`');
            out.push_str(content);
            out.push('`');
            index += len;
            continue;
        }
        if let Some((rendered, len)) = wiki_image(rest) {
            out.push_str(&rendered);
            index += len;
            continue;
        }
        if let Some((rendered, len)) = wiki_link(rest) {
            out.push_str(&rendered);
            index += len;
            continue;
        }
        if let Some((content, len)) = wiki_emphasis(rest, '*', prev) {
            out.push_str("**");
            out.push_str(&wiki_inline(content));
            out.push_str("**");
            index += len;
            continue;
        }
        if let Some((content, len)) = wiki_emphasis(rest, '_', prev) {
            out.push('_');
            out.push_str(&wiki_inline(content));
            out.push('_');
            index += len;
            continue;
        }
        let ch = rest.chars().next().unwrap_or_default();
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

fn wiki_break(input: &str) -> Option<usize> {
    input.starts_with("\\\\").then_some(2)
}

fn wiki_escape(input: &str) -> Option<char> {
    let mut chars = input.chars();
    if chars.next() != Some('\\') {
        return None;
    }
    chars.next()
}

fn wiki_color_tag(input: &str) -> Option<usize> {
    let lower = input.to_ascii_lowercase();
    if !lower.starts_with("{color") {
        return None;
    }
    let after = "{color".len();
    if !matches!(lower.as_bytes().get(after), Some(b'}' | b':' | b'|')) {
        return None;
    }
    Some(input.find('}')? + 1)
}

fn wiki_delimited<'a>(input: &'a str, open: &str, close: &str) -> Option<(&'a str, usize)> {
    if !input.starts_with(open) {
        return None;
    }
    let rest = &input[open.len()..];
    let end = rest.find(close)?;
    let content = &rest[..end];
    if content.is_empty() || content.contains('\n') {
        return None;
    }
    Some((content, open.len() + end + close.len()))
}

fn wiki_image(input: &str) -> Option<(String, usize)> {
    let (content, len) = wiki_delimited(input, "!", "!")?;
    if content.contains(' ') {
        return None;
    }
    let source = content.split('|').next().unwrap_or(content).trim();
    if source.is_empty() {
        return None;
    }
    let rendered = if looks_like_url(source) {
        format!("![]({source})")
    } else {
        source.to_string()
    };
    Some((rendered, len))
}

fn wiki_link(input: &str) -> Option<(String, usize)> {
    let (content, len) = wiki_delimited(input, "[", "]")?;
    if content.contains('[') {
        return None;
    }
    if let Some(user) = content.strip_prefix('~') {
        let valid = !user.is_empty()
            && !user.contains('|')
            && user
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'));
        if valid {
            return Some((format!("@{user}"), len));
        }
    }
    if let Some((text, url)) = content.split_once('|') {
        let text = text.trim();
        let url = url.trim();
        if text.is_empty() || url.is_empty() {
            return None;
        }
        return Some((format!("[{text}]({url})"), len));
    }
    if looks_like_url(content) {
        return Some((format!("<{content}>"), len));
    }
    None
}

fn looks_like_url(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.starts_with("https://")
        || value.starts_with("http://")
        || value.starts_with("mailto:")
        || value.starts_with("www.")
}

fn wiki_emphasis(input: &str, marker: char, prev: Option<char>) -> Option<(&str, usize)> {
    if !wiki_boundary(prev) || !input.starts_with(marker) {
        return None;
    }
    let rest = &input[marker.len_utf8()..];
    if rest.starts_with(marker) {
        return None;
    }
    let end = rest.find(marker)?;
    let content = &rest[..end];
    if content.is_empty()
        || content.contains('\n')
        || content.contains(marker)
        || content.starts_with(' ')
        || content.ends_with(' ')
    {
        return None;
    }
    let after = &rest[end + marker.len_utf8()..];
    if after.starts_with(marker) || !wiki_boundary(after.chars().next()) {
        return None;
    }
    Some((content, marker.len_utf8() + end + marker.len_utf8()))
}

fn wiki_boundary(ch: Option<char>) -> bool {
    ch.is_none_or(|ch| !ch.is_ascii_alphanumeric())
}

// ---- Config storage ----

fn normalize_jira_site(raw: &str, deployment: JiraDeployment) -> Result<String, String> {
    let raw = raw.trim().trim_end_matches('/');
    if raw.is_empty() {
        return Err(match deployment {
            JiraDeployment::Cloud => "Enter your Jira site (e.g. yourteam.atlassian.net)".into(),
            JiraDeployment::DataCenter => {
                "Enter your Jira Data Center URL (e.g. jira.example.com)".into()
            }
        });
    }
    // Bare `yourteam` is an Atlassian Cloud site. Data Center keeps the host,
    // including a context path such as `/jira`.
    let with_scheme = if raw.contains("://") {
        raw.to_string()
    } else if deployment == JiraDeployment::Cloud
        && !raw.contains('.')
        && !raw.contains('/')
        && !raw.contains(':')
    {
        format!("https://{raw}.atlassian.net")
    } else {
        format!("https://{raw}")
    };
    let url = url::Url::parse(&with_scheme).map_err(|_| "Jira site is invalid".to_string())?;
    // Credentials travel on every request. Never accept them in the URL.
    if url.scheme() != "https" {
        return Err("Jira site must use HTTPS".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Jira site is invalid".into());
    }
    let host = jira_origin_host(&url);
    if host.is_empty() {
        return Err("Jira site is invalid".into());
    }
    let port = url
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    let path = match deployment {
        JiraDeployment::Cloud => String::new(),
        JiraDeployment::DataCenter => datacenter_context_path(url.path()).to_string(),
    };
    Ok(format!("https://{host}{port}{path}"))
}

fn jira_origin_host(url: &url::Url) -> String {
    match url.host() {
        Some(url::Host::Domain(domain)) => domain.to_ascii_lowercase(),
        Some(url::Host::Ipv4(ip)) => ip.to_string(),
        Some(url::Host::Ipv6(ip)) => format!("[{ip}]"),
        None => String::new(),
    }
}

/// Keep `/jira` and drop a pasted page such as `/jira/browse/ENG-1`.
fn datacenter_context_path(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "";
    }
    let lower = trimmed.to_ascii_lowercase();
    let mut cut = None;
    for marker in [
        "/rest",
        "/browse",
        "/secure",
        "/projects",
        "/issues",
        "/plugins",
        "/login.jsp",
    ] {
        let mut start = 0;
        while let Some(rel) = lower[start..].find(marker) {
            let index = start + rel;
            let after = index + marker.len();
            let boundary = after == lower.len() || lower.as_bytes()[after] == b'/';
            if boundary {
                cut = Some(cut.map_or(index, |current: usize| current.min(index)));
                break;
            }
            start = index + 1;
        }
    }
    match cut {
        Some(0) => "",
        Some(index) => trimmed[..index].trim_end_matches('/'),
        None => trimmed,
    }
}

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join("jira-config.json"))
}

fn read_config(app: &AppHandle) -> Result<Option<JiraConfig>, String> {
    let path = config_path(app)?;
    match fs::read_to_string(path) {
        Ok(raw) => {
            let mut config: JiraConfig =
                serde_json::from_str(&raw).map_err(|_| "Jira settings are invalid".to_string())?;
            config.site = normalize_jira_site(&config.site, config.deployment)?;
            config.email = config.email.trim().to_string();
            config.token = config.token.trim().to_string();
            let missing_cloud_account =
                config.deployment == JiraDeployment::Cloud && config.email.is_empty();
            if config.token.is_empty() || missing_cloud_account {
                Ok(None)
            } else {
                Ok(Some(config))
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn require_config(app: &AppHandle) -> Result<JiraConfig, String> {
    read_config(app)?.ok_or_else(|| "Connect Jira in Settings".to_string())
}

fn write_config(app: &AppHandle, config: &JiraConfig) -> Result<(), String> {
    let path = config_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let value = serde_json::to_string(config).map_err(|error| error.to_string())?;
    write_secret_file(&path, &value)
}

fn delete_config(app: &AppHandle) -> Result<(), String> {
    let path = config_path(app)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn write_secret_file(path: &Path, value: &str) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|error| error.to_string())?;
        file.write_all(value.as_bytes())
            .map_err(|error| error.to_string())?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::write(path, value).map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SITE: &str = "https://acme.atlassian.net";

    #[test]
    fn fetches_every_project_page() {
        let mut offsets = Vec::new();
        let projects = fetch_jira_projects(|start| {
            offsets.push(start);
            Ok(if start == 0 {
                json!({ "startAt": 0, "isLast": false, "values": [
                    { "id": "10000", "key": "ENG", "name": "Engineering" }
                ] })
            } else {
                json!({ "startAt": 1, "isLast": true, "values": [
                    { "id": "10001", "key": "OPS", "name": "Operations" }
                ] })
            })
        })
        .unwrap();
        assert_eq!(offsets, vec![0, 1]);
        assert_eq!(projects.len(), 2);
        assert_eq!(projects[1].key, "OPS");
    }

    #[test]
    fn rejects_stalled_project_pagination() {
        assert!(fetch_jira_projects(|_| Ok(json!({
            "isLast": false, "values": []
        })))
        .is_err());
    }

    #[test]
    fn normalizes_jira_sites() {
        assert_eq!(
            normalize_jira_site("acme", JiraDeployment::Cloud).unwrap(),
            SITE
        );
        assert_eq!(
            normalize_jira_site("acme.atlassian.net", JiraDeployment::Cloud).unwrap(),
            SITE
        );
        assert_eq!(
            normalize_jira_site(
                " https://Acme.atlassian.net/jira/software/projects ",
                JiraDeployment::Cloud
            )
            .unwrap(),
            SITE
        );
        assert_eq!(
            normalize_jira_site("jira.example.com:8443", JiraDeployment::Cloud).unwrap(),
            "https://jira.example.com:8443"
        );
        assert!(normalize_jira_site("http://acme.atlassian.net", JiraDeployment::Cloud).is_err());
        assert!(
            normalize_jira_site("https://user@acme.atlassian.net", JiraDeployment::Cloud).is_err()
        );
        assert!(normalize_jira_site("", JiraDeployment::Cloud).is_err());
    }

    #[test]
    fn datacenter_site_keeps_context_path() {
        assert_eq!(
            normalize_jira_site(
                " https://Jira.Example.com/Jira/browse/ENG-1 ",
                JiraDeployment::DataCenter
            )
            .unwrap(),
            "https://jira.example.com/Jira"
        );
        assert_eq!(
            normalize_jira_site("jira.example.com:8443/jira", JiraDeployment::DataCenter).unwrap(),
            "https://jira.example.com:8443/jira"
        );
        assert_eq!(
            normalize_jira_site("acme", JiraDeployment::DataCenter).unwrap(),
            "https://acme"
        );
        assert_eq!(
            normalize_jira_site(
                "https://jira.example.com/jira/browser",
                JiraDeployment::DataCenter
            )
            .unwrap(),
            "https://jira.example.com/jira/browser"
        );
        assert!(
            normalize_jira_site("http://jira.example.com", JiraDeployment::DataCenter).is_err()
        );
        assert!(normalize_jira_site(
            "https://user@jira.example.com/jira",
            JiraDeployment::DataCenter
        )
        .is_err());
    }

    #[test]
    fn old_config_defaults_to_cloud() {
        let config: JiraConfig = serde_json::from_str(
            r#"{"site":"https://acme.atlassian.net","email":"ada@acme.com","token":"secret"}"#,
        )
        .unwrap();
        assert_eq!(config.deployment, JiraDeployment::Cloud);
    }

    #[test]
    fn authorization_is_basic_email_and_token() {
        let config = JiraConfig {
            site: SITE.into(),
            email: "ada@acme.com".into(),
            token: "secret".into(),
            deployment: JiraDeployment::Cloud,
        };
        assert_eq!(
            jira_authorization(&config),
            "Basic YWRhQGFjbWUuY29tOnNlY3JldA=="
        );
    }

    #[test]
    fn datacenter_authorization_is_bearer() {
        let config = JiraConfig {
            site: "https://jira.example.com/jira".into(),
            email: String::new(),
            token: "pat-secret".into(),
            deployment: JiraDeployment::DataCenter,
        };
        assert_eq!(jira_authorization(&config), "Bearer pat-secret");
        assert_eq!(
            issue_search_request(JiraDeployment::DataCenter, "project = ENG", 40).0,
            "/rest/api/2/search"
        );
        assert_eq!(
            issue_search_request(JiraDeployment::Cloud, "project = ENG", 40).0,
            "/rest/api/3/search/jql"
        );
        assert_eq!(
            comment_request_body(JiraDeployment::DataCenter, "Fixed"),
            json!({ "body": "Fixed" })
        );
    }

    #[test]
    fn datacenter_account_prefers_username() {
        assert_eq!(
            datacenter_account(
                &json!({ "name": "ada", "displayName": "Ada Lovelace", "key": "JIRAUSER1" })
            )
            .unwrap(),
            "ada"
        );
        assert_eq!(
            datacenter_account(&json!({ "displayName": "Ada Lovelace" })).unwrap(),
            "Ada Lovelace"
        );
        assert!(datacenter_account(&json!({})).is_err());
    }

    #[test]
    fn issue_jql_combines_filters() {
        assert_eq!(
            issue_jql(true, "open", &[" 10000 ".into(), "".into(), "10001".into(), "x) OR (1".into()]),
            "assignee = currentUser() AND project in (10000, 10001) AND statusCategory != Done ORDER BY updated DESC"
        );
        assert_eq!(
            issue_jql(false, "all", &[]),
            "updated >= -365d ORDER BY updated DESC"
        );
    }

    #[test]
    fn parse_jira_projects_reads_values() {
        let data = json!({
            "values": [
                { "id": "10000", "key": "ENG", "name": "Engineering" },
                { "id": "", "key": "SKIP", "name": "Skip" }
            ]
        });
        assert_eq!(
            parse_jira_projects(&data).unwrap(),
            vec![JiraProject {
                id: "10000".into(),
                key: "ENG".into(),
                name: "Engineering".into(),
            }]
        );
    }

    #[test]
    fn parse_jira_issues_maps_fields() {
        let data = json!({
            "issues": [{
                "id": "10042",
                "key": "ENG-42",
                "fields": {
                    "summary": "Fix auth",
                    "updated": "2026-08-27T10:00:00.000+0000",
                    "status": { "name": "In Progress", "statusCategory": { "key": "indeterminate" } },
                    "labels": ["bug", "backend"],
                    "assignee": {
                        "displayName": "Maya",
                        "avatarUrls": { "48x48": "https://avatar.example/maya.png" }
                    },
                    "project": { "id": "10000", "key": "ENG", "name": "Engineering" }
                }
            }, { "id": "10043" }]
        });
        let items = parse_jira_issues(&data, SITE).unwrap();
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.provider, "jira");
        assert_eq!(item.kind, "jira");
        assert_eq!(item.id, "10042");
        assert_eq!(item.identifier, "ENG-42");
        assert_eq!(item.number, 42);
        assert_eq!(item.url, "https://acme.atlassian.net/browse/ENG-42");
        assert_eq!(item.state, "In Progress");
        assert_eq!(item.state_type, "indeterminate");
        assert_eq!(item.updated_at, "2026-08-27T10:00:00.000+00:00");
        assert_eq!(item.labels[1].name, "backend");
        assert_eq!(item.assignees[0].login, "Maya");
        assert_eq!(
            item.assignees[0].avatar_url,
            "https://avatar.example/maya.png"
        );
        assert_eq!(item.repo, "ENG");
        assert_eq!(item.team_id, "10000");
        assert_eq!(item.team_name, "Engineering");
    }

    #[test]
    fn parse_jira_issue_details_prefers_reporter() {
        let data = json!({
            "fields": {
                "description": {
                    "type": "doc",
                    "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "Steps" }] }]
                },
                "reporter": { "displayName": "Ada", "avatarUrls": { "48x48": "https://avatar.example/ada.png" } },
                "creator": { "displayName": "Lin" },
                "assignee": null
            }
        });
        let details = parse_jira_issue_details(&data, SITE, JiraDeployment::Cloud).unwrap();
        assert_eq!(details.body, "Steps");
        assert_eq!(details.author, "Ada");
        assert_eq!(details.author_avatar_url, "https://avatar.example/ada.png");
    }

    #[test]
    fn parse_jira_issue_thread_sorts_oldest_first() {
        let data = json!({
            "total": 3,
            "comments": [
                {
                    "id": "2",
                    "author": { "displayName": "Maya" },
                    "body": "Plain reply",
                    "created": "2026-08-31T11:00:00.000+0000"
                },
                {
                    "id": "1",
                    "author": { "displayName": "Ada" },
                    "body": { "type": "doc", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "Root" }] }] },
                    "created": "2026-08-31T10:00:00.000+0000"
                }
            ]
        });
        let thread = parse_jira_issue_thread(&data, SITE, "ENG-42", JiraDeployment::Cloud).unwrap();
        assert!(thread.truncated);
        assert_eq!(thread.comments.len(), 2);
        assert_eq!(thread.comments[0].body, "Root");
        assert_eq!(
            thread.comments[0].url,
            "https://acme.atlassian.net/browse/ENG-42?focusedCommentId=1"
        );
        assert_eq!(thread.comments[1].author, "Maya");
        assert_eq!(thread.comments[1].body, "Plain reply");
    }

    #[test]
    fn datacenter_description_converts_wiki() {
        let data = json!({
            "fields": {
                "description": "h1. Steps\n\n* Check auth\n\n{color:red}blocked{color}",
                "reporter": {
                    "displayName": "Ada",
                    "avatarUrls": { "48x48": "/secure/useravatar?size=large" }
                }
            }
        });
        let details = parse_jira_issue_details(
            &data,
            "https://jira.example.com/jira",
            JiraDeployment::DataCenter,
        )
        .unwrap();
        assert_eq!(details.body, "# Steps\n\n- Check auth\n\nblocked");
        assert_eq!(details.author, "Ada");
        assert_eq!(
            details.author_avatar_url,
            "https://jira.example.com/jira/secure/useravatar?size=large"
        );
    }

    #[test]
    fn wiki_markup_becomes_markdown() {
        let wiki = "\
h2. Repro\n\
\n\
Ping *Maya* and _now_ run {{make}}\n\
\n\
* one\n\
** nested\n\
* two\n\
\n\
{code:sh}\n\
npm test\n\
{code}\n\
\n\
See [docs|https://example.com] and [~ada]\n\
\n\
||A||B||\n\
|1|2|\n\
\n\
{quote}\n\
quoted\n\
{quote}\n";
        assert_eq!(
            wiki_to_markdown(wiki),
            "## Repro\n\nPing **Maya** and _now_ run `make`\n\n- one\n  - nested\n- two\n\n```sh\nnpm test\n```\n\nSee [docs](https://example.com) and @ada\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n\n> quoted"
        );
        assert_eq!(wiki_to_markdown("Plain reply"), "Plain reply");
    }

    #[test]
    fn adf_converts_to_markdown() {
        let doc = json!({
            "type": "doc",
            "version": 1,
            "content": [
                { "type": "heading", "attrs": { "level": 2 }, "content": [{ "type": "text", "text": "Repro" }] },
                { "type": "paragraph", "content": [
                    { "type": "text", "text": "Ping " },
                    { "type": "mention", "attrs": { "text": "@Maya" } },
                    { "type": "text", "text": " see " },
                    { "type": "text", "text": "docs", "marks": [{ "type": "link", "attrs": { "href": "https://example.com" } }] },
                    { "type": "hardBreak" },
                    { "type": "text", "text": "now", "marks": [{ "type": "strong" }] },
                    { "type": "text", "text": " run " },
                    { "type": "text", "text": "make", "marks": [{ "type": "code" }, { "type": "strong" }] }
                ]},
                { "type": "bulletList", "content": [
                    { "type": "listItem", "content": [
                        { "type": "paragraph", "content": [{ "type": "text", "text": "one" }] },
                        { "type": "orderedList", "content": [
                            { "type": "listItem", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "nested" }] }] }
                        ]}
                    ]},
                    { "type": "listItem", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "two" }] }] }
                ]},
                { "type": "codeBlock", "attrs": { "language": "sh" }, "content": [{ "type": "text", "text": "npm test" }] },
                { "type": "blockquote", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "quoted" }] }] },
                { "type": "mediaSingle", "content": [{ "type": "media", "attrs": { "id": "x" } }] }
            ]
        });
        assert_eq!(
            adf_to_markdown(&doc),
            "## Repro\n\nPing @Maya see [docs](https://example.com)\n**now** run `make`\n\n- one\n  1. nested\n- two\n\n```sh\nnpm test\n```\n\n> quoted"
        );
    }

    #[test]
    fn adf_renders_tables() {
        let cell = |text: &str| json!({ "type": "tableCell", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": text }] }] });
        let doc = json!({
            "type": "doc",
            "content": [{ "type": "table", "content": [
                { "type": "tableRow", "content": [cell("a"), cell("b")] },
                { "type": "tableRow", "content": [cell("1"), cell("2|3")] }
            ]}]
        });
        assert_eq!(
            adf_to_markdown(&doc),
            "| a | b |\n| --- | --- |\n| 1 | 2\\|3 |"
        );
    }

    #[test]
    fn text_to_adf_splits_paragraphs_and_lines() {
        assert_eq!(
            text_to_adf("First line\nsecond\n\nNext"),
            json!({
                "type": "doc",
                "version": 1,
                "content": [
                    { "type": "paragraph", "content": [
                        { "type": "text", "text": "First line" },
                        { "type": "hardBreak" },
                        { "type": "text", "text": "second" }
                    ]},
                    { "type": "paragraph", "content": [{ "type": "text", "text": "Next" }] }
                ]
            })
        );
    }

    #[test]
    fn normalizes_jira_offsets() {
        assert_eq!(
            normalize_jira_time("2026-08-27T10:00:00.000+0200"),
            "2026-08-27T10:00:00.000+02:00"
        );
        assert_eq!(
            normalize_jira_time("2026-08-27T10:00:00.000Z"),
            "2026-08-27T10:00:00.000Z"
        );
    }

    #[test]
    fn issue_keys_are_validated() {
        assert_eq!(require_issue_key(" ENG-42 ").unwrap(), "ENG-42");
        assert!(require_issue_key("").is_err());
        assert!(require_issue_key("ENG-42/../x").is_err());
    }

    #[test]
    fn jira_error_message_reads_both_shapes() {
        assert_eq!(
            jira_error_message(r#"{"errorMessages":["Issue does not exist"],"errors":{}}"#)
                .as_deref(),
            Some("Issue does not exist")
        );
        assert_eq!(
            jira_error_message(r#"{"errorMessages":[],"errors":{"jql":"Bad JQL"}}"#).as_deref(),
            Some("Bad JQL")
        );
    }
}
