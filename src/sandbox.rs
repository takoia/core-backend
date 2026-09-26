//! Pluggable execution sandbox for agent `claude -p` runs.
//!
//! Autonomous full-auto agents can write and run code via their tools, so the
//! subprocess is confined according to the backend chosen in Settings:
//!
//! - `none`        — host execution (no isolation).
//! - `landlock`    — native Linux Landlock LSM (filesystem confinement, no
//!                   external binary, no KVM). Applied in `pre_exec`. Default.
//! - `bubblewrap`  — `bwrap` namespaces (filesystem + optional network).
//! - `nsjail`      — `nsjail` namespaces.
//! - `docker` / `podman` — one container per run (needs an image with `claude`).
//! - `firecracker` / `microsandbox` — microVM per run (needs `/dev/kvm`).
//!
//! Network stays available by default because `claude -p` must reach the model
//! API; the isolation protects the host filesystem and the other agents.

use crate::db::Db;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use tokio::process::Command;

pub const BACKENDS: [&str; 8] = [
    "none",
    "landlock",
    "bubblewrap",
    "nsjail",
    "docker",
    "podman",
    "firecracker",
    "microsandbox",
];

#[derive(Clone, Debug)]
pub struct SandboxConfig {
    pub kind: String,
    pub params: Value,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            kind: "landlock".into(),
            params: json!({}),
        }
    }
}

/// Read the active sandbox config (singleton row). Migration 0015 seeds the row
/// with `landlock`, and the in-code default matches it, so a fresh install is
/// confined without any configuration.
pub async fn active(db: &Db) -> SandboxConfig {
    let row: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT kind, params FROM sandbox_backend WHERE id = 1")
            .fetch_optional(db)
            .await
            .ok()
            .flatten();
    match row {
        Some((kind, params)) => SandboxConfig {
            kind,
            params: params
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| json!({})),
        },
        None => SandboxConfig::default(),
    }
}

/// Persist the active sandbox selection + config.
pub async fn set_active(db: &Db, kind: &str, params: &Value) -> Result<()> {
    if !BACKENDS.contains(&kind) {
        return Err(anyhow!("unknown sandbox backend: {kind}"));
    }
    let blob = serde_json::to_string(params)?;
    sqlx::query(
        "UPDATE sandbox_backend SET kind = ?, params = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = 1",
    )
    .bind(kind)
    .bind(blob)
    .execute(db)
    .await?;
    Ok(())
}

/// Probe whether the host can actually run a given sandbox backend (without
/// affecting the running server). Used by the Settings "test" button.
pub async fn probe(cfg: &SandboxConfig) -> Result<String> {
    match cfg.kind.as_str() {
        "none" => Ok("host execution (no isolation)".into()),
        "landlock" => probe_landlock(),
        "bubblewrap" => probe_bin("bwrap", &["--version"]).await,
        "nsjail" => probe_bin("nsjail", &["--help"]).await,
        "docker" => probe_bin("docker", &["version", "--format", "{{.Server.Version}}"]).await,
        "podman" => probe_bin("podman", &["--version"]).await,
        "firecracker" => probe_microvm("firecracker").await,
        "microsandbox" => {
            let l = pstr(cfg, "launcher");
            probe_microvm(if l.is_empty() { "microsandbox" } else { l }).await
        }
        other => Err(anyhow!("unknown backend: {other}")),
    }
}

#[cfg(not(target_os = "linux"))]
fn probe_landlock() -> Result<String> {
    Err(anyhow!(
        "landlock is a Linux LSM and is unavailable on this host; pick another sandbox backend"
    ))
}

#[cfg(target_os = "linux")]
fn probe_landlock() -> Result<String> {
    use landlock::{Access, AccessFs, Ruleset, RulesetAttr, ABI};
    // create() opens the ruleset fd WITHOUT restrict_self, so the server process
    // is never confined by the probe; the fd is dropped immediately.
    Ruleset::default()
        .handle_access(AccessFs::from_all(ABI::V1))?
        .create()?;
    Ok("landlock available — filesystem confinement applied per run".into())
}

async fn probe_bin(bin: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(bin)
        .args(args)
        .output()
        .await
        .map_err(|_| anyhow!("`{bin}` not found on host"))?;
    if out.status.success() {
        Ok(format!("`{bin}` available"))
    } else {
        Err(anyhow!("`{bin}` present but returned an error"))
    }
}

