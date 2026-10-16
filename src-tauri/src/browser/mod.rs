//! User-owned persistent browser profiles; agent operations require an active
//! authenticated /operator grant in control.rs. Import is never an agent action.
mod profiles;
mod runtime;

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, State};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProfile {
    pub id: String,
    pub name: String,
    pub browser: String,
    pub executable: String,
    pub data_dir: String,
    pub directory: String,
}

#[derive(Default)]
pub struct BrowserHost {
    pub runtime: Mutex<runtime::BrowserRuntime>,
}

fn root(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|path| path.join("browser"))
        .map_err(|error| error.to_string())
}

pub fn call(app: &AppHandle, owner: &str, action: &str, input: &Value) -> Result<Value, String> {
    let host = app.state::<BrowserHost>();
    let mut runtime = host
        .runtime
        .lock()
        .map_err(|_| "Browser service unavailable")?;
    if action == "browser.status" || (action == "browser.tabs" && !runtime.running()) {
        if input.as_object().is_none_or(|fields| !fields.is_empty()) {
            return Err(format!("{action} expects an empty object"));
        }
        return Ok(if action == "browser.status" {
            json!({"running": runtime.running()})
        } else {
            json!({"tabs": []})
        });
    }
    if action != "browser.open" && !runtime.running() {
        return Err("Browser is not running; use browser.open first".into());
    }
    let root = root(app)?;
    let profile = profiles::selected(&root)?;
    runtime.call(&profile, owner, action, input, &root.join("screenshots"))
}

#[tauri::command(async)]
pub fn browser_status(app: AppHandle, host: State<'_, BrowserHost>) -> Result<Value, String> {
    let mut runtime = host
        .runtime
        .lock()
        .map_err(|_| "Browser service unavailable")?;
    let root = root(&app)?;
    let profiles = profiles::profiles(&root)?;
    let selected = match std::fs::read(root.join("selected.json")) {
        Ok(bytes) => {
            let value: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
            let id = value["id"]
                .as_str()
                .ok_or("Invalid selected browser profile")?;
            Some(
                profiles
                    .iter()
                    .find(|profile| profile.id == id)
                    .ok_or("Selected browser profile is missing")?,
            )
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.to_string()),
    };
    Ok(json!({
        "running": runtime.running(),
        "selectedProfile": selected,
        "profiles": profiles,
        "sources": profiles::sources()?,
    }))
}

#[tauri::command(async)]
pub fn browser_launch(app: AppHandle) -> Result<Value, String> {
    call(&app, "user", "browser.open", &json!({}))
}

#[tauri::command(async)]
pub fn browser_shutdown(host: State<'_, BrowserHost>) -> Result<(), String> {
    host.runtime
        .lock()
        .map_err(|_| "Browser service unavailable")?
        .shutdown();
    Ok(())
}

#[tauri::command(async)]
pub fn browser_import(
    app: AppHandle,
    host: State<'_, BrowserHost>,
    source_id: String,
) -> Result<BrowserProfile, String> {
    let mut runtime = host
        .runtime
        .lock()
        .map_err(|_| "Browser service unavailable")?;
    if runtime.running() {
        return Err("Stop the agent browser before importing a profile".into());
    }
    profiles::import(&root(&app)?, &source_id)
}

#[tauri::command(async)]
pub fn browser_select(
    app: AppHandle,
    host: State<'_, BrowserHost>,
    id: String,
) -> Result<(), String> {
    let mut runtime = host
        .runtime
        .lock()
        .map_err(|_| "Browser service unavailable")?;
    if runtime.running() {
        return Err("Stop the agent browser before switching profiles".into());
    }
    profiles::select(&root(&app)?, &id)
}

pub fn shutdown(app: &AppHandle) {
    if let Some(host) = app.try_state::<BrowserHost>() {
        if let Ok(mut runtime) = host.runtime.lock() {
            runtime.shutdown();
        }
    }
}
