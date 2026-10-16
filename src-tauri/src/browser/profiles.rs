use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::BrowserProfile;

// Deliberately exclude passwords, autofill, downloads, history, extensions and
// caches. These stores retain site sessions without exporting saved credentials.
const SESSION_PATHS: &[&str] = &[
    "Cookies",
    "Cookies-journal",
    "Cookies-wal",
    "Cookies-shm",
    "Network/Cookies",
    "Network/Cookies-journal",
    "Network/Cookies-wal",
    "Network/Cookies-shm",
    "Local Storage",
    "IndexedDB",
    "Session Storage",
    "WebStorage",
    "Storage",
    "File System",
    "Service Worker/Database",
    "Preferences",
    "Secure Preferences",
];

struct Installation {
    browser: &'static str,
    executable: PathBuf,
    data_dir: PathBuf,
}

fn error(path: &Path, error: impl std::fmt::Display) -> String {
    format!("{}: {error}", path.display())
}

fn component(value: &str) -> Result<(), String> {
    if value.is_empty() || value.contains(['/', '\\', ':']) || value.contains('\0') {
        return Err("Browser profile directory must be a single normal path component".into());
    }
    let mut parts = Path::new(value).components();
    if !matches!(parts.next(), Some(Component::Normal(_))) || parts.next().is_some() {
        return Err("Browser profile directory must be a single normal path component".into());
    }
    Ok(())
}

fn managed_id(id: &str) -> Result<(), String> {
    let parsed =
        Uuid::parse_str(id).map_err(|_| "Invalid managed browser profile ID".to_string())?;
    if parsed.to_string() != id {
        return Err("Invalid managed browser profile ID".into());
    }
    Ok(())
}

fn is_link(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Junctions and other reparse points must not escape the source tree.
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}

fn checked_metadata(path: &Path) -> Result<fs::Metadata, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| error(path, e))?;
    if is_link(&metadata) {
        return Err(error(path, "symlinks and junctions cannot be imported"));
    }
    Ok(metadata)
}

fn check_ancestors(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err(error(path, "expected an absolute browser profile path"));
    }
    let mut current = PathBuf::new();
    for part in path.components() {
        if matches!(part, Component::ParentDir | Component::CurDir) {
            return Err(error(path, "relative path components are not allowed"));
        }
        current.push(part);
        if !checked_metadata(&current)?.is_dir() {
            return Err(error(&current, "expected a directory"));
        }
    }
    Ok(())
}

fn open_read(path: &Path) -> Result<File, String> {
    if !checked_metadata(path)?.is_file() {
        return Err(error(path, "expected a regular file"));
    }
    #[cfg(unix)]
    let file = {
        use std::ffi::CString;
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::os::unix::ffi::OsStrExt;
        if !path.is_absolute() {
            return Err(error(path, "expected an absolute source path"));
        }
        // Walk directory descriptors so replacing an ancestor with a symlink
        // during import cannot redirect a session-file read outside the source.
        let mut directory = File::open("/").map_err(|e| error(path, e))?;
        let mut parts = path.components().peekable();
        while let Some(part) = parts.next() {
            if matches!(part, Component::RootDir) {
                continue;
            }
            let Component::Normal(name) = part else {
                return Err(error(path, "relative path components are not allowed"));
            };
            let name = CString::new(name.as_bytes()).map_err(|e| error(path, e))?;
            let mut flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;
            if parts.peek().is_some() {
                flags |= libc::O_DIRECTORY;
            }
            let descriptor = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
            if descriptor < 0 {
                return Err(error(path, std::io::Error::last_os_error()));
            }
            directory = unsafe { File::from_raw_fd(descriptor) };
        }
        directory
    };
    #[cfg(not(unix))]
    let file = {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT
        }
        let file = options.open(path).map_err(|e| error(path, e))?;
        if is_link(&file.metadata().map_err(|e| error(path, e))?) {
            return Err(error(path, "symlinks and junctions cannot be imported"));
        }
        file
    };
    if !file.metadata().map_err(|e| error(path, e))?.is_file() {
        return Err(error(path, "expected a regular file"));
    }
    Ok(file)
}

fn read_json(path: &Path) -> Result<Value, String> {
    let file = open_read(path)?;
    serde_json::from_reader(file).map_err(|e| error(path, format!("invalid JSON: {e}")))
}