async fn probe_microvm(bin: &str) -> Result<String> {
    if !std::path::Path::new("/dev/kvm").exists() {
        return Err(anyhow!(
            "/dev/kvm missing — host has no (nested) virtualization"
        ));
    }
    probe_bin(bin, &["--version"]).await
}

fn pbool(cfg: &SandboxConfig, key: &str, default: bool) -> bool {
    cfg.params
        .get(key)
        .and_then(|v| v.as_bool())
        .unwrap_or(default)
}
fn pstr<'a>(cfg: &'a SandboxConfig, key: &str) -> &'a str {
    cfg.params.get(key).and_then(|v| v.as_str()).unwrap_or("")
}
fn pu64(cfg: &SandboxConfig, key: &str) -> Option<u64> {
    cfg.params
        .get(key)
        .and_then(|v| v.as_u64())
        .filter(|n| *n > 0)
}
fn extra_args(cfg: &SandboxConfig) -> Vec<String> {
    cfg.params
        .get("extra_args")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Environment for a sandboxed `claude` child: an explicit whitelist, never the
/// server's environment, which holds MASTER_KEY, provider API keys and the
/// database URL and is readable through /proc/self/environ by a confined child.
/// `own_home` points HOME at the workdir so config/cache writes stay inside it.
pub fn child_env(workdir: &str, token: Option<&str>, own_home: bool) -> Vec<(String, String)> {
    let tmp = format!("{workdir}/tmp");
    let _ = std::fs::create_dir_all(&tmp);
    let mut env: Vec<(String, String)> = vec![
        (
            "PATH".into(),
            std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into()),
        ),
        ("TMPDIR".into(), tmp),
        ("LANG".into(), "C.UTF-8".into()),
    ];
    if own_home {
        env.push(("HOME".into(), workdir.to_string()));
    } else if let Ok(home) = std::env::var("HOME") {
        env.push(("HOME".into(), home));
    }
    if let Some(t) = token {
        env.push(("CLAUDE_CODE_OAUTH_TOKEN".into(), t.to_string()));
    }
    env
}

fn apply_child_env(cmd: &mut std::process::Command, env: &[(String, String)]) {
    cmd.env_clear();
    for (k, v) in env {
        cmd.env(k, v);
    }
}

