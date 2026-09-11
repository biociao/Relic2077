//! Explicit, inspectable host setup. Configuration presence is not host execution.
use crate::{automation, hooks, vault::Vault};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};
#[derive(Default, Serialize, Deserialize)]
pub struct Setup {
    pub project: PathBuf,
    pub dsh_home: PathBuf,
    pub executable: PathBuf,
}
pub fn default_dsh_home() -> PathBuf {
    std::env::var_os("DSH_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(
                std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .unwrap_or_default(),
            )
            .join(".dsh")
        })
}
fn read_setup(v: &Vault) -> Result<Setup> {
    automation::read(&v.root.join(".relic/automation/setup.json"))
}
fn plugin_path(v: &Vault) -> PathBuf {
    v.root.join(".relic/automation/dsh-relic.mjs")
}
fn dsh_item(v: &Vault, s: &Setup) -> Value {
    json!({"name":plugin_path(v),"config":{"executable":s.executable,"vault":v.root,"project":s.project}})
}
fn dsh_config(path: &Path) -> Result<Vec<Value>> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let raw = fs::read_to_string(path)?;
    if raw.trim().is_empty() {
        return Ok(vec![]);
    }
    serde_yaml::from_str(&raw).context("DSH configuration must be a YAML plugin list")
}
fn backup(path: &Path) -> Result<()> {
    if path.exists() {
        fs::copy(
            path,
            path.with_extension(format!("bak-{}", uuid::Uuid::new_v4())),
        )?;
    }
    Ok(())
}
pub fn install(
    v: &Vault,
    project: &Path,
    dsh_home: &Path,
    executable: &Path,
    host: &str,
) -> Result<()> {
    ensure!(matches!(host, "codex" | "dsh"), "Unsupported host");
    ensure!(
        project.is_absolute() && project.is_dir(),
        "Project must be an existing absolute directory"
    );
    ensure!(dsh_home.is_absolute(), "DSH home must be absolute");
    let s = Setup {
        project: fs::canonicalize(project)?,
        dsh_home: dsh_home.to_owned(),
        executable: fs::canonicalize(executable)?,
    };
    let _guard = automation::lock(v, "setup.lock")?;
    if host == "codex" {
        hooks::install(
            &s.project.join(".codex/hooks.json"),
            &v.root,
            &s.executable,
            false,
        )?;
    } else {
        let path = s.dsh_home.join("cordis.patch.yml");
        let mut items = dsh_config(&path)?;
        let item = dsh_item(v, &s);
        items.retain(|x| x["name"] != item["name"]);
        items.push(item);
        let raw = serde_yaml::to_string(&items)?;
        fs::create_dir_all(&s.dsh_home)?;
        backup(&path)?;
        crate::capture::atomic_write(
            &plugin_path(v),
            include_bytes!("../integrations/dsh/relic.mjs"),
        )?;
        crate::capture::atomic_write(&path, raw.as_bytes())?;
    }
    automation::save(&v.root.join(".relic/automation/setup.json"), &s)
}
pub fn status(v: &Vault, events: &[Value]) -> Result<Value> {
    let mut s = read_setup(v)?;
    if s.project.as_os_str().is_empty() {
        s.project = std::env::current_dir()?;
    }
    if s.dsh_home.as_os_str().is_empty() {
        s.dsh_home = default_dsh_home();
    }
    if s.executable.as_os_str().is_empty() {
        s.executable = std::env::current_exe()?;
    }
    let codex = s.project.join(".codex/hooks.json");
    let dsh = s.dsh_home.join("cordis.patch.yml");
    let check_codex = || -> Result<bool> {
        if !codex.exists() {
            return Ok(false);
        }
        let actual: Value = serde_json::from_slice(&fs::read(&codex)?)?;
        Ok(actual == hooks::install(&codex, &v.root, &s.executable, true)?)
    };
    let check_dsh = || -> Result<bool> {
        Ok(dsh_config(&dsh)?.contains(&dsh_item(v, &s))
            && fs::read(plugin_path(v)).unwrap_or_default()
                == include_bytes!("../integrations/dsh/relic.mjs"))
    };
    let host = |name: &str, check: Result<bool>, path: &Path| {
        let last = events.iter().rev().find(|e| {
            e["host"] == name
                && e["project"]
                    .as_str()
                    .is_some_and(|p| Path::new(p).starts_with(&s.project))
        });
        let installed = check.as_ref().copied().unwrap_or(false);
        json!({"installed":installed,"path":path,"error":check.err().map(|e|e.to_string()),
            "state":if !installed {"not_installed"} else if last.is_some_and(|e| e["status"] == "error") {"error"} else if last.is_some(){"triggered"} else {"awaiting_event"},"last_event":last})
    };
    Ok(
        json!({"project":s.project,"dsh_home":s.dsh_home,"codex":host("codex",check_codex(),&codex),"dsh":host("dsh",check_dsh(),&dsh),"worker_running":automation::running(v)?}),
    )
}
