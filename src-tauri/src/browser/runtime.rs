use super::BrowserProfile;
use base64::Engine;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tungstenite::{Message, WebSocket};

const IO_TIMEOUT: Duration = Duration::from_secs(15);

struct Tab {
    owner: String,
    session: String,
}

#[derive(Default)]
pub struct BrowserRuntime {
    child: Option<Child>,
    socket: Option<WebSocket<TcpStream>>,
    tabs: HashMap<String, Tab>,
    sequence: u64,
    profile_id: Option<String>,
}

impl BrowserRuntime {
    pub fn running(&mut self) -> bool {
        let alive = self
            .child
            .as_mut()
            .map(|child| matches!(child.try_wait(), Ok(None)))
            .unwrap_or(false);
        if !alive {
            self.shutdown();
        }
        alive
    }

    pub fn shutdown(&mut self) {
        // CDP Browser.close can hang during unload; killing our own child is bounded
        // and Chromium's persistent cookie store remains in the isolated profile.
        if self.child.is_some() && self.socket.is_some() {
            let _ = self.command(None, "Browser.close", json!({}));
        }
        self.socket = None;
        if let Some(mut child) = self.child.take() {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(50))
                    }
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                }
            }
        }
        self.tabs.clear();
        self.profile_id = None;
    }

    fn launch(&mut self, profile: &BrowserProfile) -> Result<(), String> {
        if self.running() {
            return if self.profile_id.as_deref() == Some(&profile.id) {
                Ok(())
            } else {
                Err("Stop the browser before changing profiles".into())
            };
        }
        let root = Path::new(&profile.data_dir);
        fs::create_dir_all(root).map_err(|e| e.to_string())?;
        let port_file = root.join("DevToolsActivePort");
        match fs::remove_file(&port_file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("Cannot clear stale browser endpoint: {e}")),
        }
        let child = Command::new(&profile.executable)
            .arg(format!("--user-data-dir={}", profile.data_dir))
            .arg(format!("--profile-directory={}", profile.directory))
            .args([
                "--remote-debugging-port=0",
                "--remote-debugging-address=127.0.0.1",
                "--no-first-run",
                "--no-default-browser-check",
                "about:blank",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Cannot launch browser: {e}"))?;
        self.child = Some(child);
        self.profile_id = Some(profile.id.clone());
        let result = (|| {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                if !matches!(self.child.as_mut().unwrap().try_wait(), Ok(None)) {
                    return Err("Browser exited before its debugging endpoint was ready; close other browsers using this profile".into());
                }
                if let Ok(contents) = fs::read_to_string(&port_file) {
                    let mut lines = contents.lines();
                    if let (Some(port), Some(path)) = (
                        lines.next().and_then(|s| s.parse::<u16>().ok()),
                        lines.next(),
                    ) {
                        if port != 0
                            && path.starts_with("/devtools/browser/")
                            && !path.contains(['\r', '\n', '#', '?'])
                        {
                            let stream = TcpStream::connect_timeout(
                                &SocketAddr::from(([127, 0, 0, 1], port)),
                                Duration::from_secs(2),
                            )
                            .map_err(|_| "Cannot connect to launched browser".to_string())?;
                            stream
                                .set_read_timeout(Some(IO_TIMEOUT))
                                .map_err(|e| e.to_string())?;
                            stream
                                .set_write_timeout(Some(IO_TIMEOUT))
                                .map_err(|e| e.to_string())?;
                            let (socket, _) =
                                tungstenite::client(format!("ws://127.0.0.1:{port}{path}"), stream)
                                    .map_err(|_| {
                                        "Cannot establish browser control connection".to_string()
                                    })?;
                            self.socket = Some(socket);
                            return Ok(());
                        }
                    }
                }
                if Instant::now() >= deadline {
                    return Err("Timed out waiting for browser startup".into());
                }
                std::thread::sleep(Duration::from_millis(75));
            }
        })();
        if result.is_err() {
            self.shutdown();
        }
        result
    }

    fn command(
        &mut self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        self.sequence += 1;
        let id = self.sequence;
        let mut request = json!({"id":id,"method":method,"params":params});
        if let Some(session) = session {
            request["sessionId"] = json!(session);
        }
        let socket = self.socket.as_mut().ok_or("Browser is not running")?;
        socket
            .send(Message::Text(request.to_string().into()))
            .map_err(|_| "Browser control connection closed".to_string())?;
        let deadline = Instant::now() + IO_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(format!("Browser command timed out: {method}"));
            }
            socket
                .get_mut()
                .set_read_timeout(Some(remaining))
                .map_err(|e| e.to_string())?;
            let message = socket
                .read()
                .map_err(|_| format!("Browser connection failed or timed out: {method}"))?;
            match message {
                Message::Text(text) => {
                    let response: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
                    if response.get("id").and_then(Value::as_u64) != Some(id) {
                        continue;
                    }
                    if let Some(error) = response.get("error") {
                        return Err(format!("Browser command {method} failed: {error}"));
                    }
                    return Ok(response.get("result").cloned().unwrap_or_else(|| json!({})));
                }
                Message::Close(_) => return Err("Browser control connection closed".into()),
                _ => {}
            }
        }
    }

    fn evaluate(&mut self, session: &str, expression: &str) -> Result<Value, String> {
        let response = self.command(Some(session), "Runtime.evaluate", json!({"expression":expression,"returnByValue":true,"awaitPromise":true,"timeout":10000}))?;
        if let Some(exception) = response.get("exceptionDetails") {
            return Err(format!("JavaScript exception: {exception}"));
        }
        let result = &response["result"];
        Ok(result.get("value").cloned().unwrap_or_else(|| json!({"type":result["type"],"description":result["description"],"unserializableValue":result["unserializableValue"]})))
    }

    fn info(&mut self, id: &str, session: &str) -> Result<Value, String> {
        let mut value = self.evaluate(session, "({title:document.title,url:location.href})")?;
        value["tabId"] = json!(id);
        Ok(value)
    }

    fn wait_document(&mut self, session: &str) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if self.evaluate(session, "document.readyState")?.as_str() != Some("loading") {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("Navigation timed out waiting for document".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn call(
        &mut self,
        profile: &BrowserProfile,
        owner: &str,
        action: &str,
        input: &Value,
        screenshots: &Path,
    ) -> Result<Value, String> {
        let fields: &[&str] = match action {
            "browser.status" | "browser.tabs" => &[],
            "browser.open" => &["url"],
            "browser.navigate" => &["tabId", "url"],
            "browser.snapshot" | "browser.screenshot" | "browser.close" => &["tabId"],
            "browser.click" => &["tabId", "selector"],
            "browser.fill" => &["tabId", "selector", "value"],
            "browser.press" => &["tabId", "key"],
            "browser.evaluate" => &["tabId", "expression"],
            _ => return Err("Unknown browser action".into()),
        };
        let object = input
            .as_object()
            .ok_or("Browser arguments must be an object")?;
        if let Some(key) = object.keys().find(|key| !fields.contains(&key.as_str())) {
            return Err(format!("Unknown browser argument: {key}"));
        }
        if action == "browser.status" {
            return Ok(json!({"running":self.running()}));
        }
        // Validate before launching, including URL scheme and required argument types.
        for field in fields {
            if action == "browser.open" && !object.contains_key(*field) {
                continue;
            }
            string(input, field)?;
        }
        if matches!(action, "browser.open" | "browser.navigate") {
            validate_url(
                input
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or("about:blank"),
            )?;
        }
        if action == "browser.open" {
            self.launch(profile)?;
            let url = input
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or("about:blank");
            let target = self.command(None, "Target.createTarget", json!({"url":"about:blank"}))?;
            let id = target["targetId"]
                .as_str()
                .ok_or("Browser did not return a target")?
                .to_string();
            let attachment = self.command(
                None,
                "Target.attachToTarget",
                json!({"targetId":id,"flatten":true}),
            )?;
            let session = attachment["sessionId"]
                .as_str()
                .ok_or("Browser did not attach a session")?
                .to_string();
            self.tabs.insert(
                id.clone(),
                Tab {
                    owner: owner.into(),
                    session: session.clone(),
                },
            );
            self.command(Some(&session), "Page.enable", json!({}))?;
            if url != "about:blank" {
                self.navigate(&session, url)?;
            }
            return self.info(&id, &session);
        }
        if !self.running() {
            return if action == "browser.tabs" {
                Ok(json!({"tabs":[]}))
            } else {
                Err("Browser is not running; use browser.open first".into())
            };
        }
        if action == "browser.tabs" {
            let targets = self.command(None, "Target.getTargets", json!({}))?;
            let live: std::collections::HashSet<String> = targets["targetInfos"]
                .as_array()
                .ok_or("Invalid browser targets response")?
                .iter()
                .filter_map(|t| t["targetId"].as_str().map(str::to_owned))
                .collect();
            self.tabs.retain(|id, _| live.contains(id));
            let tabs: Vec<Value> = targets["targetInfos"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|t| {
                    t["targetId"]
                        .as_str()
                        .and_then(|id| self.tabs.get(id))
                        .map(|t| t.owner == owner)
                        .unwrap_or(false)
                })
                .map(|t| json!({"tabId":t["targetId"],"title":t["title"],"url":t["url"]}))
                .collect();
            return Ok(json!({"tabs":tabs}));
        }
        let id = string(input, "tabId")?;
        let session = self
            .tabs
            .get(id)
            .filter(|tab| tab.owner == owner)
            .ok_or("Tab is unavailable to this owner")?
            .session
            .clone();
        match action {
            "browser.navigate" => {
                self.navigate(&session, string(input, "url")?)?;
                self.info(id, &session)
            }
            "browser.snapshot" => self.evaluate(&session, SNAPSHOT),
            "browser.evaluate" => self.evaluate(&session, string(input, "expression")?),
            "browser.close" => {
                let result = self.command(None, "Target.closeTarget", json!({"targetId":id}))?;
                if result["success"] != true {
                    return Err("Browser refused to close tab".into());
                }
                self.tabs.remove(id);
                Ok(json!({"closed":true,"tabId":id}))
            }
            "browser.screenshot" => {
                let result = self.command(
                    Some(&session),
                    "Page.captureScreenshot",
                    json!({"format":"png","captureBeyondViewport":false}),
                )?;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(
                        result["data"]
                            .as_str()
                            .ok_or("Browser did not return screenshot data")?,
                    )
                    .map_err(|e| e.to_string())?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
                    let mut builder = fs::DirBuilder::new();
                    builder.recursive(true).mode(0o700);
                    builder.create(screenshots).map_err(|e| e.to_string())?;
                    fs::set_permissions(screenshots, fs::Permissions::from_mode(0o700))
                        .map_err(|e| e.to_string())?;
                }
                #[cfg(not(unix))]
                fs::create_dir_all(screenshots).map_err(|e| e.to_string())?;
                let path = screenshots.join(format!("browser-{}.png", uuid::Uuid::new_v4()));
                let mut options = fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = options.open(&path).map_err(|e| e.to_string())?;
                std::io::Write::write_all(&mut file, &bytes).map_err(|e| e.to_string())?;
                Ok(json!({"path":path.to_string_lossy(),"tabId":id}))
            }
            "browser.click" | "browser.fill" => {
                let selector =
                    serde_json::to_string(string(input, "selector")?).map_err(|e| e.to_string())?;
                let point = self.evaluate(&session, &format!("(() => {{ const e=document.querySelector({selector}); if(!e) throw Error('Element not found'); e.scrollIntoView({{block:'center',inline:'center'}}); const r=e.getBoundingClientRect(); if(!r.width || !r.height) throw Error('Element is not visible'); return {{x:r.x+r.width/2,y:r.y+r.height/2}}; }})()"))?;
                let x = point["x"]
                    .as_f64()
                    .ok_or("Element has no clickable position")?;
                let y = point["y"]
                    .as_f64()
                    .ok_or("Element has no clickable position")?;
                self.command(
                    Some(&session),
                    "Input.dispatchMouseEvent",
                    json!({"type":"mousePressed","x":x,"y":y,"button":"left","clickCount":1}),
                )?;
                self.command(
                    Some(&session),
                    "Input.dispatchMouseEvent",
                    json!({"type":"mouseReleased","x":x,"y":y,"button":"left","clickCount":1}),
                )?;
                if action == "browser.fill" {
                    self.evaluate(&session, &format!("(() => {{ const e=document.querySelector({selector}); if(!e || e.disabled || e.readOnly || !(e instanceof HTMLInputElement || e instanceof HTMLTextAreaElement || e.isContentEditable)) throw Error('Element is not editable'); e.focus(); if(typeof e.select==='function') e.select(); else {{const r=document.createRange();r.selectNodeContents(e);const s=getSelection();s.removeAllRanges();s.addRange(r);}} return true; }})()"))?;
                    let value = string(input, "value")?;
                    if value.is_empty() {
                        self.press(&session, "Backspace")?;
                    } else {
                        self.command(Some(&session), "Input.insertText", json!({"text":value}))?;
                    }
                }
                Ok(json!({"tabId":id,"ok":true}))
            }
            "browser.press" => {
                self.press(&session, string(input, "key")?)?;
                Ok(json!({"tabId":id,"ok":true}))
            }
            _ => Err("Unknown browser action".into()),
        }
    }

    fn navigate(&mut self, session: &str, url: &str) -> Result<(), String> {
        let result = self.command(Some(session), "Page.navigate", json!({"url":url}))?;
        if let Some(error) = result.get("errorText").and_then(Value::as_str) {
            return Err(format!("Navigation failed: {error}"));
        }
        self.wait_document(session)
    }

    fn press(&mut self, session: &str, key: &str) -> Result<(), String> {
        let (code, virtual_key, text) = match key {
            "Enter" => ("Enter", 13, Some("\r")),
            "Tab" => ("Tab", 9, None),
            "Escape" => ("Escape", 27, None),
            "Backspace" => ("Backspace", 8, None),
            "Delete" => ("Delete", 46, None),
            "ArrowLeft" => ("ArrowLeft", 37, None),
            "ArrowUp" => ("ArrowUp", 38, None),
            "ArrowRight" => ("ArrowRight", 39, None),
            "ArrowDown" => ("ArrowDown", 40, None),
            "Home" => ("Home", 36, None),
            "End" => ("End", 35, None),
            "PageUp" => ("PageUp", 33, None),
            "PageDown" => ("PageDown", 34, None),
            "Space" | " " => ("Space", 32, Some(" ")),
            _ if key.chars().count() == 1 => { self.command(Some(session), "Input.insertText", json!({"text":key}))?; return Ok(()); },
            _ => return Err("Unsupported key; use a character or Enter, Tab, Escape, Backspace, Delete, ArrowLeft/Up/Right/Down, Home, End, PageUp/Down, Space".into()),
        };
        let key_name = if code == "Space" { " " } else { key };
        let mut params = json!({"type":"keyDown","key":key_name,"code":code,"windowsVirtualKeyCode":virtual_key});
        if let Some(text) = text {
            params["text"] = json!(text);
        }
        self.command(Some(session), "Input.dispatchKeyEvent", params)?;
        self.command(
            Some(session),
            "Input.dispatchKeyEvent",
            json!({"type":"keyUp","key":key_name,"code":code,"windowsVirtualKeyCode":virtual_key}),
        )?;
        Ok(())
    }
}