/// Build the `tokio` command that runs `program args...` with cwd `workdir`,
/// confined according to the active sandbox backend. `token` is injected into
/// the sandboxed environment when present.
pub fn build_command(
    cfg: &SandboxConfig,
    workdir: &str,
    program: &str,
    args: &[String],
    token: Option<&str>,
) -> Command {
    match cfg.kind.as_str() {
        "landlock" => {
            // Native FS confinement, applied to the child via pre_exec.
            let mut std_cmd = std::process::Command::new(program);
            std_cmd.args(args).current_dir(workdir);
            apply_child_env(&mut std_cmd, &child_env(workdir, token, true));
            attach_landlock(&mut std_cmd, workdir, pbool(cfg, "share_tmp", true));
            Command::from(std_cmd)
        }
        "bubblewrap" => {
            let mut a: Vec<String> = vec![
                "--die-with-parent".into(),
                "--unshare-pid".into(),
                "--unshare-ipc".into(),
                "--unshare-uts".into(),
                "--new-session".into(),
                "--ro-bind".into(),
                "/usr".into(),
                "/usr".into(),
                "--ro-bind".into(),
                "/etc".into(),
                "/etc".into(),
                "--ro-bind-try".into(),
                "/bin".into(),
                "/bin".into(),
                "--ro-bind-try".into(),
                "/sbin".into(),
                "/sbin".into(),
                "--ro-bind-try".into(),
                "/lib".into(),
                "/lib".into(),
                "--ro-bind-try".into(),
                "/lib64".into(),
                "/lib64".into(),
                "--proc".into(),
                "/proc".into(),
                "--dev".into(),
                "/dev".into(),
                "--tmpfs".into(),
                "/tmp".into(),
                "--bind".into(),
                workdir.into(),
                workdir.into(),
                "--chdir".into(),
                workdir.into(),
                "--clearenv".into(),
            ];
            for (k, v) in child_env(workdir, token, true) {
                a.extend(["--setenv".into(), k, v]);
            }
            if !pbool(cfg, "allow_network", true) {
                a.push("--unshare-net".into());
            }
            a.extend(extra_args(cfg));
            a.push("--".into());
            a.push(program.into());
            a.extend(args.iter().cloned());
            let mut c = Command::new("bwrap");
            c.args(a);
            c
        }
        "nsjail" => {
            let mut a: Vec<String> = vec![
                "-Mo".into(),
                "--rlimit_as".into(),
                "max".into(),
                "--chroot".into(),
                "/".into(),
                "--cwd".into(),
                workdir.into(),
                "--bindmount".into(),
                format!("{workdir}:{workdir}"),
            ];
            if pbool(cfg, "allow_network", true) {
                a.push("--disable_clone_newnet".into());
            }
            // nsjail starts from an empty environment; pass the whitelist only.
            for (k, v) in child_env(workdir, token, true) {
                a.extend(["--env".into(), format!("{k}={v}")]);
            }
            a.extend(extra_args(cfg));
            a.push("--".into());
            a.push(program.into());
            a.extend(args.iter().cloned());
            let mut c = Command::new("nsjail");
            c.args(a);
            c
        }
        "docker" | "podman" => {
            let engine = cfg.kind.as_str();
            let image = {
                let i = pstr(cfg, "image");
                if i.is_empty() {
                    "takoia/agent-runtime:latest"
                } else {
                    i
                }
            };
            let mut a: Vec<String> = vec![
                "run".into(),
                "--rm".into(),
                "-v".into(),
                format!("{workdir}:{workdir}"),
                "-w".into(),
                workdir.into(),
                "-i".into(),
            ];
            if !pbool(cfg, "allow_network", true) {
                a.extend(["--network".into(), "none".into()]);
            }
            if let Some(m) = pu64(cfg, "mem_mb") {
                a.extend(["--memory".into(), format!("{m}m")]);
            }
            if let Some(c) = pu64(cfg, "cpus") {
                a.extend(["--cpus".into(), c.to_string()]);
            }
            if let Some(t) = token {
                a.extend(["-e".into(), format!("CLAUDE_CODE_OAUTH_TOKEN={t}")]);
            }
            a.extend(extra_args(cfg));
            a.push(image.into());
            a.push(program.into());
            a.extend(args.iter().cloned());
            let mut c = Command::new(engine);
            c.args(a);
            c
        }
        "firecracker" | "microsandbox" => {
            // microVM backends need /dev/kvm and a prepared rootfs/VM image with
            // `claude` inside; the operator provides a launcher that receives the
            // workdir, the program and its args. Defaults to the named CLI.
            let launcher = {
                let l = pstr(cfg, "launcher");
                if l.is_empty() {
                    cfg.kind.clone()
                } else {
                    l.to_string()
                }
            };
            let mut a: Vec<String> = vec!["--workdir".into(), workdir.into()];
            if let Some(t) = token {
                a.extend(["--env".into(), format!("CLAUDE_CODE_OAUTH_TOKEN={t}")]);
            }
            a.extend(extra_args(cfg));
            a.push(program.into());
            a.extend(args.iter().cloned());
            let mut c = Command::new(launcher);
            c.args(a);
            c
        }
        // "none" and anything unknown: run directly on the host, still with a
        // cleared environment. Without a plan token the parent HOME is kept so a
        // developer's own `claude` login keeps working.
        _ => {
            let mut std_cmd = std::process::Command::new(program);
            std_cmd.args(args).current_dir(workdir);
            apply_child_env(&mut std_cmd, &child_env(workdir, token, token.is_some()));
            Command::from(std_cmd)
        }
    }
}

/// Self-test the Landlock filesystem confinement using the exact rules
/// `build_command` applies. Restricts THIS process, then probes writes — run it
/// as a throwaway `takoia sandbox-selftest <workdir>` process, never the server.
#[cfg(not(target_os = "linux"))]
pub fn selftest(_workdir: &str) -> String {
    "landlock restrict_self: UNAVAILABLE (Linux-only LSM)\n".to_string()
}