fn private_dir(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err(error(path, "expected an absolute managed profile path"));
    }
    let mut current = PathBuf::new();
    for part in path.components() {
        if matches!(part, Component::ParentDir | Component::CurDir) {
            return Err(error(path, "relative path components are not allowed"));
        }
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut builder = fs::DirBuilder::new();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                builder.create(&current).map_err(|e| error(&current, e))?;
            }
            Err(e) => return Err(error(&current, e)),
        }
        if !checked_metadata(&current)?.is_dir() {
            return Err(error(&current, "expected a directory"));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|e| error(path, e))?;
    }
    Ok(())
}

fn create_file(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path).map_err(|e| error(path, e))
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let mut file = create_file(path)?;
    serde_json::to_writer_pretty(&mut file, value).map_err(|e| error(path, e))?;
    file.write_all(b"\n").map_err(|e| error(path, e))?;
    file.sync_all().map_err(|e| error(path, e))
}

fn source_id(browser: &str, directory: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(browser.as_bytes());
    digest.update([0]);
    digest.update(directory.as_bytes());
    format!("source-{:x}", digest.finalize())
}

fn executable(path: &Path) -> bool {
    if !path.is_absolute() || !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    true
}

fn home() -> Option<PathBuf> {
    crate::dirs_home()
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

#[cfg(target_os = "macos")]
fn installations() -> Vec<Installation> {
    let Some(home) = home() else {
        return Vec::new();
    };
    let definitions = [
        ("Chrome", "Google Chrome", "Google/Chrome"),
        ("Edge", "Microsoft Edge", "Microsoft Edge"),
        ("Brave", "Brave Browser", "BraveSoftware/Brave-Browser"),
        ("Chromium", "Chromium", "Chromium"),
        ("Helium", "Helium", "net.imput.helium"),
    ];
    definitions
        .into_iter()
        .filter_map(|(browser, app, data)| {
            let relative = format!("{app}.app/Contents/MacOS/{app}");
            let binary = [
                Path::new("/Applications").join(&relative),
                home.join("Applications").join(&relative),
            ]
            .into_iter()
            .find(|p| executable(p))?;
            Some(Installation {
                browser,
                executable: binary,
                data_dir: home.join("Library/Application Support").join(data),
            })
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn installations() -> Vec<Installation> {
    let Some(home) = home() else {
        return Vec::new();
    };
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    let definitions: &[(&str, &[&str], &str)] = &[
        (
            "Chrome",
            &["google-chrome-stable", "google-chrome"],
            "google-chrome",
        ),
        (
            "Edge",
            &["microsoft-edge-stable", "microsoft-edge"],
            "microsoft-edge",
        ),
        (
            "Brave",
            &["brave-browser", "brave"],
            "BraveSoftware/Brave-Browser",
        ),
        ("Chromium", &["chromium", "chromium-browser"], "chromium"),
        ("Helium", &["helium", "helium-browser"], "net.imput.helium"),
    ];
    let paths: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    definitions
        .iter()
        .filter_map(|&(browser, binaries, data)| {
            let binary = binaries
                .iter()
                .flat_map(|name| paths.iter().map(move |path| path.join(name)))
                .find(|p| executable(p))?;
            Some(Installation {
                browser,
                executable: binary,
                data_dir: config.join(data),
            })
        })
        .collect()
}

#[cfg(windows)]
fn installations() -> Vec<Installation> {
    let Some(local) = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    else {
        return Vec::new();
    };
    let mut program_roots = vec![local.clone()];
    for variable in ["PROGRAMFILES", "PROGRAMFILES(X86)"] {
        if let Some(path) = std::env::var_os(variable)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
        {
            program_roots.push(path);
        }
    }
    let definitions = [
        (
            "Chrome",
            "Google/Chrome/Application/chrome.exe",
            "Google/Chrome/User Data",
        ),
        (
            "Edge",
            "Microsoft/Edge/Application/msedge.exe",
            "Microsoft/Edge/User Data",
        ),
        (
            "Brave",
            "BraveSoftware/Brave-Browser/Application/brave.exe",
            "BraveSoftware/Brave-Browser/User Data",
        ),
        (
            "Chromium",
            "Chromium/Application/chrome.exe",
            "Chromium/User Data",
        ),
        (
            "Helium",
            "Helium/Application/helium.exe",
            "Helium/User Data",
        ),
    ];
    definitions
        .into_iter()
        .filter_map(|(browser, binary, data)| {
            let binary = program_roots
                .iter()
                .map(|root| root.join(binary))
                .find(|p| executable(p))?;
            Some(Installation {
                browser,
                executable: binary,
                data_dir: local.join(data),
            })
        })
        .collect()
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn installations() -> Vec<Installation> {
    Vec::new()
}

fn discover(installation: &Installation) -> Result<Vec<BrowserProfile>, String> {
    match fs::symlink_metadata(&installation.data_dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(error(&installation.data_dir, e)),
        Ok(_) => check_ancestors(&installation.data_dir)?,
    }
    let state = installation.data_dir.join("Local State");
    let mut entries = BTreeMap::new();
    match fs::symlink_metadata(&state) {
        Ok(_) => {
            let value = read_json(&state)?;
            if let Some(cache) = value
                .pointer("/profile/info_cache")
                .and_then(Value::as_object)
            {
                for (directory, metadata) in cache {
                    component(directory).map_err(|e| error(&state, e))?;
                    let path = installation.data_dir.join(directory);
                    match fs::symlink_metadata(&path) {
                        Ok(_) if checked_metadata(&path)?.is_dir() => {
                            let name = metadata
                                .get("name")
                                .and_then(Value::as_str)
                                .filter(|s| !s.is_empty())
                                .unwrap_or(directory);
                            entries.insert(directory.clone(), name.to_string());
                        }
                        Ok(_) => return Err(error(&path, "profile is not a directory")),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(error(&path, e)),
                    }
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(error(&state, e)),
    }
    if entries.is_empty() {
        let default = installation.data_dir.join("Default");
        match fs::symlink_metadata(&default) {
            Ok(_) if checked_metadata(&default)?.is_dir() => {
                entries.insert("Default".to_string(), "Default".to_string());
            }
            Ok(_) => return Err(error(&default, "profile is not a directory")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(error(&default, e)),
        }
    }
    Ok(entries
        .into_iter()
        .map(|(directory, name)| BrowserProfile {
            id: source_id(installation.browser, &directory),
            name: format!("{} — {name}", installation.browser),
            browser: installation.browser.to_string(),
            executable: installation.executable.to_string_lossy().into_owned(),
            data_dir: installation.data_dir.to_string_lossy().into_owned(),
            directory,
        })
        .collect())
}

/// Source IDs are resolved against current discovery, never interpreted as paths.
pub fn sources() -> Result<Vec<BrowserProfile>, String> {
    let mut result = Vec::new();
    for installation in installations() {
        result.extend(discover(&installation)?);
    }
    Ok(result)
}

fn load_profile(root: &Path, id: &str) -> Result<BrowserProfile, String> {
    managed_id(id)?;
    let directory = root.join("profiles").join(id);
    check_ancestors(&directory)?;
    let metadata = directory.join("profile.json");
    let profile: BrowserProfile = serde_json::from_value(read_json(&metadata)?)
        .map_err(|e| error(&metadata, format!("invalid profile metadata: {e}")))?;
    component(&profile.directory)?;
    if profile.id != id
        || Path::new(&profile.data_dir) != directory
        || profile.name.is_empty()
        || profile.browser.is_empty()
    {
        return Err(error(
            &metadata,
            "managed profile metadata does not match its owned directory",
        ));
    }
    if !Path::new(&profile.executable).is_absolute() {
        return Err(error(
            &metadata,
            "browser executable is not an absolute path",
        ));
    }
    let target = directory.join(&profile.directory);
    if !checked_metadata(&target)?.is_dir() {
        return Err(error(&target, "managed browser profile is not a directory"));
    }
    Ok(profile)
}

/// Listing is read-only; opening settings must not create or select a profile.
pub fn profiles(root: &Path) -> Result<Vec<BrowserProfile>, String> {
    let directory = root.join("profiles");
    match fs::symlink_metadata(&directory) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(error(&directory, e)),
        Ok(_) => check_ancestors(&directory)?,
    }
    let mut result = Vec::new();
    for entry in fs::read_dir(&directory).map_err(|e| error(&directory, e))? {
        let entry = entry.map_err(|e| error(&directory, e))?;
        let id = entry
            .file_name()
            .into_string()
            .map_err(|_| error(&entry.path(), "invalid profile directory name"))?;
        if id.starts_with('.') {
            continue;
        }
        result.push(load_profile(root, &id)?);
    }
    result.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    Ok(result)
}

fn persist_selection(root: &Path, id: &str) -> Result<(), String> {
    let temporary = root.join(format!(".selected-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        write_json(&temporary, &json!({"id": id}))?;
        fs::rename(&temporary, root.join("selected.json")).map_err(|e| error(&temporary, e))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn select(root: &Path, id: &str) -> Result<(), String> {
    load_profile(root, id)?;
    persist_selection(root, id)
}

pub fn selected(root: &Path) -> Result<BrowserProfile, String> {
    let path = root.join("selected.json");
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            let value = read_json(&path)?;
            let id = value
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| error(&path, "selected browser profile must contain an id"))?;
            return load_profile(root, id)
                .map_err(|e| format!("Selected browser profile is unavailable: {e}"));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(error(&path, e)),
    }
    if let Some(profile) = profiles(root)?.into_iter().next() {
        select(root, &profile.id)?;
        return Ok(profile);
    }
    let installation = installations().into_iter().next().ok_or_else(||
        "No supported browser is installed. Install Chrome, Edge, Brave, Chromium or Helium first.".to_string())?;
    create_managed(
        root,
        BrowserProfile {
            id: String::new(),
            name: format!("{} — StereoCode", installation.browser),
            browser: installation.browser.to_string(),
            executable: installation.executable.to_string_lossy().into_owned(),
            data_dir: String::new(),
            directory: "Default".into(),
        },
        None,
    )
}

#[cfg(unix)]
fn ensure_closed(profile: &BrowserProfile) -> Result<(), String> {
    for marker in [
        "SingletonLock",
        "SingletonCookie",
        "SingletonSocket",
        "lockfile",
    ] {
        let path = Path::new(&profile.data_dir).join(marker);
        match fs::symlink_metadata(&path) {
            Ok(_) => return Err(format!("Close {} completely before importing its profile ({} exists). If it crashed, reopen and quit the browser, then retry.", profile.browser, path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(error(&path, e)),
        }
    }
    Ok(())
}

#[cfg(windows)]
fn ensure_closed(profile: &BrowserProfile) -> Result<(), String> {
    use std::process::Command;
    let name = Path::new(&profile.executable)
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "Invalid browser executable".to_string())?;
    // Chromium's Windows singleton is a named mutex, not a filesystem marker.
    // Conservatively reject every matching process, including another profile.
    let output = Command::new("tasklist.exe")
        .args(["/FI", &format!("IMAGENAME eq {name}"), "/FO", "CSV", "/NH"])
        .output()
        .map_err(|e| format!("Cannot check whether {} is closed: {e}", profile.browser))?;
    if !output.status.success() {
        return Err(format!(
            "Cannot check whether {} is closed; close it and retry",
            profile.browser
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout).to_lowercase();
    if text
        .lines()
        .any(|line| line.starts_with(&format!("\"{}\",", name.to_lowercase())))
    {
        return Err(format!(
            "Close {} completely before importing its profile",
            profile.browser
        ));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn ensure_closed(_: &BrowserProfile) -> Result<(), String> {
    Err("Browser profile import is unsupported on this platform".into())
}

fn skipped(name: &str) -> bool {
    matches!(
        name,
        "Cache"
            | "CacheStorage"
            | "Code Cache"
            | "GPUCache"
            | "DawnCache"
            | "GrShaderCache"
            | "ShaderCache"
            | "ScriptCache"
            | "Extensions"
            | "Extension State"
            | "ext"
            | "Local Extension Settings"
            | "Sync Extension Settings"
            | "History"
            | "History-journal"
            | "Login Data"
            | "Login Data-journal"
            | "Login Data For Account"
            | "Web Data"
            | "Web Data-journal"
    )
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), String> {
    let metadata = checked_metadata(source)?;
    if metadata.is_dir() {
        private_dir(destination)?;
        for entry in fs::read_dir(source).map_err(|e| error(source, e))? {
            let entry = entry.map_err(|e| error(source, e))?;
            let name = entry.file_name();
            // Reject links even if their names would otherwise be excluded.
            checked_metadata(&entry.path())?;
            if name.to_str().map(skipped).unwrap_or(false) {
                continue;
            }
            copy_tree(&entry.path(), &destination.join(name))?;
        }
    } else if metadata.is_file() {
        let mut input = open_read(source)?;
        let mut output = create_file(destination)?;
        std::io::copy(&mut input, &mut output).map_err(|e| error(source, e))?;
        output.sync_all().map_err(|e| error(destination, e))?;
    } else {
        return Err(error(
            source,
            "only regular files and directories can be imported",
        ));
    }
    Ok(())
}

fn copy_sessions(profile: &BrowserProfile, destination: &Path) -> Result<(), String> {
    component(&profile.directory)?;
    let root = Path::new(&profile.data_dir);
    check_ancestors(root)?;
    let source = root.join(&profile.directory);
    check_ancestors(&source)?;
    ensure_closed(profile)?;
    let target = destination.join(&profile.directory);
    private_dir(&target)?;
    let state = root.join("Local State");
    match fs::symlink_metadata(&state) {
        Ok(_) => {
            // Validate JSON before retaining encryption keys. Never decrypt or
            // export passwords; OS/browser identity may still prevent transfer.
            read_json(&state)?;
            copy_tree(&state, &destination.join("Local State"))?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(error(&state, e)),
    }
    for relative in SESSION_PATHS {
        let input = source.join(relative);
        // Check every intermediate directory (notably Network and Service
        // Worker), not just the final file, before opening any session store.
        if let Some(parent) = input.parent() {
            match fs::symlink_metadata(parent) {
                Ok(_) => check_ancestors(parent)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(error(parent, e)),
            }
        }
        match fs::symlink_metadata(&input) {
            Ok(_) => {
                let output = target.join(relative);
                if let Some(parent) = output.parent() {
                    private_dir(parent)?;
                }
                copy_tree(&input, &output)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(error(&input, e)),
        }
    }
    // A source launched during the copy must not be committed as a snapshot.
    ensure_closed(profile)
}

fn commit_stage(stage: &Path, destination: &Path) -> Result<(), String> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let from = CString::new(stage.as_os_str().as_bytes()).map_err(|e| error(stage, e))?;
        let to =
            CString::new(destination.as_os_str().as_bytes()).map_err(|e| error(destination, e))?;
        // An exclusive rename makes the whole directory visible atomically,
        // without a check/rename race that could replace an existing profile.
        #[cfg(target_os = "macos")]
        let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
        #[cfg(target_os = "linux")]
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result != 0 {
            return Err(error(destination, std::io::Error::last_os_error()));
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        // Windows directory rename already refuses an existing destination.
        fs::rename(stage, destination).map_err(|e| error(destination, e))
    }
}

fn create_managed(
    root: &Path,
    mut profile: BrowserProfile,
    source: Option<&BrowserProfile>,
) -> Result<BrowserProfile, String> {
    component(&profile.directory)?;
    if !executable(Path::new(&profile.executable)) {
        return Err(format!(
            "{} is no longer installed at {}",
            profile.browser, profile.executable
        ));
    }
    let profiles = root.join("profiles");
    private_dir(root)?;
    private_dir(&profiles)?;
    let id = Uuid::new_v4().to_string();
    let stage = profiles.join(format!(".import-{id}"));
    let destination = profiles.join(&id);
    profile.id = id;
    profile.data_dir = destination.to_string_lossy().into_owned();
    // create_dir (not create_dir_all) reserves a unique staging directory.
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&stage).map_err(|e| error(&stage, e))?;
    let mut committed = false;
    let result = (|| {
        if let Some(source) = source {
            copy_sessions(source, &stage)?;
        } else {
            private_dir(&stage.join(&profile.directory))?;
        }
        let metadata = serde_json::to_value(&profile).map_err(|e| e.to_string())?;
        write_json(&stage.join("profile.json"), &metadata)?;
        match fs::symlink_metadata(&destination) {
            Ok(_) => {
                return Err(error(
                    &destination,
                    "existing browser profiles cannot be overwritten",
                ))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(error(&destination, e)),
        }
        commit_stage(&stage, &destination)?;
        committed = true;
        persist_selection(root, &profile.id)?;
        Ok(profile)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(if committed { &destination } else { &stage });
    }
    result
}

/// Import into a new persistent profile with the source browser's executable.
/// Cookies remain encrypted: transfer depends on the browser and OS key store.
pub fn import(root: &Path, source_id: &str) -> Result<BrowserProfile, String> {
    let source = sources()?
        .into_iter()
        .find(|profile| profile.id == source_id)
        .ok_or_else(|| {
            "Browser import source no longer exists; refresh the available profiles".to_string()
        })?;
    ensure_closed(&source)?;
    create_managed(root, source.clone(), Some(&source))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("monocode-browser-profile-test-{}", Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(fs::canonicalize(path).unwrap())
        }
        fn source(&self) -> BrowserProfile {
            let root = self.0.join("source");
            fs::create_dir_all(root.join("Default/Network")).unwrap();
            fs::write(
                root.join("Local State"),
                br#"{"os_crypt":{"encrypted_key":"encrypted"}}"#,
            )
            .unwrap();
            fs::write(
                root.join("Default/Network/Cookies"),
                b"encrypted session cookies",
            )
            .unwrap();
            #[cfg(windows)]
            let binary = {
                let binary = self.0.join("fixture-browser.exe");
                fs::write(&binary, b"not a running browser").unwrap();
                binary
            };
            #[cfg(not(windows))]
            let binary = std::env::current_exe().unwrap();
            BrowserProfile {
                id: "source-test".into(),
                name: "Test".into(),
                browser: "Chromium".into(),
                executable: binary.to_string_lossy().into_owned(),
                data_dir: root.to_string_lossy().into_owned(),
                directory: "Default".into(),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn rejects_path_traversal_and_arbitrary_managed_ids() {
        for name in [
            "",
            ".",
            "..",
            "../Default",
            "a/b",
            "a\\b",
            "C:Default",
            "/tmp",
        ] {
            assert!(component(name).is_err(), "accepted {name}");
        }
        assert!(component("Profile 2").is_ok());
        assert!(managed_id("../../outside").is_err());
        assert!(managed_id(&Uuid::new_v4().to_string()).is_ok());
    }

    #[test]
    fn import_preserves_source_and_only_retains_session_storage() {
        let fixture = Fixture::new();
        let source = fixture.source();
        let source_root = Path::new(&source.data_dir);
        fs::write(source_root.join("Default/History"), b"private history").unwrap();
        fs::write(source_root.join("Default/Login Data"), b"passwords").unwrap();
        fs::create_dir(source_root.join("Default/IndexedDB")).unwrap();
        fs::write(source_root.join("Default/IndexedDB/session"), b"token").unwrap();
        let root = fixture.0.join("managed");
        let imported = create_managed(&root, source.clone(), Some(&source)).unwrap();
        let target = Path::new(&imported.data_dir);
        assert_eq!(
            fs::read(target.join("Default/Network/Cookies")).unwrap(),
            b"encrypted session cookies"
        );
        assert_eq!(
            fs::read(target.join("Default/IndexedDB/session")).unwrap(),
            b"token"
        );
        assert_eq!(
            fs::read(source_root.join("Default/Network/Cookies")).unwrap(),
            b"encrypted session cookies"
        );
        assert_eq!(
            fs::read(source_root.join("Default/History")).unwrap(),
            b"private history"
        );
        assert_eq!(
            fs::read(target.join("Local State")).unwrap(),
            fs::read(source_root.join("Local State")).unwrap()
        );
        assert!(!target.join("Default/History").exists());
        assert!(!target.join("Default/Login Data").exists());
        assert_eq!(selected(&root).unwrap().id, imported.id);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(target).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(target.join("Local State"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_failure_leaves_previous_selection_and_no_partial_import() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let source = fixture.source();
        let root = fixture.0.join("managed");
        let previous = create_managed(&root, source.clone(), None).unwrap();
        let outside = fixture.0.join("outside");
        fs::write(&outside, b"must not copy").unwrap();
        fs::create_dir(Path::new(&source.data_dir).join("Default/IndexedDB")).unwrap();
        symlink(
            &outside,
            Path::new(&source.data_dir).join("Default/IndexedDB/escape"),
        )
        .unwrap();
        assert!(create_managed(&root, source.clone(), Some(&source)).is_err());
        assert_eq!(selected(&root).unwrap().id, previous.id);
        assert_eq!(fs::read_dir(root.join("profiles")).unwrap().count(), 1);
        assert_eq!(fs::read(&outside).unwrap(), b"must not copy");
        assert!(
            fs::symlink_metadata(Path::new(&source.data_dir).join("Default/IndexedDB/escape"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_intermediate_and_live_source() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let source = fixture.source();
        let network = Path::new(&source.data_dir).join("Default/Network");
        fs::remove_file(network.join("Cookies")).unwrap();
        fs::remove_dir(&network).unwrap();
        let outside = fixture.0.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("Cookies"), b"outside").unwrap();
        symlink(&outside, &network).unwrap();
        let root = fixture.0.join("managed");
        assert!(create_managed(&root, source.clone(), Some(&source)).is_err());
        assert!(profiles(&root).unwrap().is_empty());
        assert!(!root.join("selected.json").exists());
        fs::remove_file(&network).unwrap();
        fs::create_dir(&network).unwrap();
        symlink(
            "host-123",
            Path::new(&source.data_dir).join("SingletonLock"),
        )
        .unwrap();
        let message = create_managed(&root, source.clone(), Some(&source)).unwrap_err();
        assert!(message.contains("Close Chromium"));
        assert!(profiles(&root).unwrap().is_empty());
        assert!(!root.join("selected.json").exists());
    }

    #[test]
    fn discovery_uses_local_state_names_and_stable_opaque_ids() {
        let fixture = Fixture::new();
        let source = fixture.source();
        let source_root = Path::new(&source.data_dir);
        fs::create_dir(source_root.join("Profile 2")).unwrap();
        fs::write(source_root.join("Local State"), br#"{"profile":{"info_cache":{"Default":{"name":"Personal"},"Profile 2":{"name":"Work"}}}}"#).unwrap();
        let installation = Installation {
            browser: "Chromium",
            executable: PathBuf::from(&source.executable),
            data_dir: source_root.to_path_buf(),
        };
        let discovered = discover(&installation).unwrap();
        assert_eq!(discovered.len(), 2);
        assert_eq!(discovered[0].name, "Chromium — Personal");
        assert_eq!(discovered[1].directory, "Profile 2");
        assert_eq!(discovered[1].name, "Chromium — Work");
        assert_eq!(discovered[1].id, source_id("Chromium", "Profile 2"));
        assert!(!discovered[1].id.contains("Profile 2"));
        fs::write(
            source_root.join("Local State"),
            br#"{"profile":{"info_cache":{"../escape":{"name":"Invalid"}}}}"#,
        )
        .unwrap();
        assert!(discover(&installation).is_err());
        fs::write(source_root.join("Local State"), b"{}").unwrap();
        let fallback = discover(&installation).unwrap();
        assert_eq!(fallback.len(), 1);
        assert_eq!(fallback[0].directory, "Default");
    }

    #[test]
    fn selection_write_failure_removes_committed_import() {
        let fixture = Fixture::new();
        let source = fixture.source();
        let root = fixture.0.join("managed");
        private_dir(&root).unwrap();
        fs::create_dir(root.join("selected.json")).unwrap();
        assert!(create_managed(&root, source.clone(), Some(&source)).is_err());
        assert!(profiles(&root).unwrap().is_empty());
        assert_eq!(fs::read_dir(root.join("profiles")).unwrap().count(), 0);
        assert!(root.join("selected.json").is_dir());
        assert_eq!(
            fs::read(Path::new(&source.data_dir).join("Default/Network/Cookies")).unwrap(),
            b"encrypted session cookies"
        );
    }

    #[test]
    fn atomic_commit_never_overwrites_existing_profile() {
        let fixture = Fixture::new();
        let stage = fixture.0.join("stage");
        let destination = fixture.0.join("existing");
        private_dir(&stage).unwrap();
        private_dir(&destination).unwrap();
        fs::write(stage.join("profile.json"), b"new profile").unwrap();
        fs::write(destination.join("profile.json"), b"existing profile").unwrap();
        assert!(commit_stage(&stage, &destination).is_err());
        assert_eq!(
            fs::read(destination.join("profile.json")).unwrap(),
            b"existing profile"
        );
        assert_eq!(
            fs::read(stage.join("profile.json")).unwrap(),
            b"new profile"
        );
    }

    #[test]
    fn listing_does_not_create_directories_and_stale_selection_errors() {
        let fixture = Fixture::new();
        let root = fixture.0.join("missing");
        assert!(profiles(&root).unwrap().is_empty());
        assert!(!root.exists());
        private_dir(&root).unwrap();
        write_json(
            &root.join("selected.json"),
            &json!({"id": Uuid::new_v4().to_string()}),
        )
        .unwrap();
        assert!(selected(&root).unwrap_err().contains("unavailable"));
    }
}