impl Drop for BrowserRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn string<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{key} must be a string"))
}

fn validate_url(value: &str) -> Result<(), String> {
    if value == "about:blank" {
        return Ok(());
    }
    let url = url::Url::parse(value).map_err(|_| "Invalid browser URL".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("Browser URLs must use http or https (or about:blank)".into());
    }
    Ok(())
}

const SNAPSHOT: &str = r#"(() => {
 const selector = e => {
   const parts=[];
   while(e && e.nodeType===1) {
     if(e.id && document.querySelectorAll('#'+CSS.escape(e.id)).length===1) {parts.unshift('#'+CSS.escape(e.id));break;}
     let n=1;for(let p=e.previousElementSibling;p;p=p.previousElementSibling) if(p.localName===e.localName)n++;
     parts.unshift(e.localName+':nth-of-type('+n+')');e=e.parentElement;
   }
   return parts.join(' > ');
 };
 const all=Array.from(document.querySelectorAll('a[href],button,input,textarea,select,[role="button"],[role="link"],[contenteditable="true"],[tabindex]'));
 const visible=all.filter(e=>{const r=e.getBoundingClientRect();return r.width>0&&r.height>0&&getComputedStyle(e).visibility!=='hidden'});
 const elements=visible.slice(0,250).map(e=>({selector:selector(e),tag:e.localName,role:e.getAttribute('role'),type:e.getAttribute('type'),name:(e.getAttribute('aria-label')||e.getAttribute('placeholder')||e.innerText||e.getAttribute('name')||'').slice(0,300),value:e.type==='password'?'[redacted]':typeof e.value==='string'?e.value.slice(0,500):undefined,disabled:!!e.disabled,checked:typeof e.checked==='boolean'?e.checked:undefined}));
 const text=(document.body?.innerText||'').slice(0,30000);
 return {title:document.title,url:location.href,text,elements,truncated:text.length===30000||visible.length>250};
})()"#;