/// Self-test the Landlock filesystem confinement using the exact rules
/// `build_command` applies. Restricts THIS process, then probes writes — run it
/// as a throwaway `takoia sandbox-selftest <workdir>` process, never the server.
#[cfg(target_os = "linux")]
pub fn selftest(workdir: &str) -> String {
    use landlock::{
        path_beneath_rules, Access, AccessFs, Ruleset, RulesetAttr, RulesetCreatedAttr, ABI,
    };
    let _ = std::fs::create_dir_all(workdir);
    let abi = ABI::V1;
    // `/run` is needed so the systemd-resolved resolv.conf symlink resolves
    // (DNS); without it `claude` cannot reach the model API and exits.
    let ro = [
        "/usr", "/etc", "/bin", "/sbin", "/lib", "/lib64", "/dev", "/proc", "/run", "/sys",
    ];
    let rw = [workdir.to_string(), "/tmp".to_string()];
    let applied = (|| {
        Ruleset::default()
            .handle_access(AccessFs::from_all(abi))?
            .create()?
            .add_rules(path_beneath_rules(ro, AccessFs::from_read(abi)))?
            .add_rules(path_beneath_rules(rw, AccessFs::from_all(abi)))?
            .restrict_self()
    })();
    let mut out = String::new();
    match applied {
        Ok(_) => out.push_str("landlock restrict_self: APPLIED\n"),
        Err(e) => return format!("landlock restrict_self: FAILED ({e})\n"),
    }
    let inside = std::fs::write(format!("{workdir}/ll_ok.txt"), b"ok");
    out.push_str(&format!(
        "write {workdir}/ll_ok.txt   -> {}\n",
        inside
            .map(|_| "OK (workdir allowed)".to_string())
            .unwrap_or_else(|e| format!("FAIL {e}"))
    ));
    let escape = std::fs::write("/home/takoia/ll_escape.txt", b"pwned");
    out.push_str(&format!(
        "write /home/takoia/ll_escape.txt -> {}\n",
        escape
            .map(|_| "OK -- LEAK!".to_string())
            .unwrap_or_else(|e| format!("BLOCKED ({})", e.kind()))
    ));
    let read_etc = std::fs::read("/etc/hostname");
    out.push_str(&format!(
        "read  /etc/hostname        -> {}\n",
        read_etc
            .map(|_| "OK (system readable)".to_string())
            .unwrap_or_else(|e| format!("FAIL {e}"))
    ));
    out
}

/// Non-Linux hosts have no Landlock: keep the same fail-closed contract as the
/// Linux path (refuse to spawn unconfined) so a misconfigured dev box never
/// silently runs the agent on the bare host.
#[cfg(not(target_os = "linux"))]
fn attach_landlock(cmd: &mut std::process::Command, _workdir: &str, _share_tmp: bool) {
    use std::os::unix::process::CommandExt;
    let msg = "landlock sandbox unavailable on this OS, refusing to run unconfined \
               (select another sandbox backend in Settings)";
    tracing::error!("{msg}");
    unsafe {
        cmd.pre_exec(move || Err(std::io::Error::new(std::io::ErrorKind::Other, msg)));
    }
}

/// Attach a Landlock filesystem restriction to a std command via `pre_exec`.
/// The ruleset is built in the parent (allowed allocation); only the
/// `restrict_self` syscall runs post-fork in the child.
#[cfg(target_os = "linux")]
fn attach_landlock(cmd: &mut std::process::Command, workdir: &str, share_tmp: bool) {
    use landlock::{
        path_beneath_rules, Access, AccessFs, Ruleset, RulesetAttr, RulesetCreatedAttr, ABI,
    };
    use std::os::unix::process::CommandExt;

    // ABI V1 (Linux 5.13+) for the widest kernel support; best-effort so a newer
    // kernel still applies what it can and an older one degrades gracefully.
    let abi = ABI::V1;
    // Read-only system paths the runtime needs; read/write only the workdir + /tmp.
    // `/run` is needed so the systemd-resolved resolv.conf symlink resolves
    // (DNS); without it `claude` cannot reach the model API and exits.
    let ro = [
        "/usr", "/etc", "/bin", "/sbin", "/lib", "/lib64", "/dev", "/proc", "/run", "/sys",
    ];
    // The workdir and its private tmp (TMPDIR) are always writable. `/tmp` stays
    // writable by default because the CLI may still touch it directly, but it is
    // shared by every agent on the host: set the sandbox param share_tmp=false
    // to close it once TMPDIR is confirmed to be honoured on the target.
    let mut rw = vec![workdir.to_string(), format!("{workdir}/tmp")];
    if share_tmp {
        rw.push("/tmp".to_string());
    }

    let built = (|| {
        Ruleset::default()
            .handle_access(AccessFs::from_all(abi))?
            .create()?
            .add_rules(path_beneath_rules(ro, AccessFs::from_read(abi)))?
            .add_rules(path_beneath_rules(rw, AccessFs::from_all(abi)))
    })();

    match built {
        Ok(ruleset) => {
            let mut cell = Some(ruleset);
            unsafe {
                cmd.pre_exec(move || {
                    if let Some(r) = cell.take() {
                        r.restrict_self().map_err(|e| {
                            std::io::Error::new(
                                std::io::ErrorKind::Other,
                                format!("landlock restrict failed: {e}"),
                            )
                        })?;
                    }
                    Ok(())
                });
            }
        }
        Err(e) => {
            // Fail closed: the operator selected the Landlock sandbox, so if the
            // ruleset cannot be applied we refuse to spawn rather than silently
            // running the agent unconfined on the host.
            let msg = format!("landlock sandbox unavailable, refusing to run unconfined: {e}");
            tracing::error!("{msg}");
            unsafe {
                cmd.pre_exec(move || {
                    Err(std::io::Error::new(std::io::ErrorKind::Other, msg.clone()))
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(kind: &str, params: Value) -> SandboxConfig {
        SandboxConfig {
            kind: kind.into(),
            params,
        }
    }

    fn envs(cmd: &Command) -> Vec<(String, Option<String>)> {
        cmd.as_std()
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect()
    }

    #[test]
    fn host_backend_never_inherits_the_server_environment() {
        std::env::set_var("TAKOIA_TEST_SECRET", "leak-me");
        let wd = std::env::temp_dir().join("takoia-env-test");
        let cmd = build_command(
            &cfg("none", json!({})),
            wd.to_str().unwrap(),
            "true",
            &[],
            Some("tok"),
        );
        assert!(
            cmd.as_std().get_envs().len() > 0,
            "env_clear must have been called (get_envs lists the explicit set)"
        );
        let set = envs(&cmd);
        assert!(set.iter().all(|(k, _)| k != "TAKOIA_TEST_SECRET"));
        assert!(set
            .iter()
            .any(|(k, v)| k == "CLAUDE_CODE_OAUTH_TOKEN" && v.as_deref() == Some("tok")));
        assert!(set
            .iter()
            .any(|(k, v)| k == "HOME" && v.as_deref() == wd.to_str()));
        assert!(set.iter().any(|(k, _)| k == "PATH"));
        assert!(set.iter().any(|(k, _)| k == "TMPDIR"));
    }

    #[test]
    fn host_backend_without_token_keeps_developer_home() {
        let wd = std::env::temp_dir().join("takoia-env-test2");
        let cmd = build_command(
            &cfg("none", json!({})),
            wd.to_str().unwrap(),
            "true",
            &[],
            None,
        );
        let set = envs(&cmd);
        let home = std::env::var("HOME").ok();
        assert!(set.iter().any(|(k, v)| k == "HOME" && *v == home));
        assert!(set.iter().all(|(k, _)| k != "CLAUDE_CODE_OAUTH_TOKEN"));
    }

    #[test]
    fn bubblewrap_clears_env_before_setting_the_whitelist() {
        let wd = std::env::temp_dir().join("takoia-env-test3");
        let cmd = build_command(
            &cfg("bubblewrap", json!({})),
            wd.to_str().unwrap(),
            "claude",
            &["-p".into()],
            Some("tok"),
        );
        let args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let clear = args
            .iter()
            .position(|a| a == "--clearenv")
            .expect("--clearenv present");
        let first_setenv = args
            .iter()
            .position(|a| a == "--setenv")
            .expect("--setenv present");
        assert!(
            clear < first_setenv,
            "--clearenv must precede every --setenv"
        );
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--setenv" && w[1] == "CLAUDE_CODE_OAUTH_TOKEN"));
    }
}
