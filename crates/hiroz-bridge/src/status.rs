//! PID-file-based bridge status tracking.
//!
//! When `hu bridge start` launches, it writes a state file to
//! `$XDG_RUNTIME_DIR/hu/bridge.state` (falling back to
//! `$HOME/.local/state/hu/bridge.state`). The file contains JSON with the PID,
//! start time, and bridge mode. `hu bridge status` reads it and checks whether
//! the recorded PID is still alive.

use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeState {
    pub pid: u32,
    pub started_at: u64, // Unix seconds
    pub mode: BridgeMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BridgeMode {
    CrossDistro {
        pair: String,
        source_endpoint: String,
        target_endpoint: String,
        domain: usize,
    },
    CrossDds {
        endpoint: String,
        domain: usize,
        allow: Option<String>,
        deny: Option<String>,
    },
    Combined {
        distro_pair: String,
        source_endpoint: String,
        target_endpoint: String,
        dds_endpoint: String,
        domain: usize,
    },
}

fn state_path() -> PathBuf {
    let base = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs_next::home_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".local/state")
        });
    base.join("hu").join("bridge.state")
}

pub fn write(state: &BridgeState) -> Result<()> {
    let path = state_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_string_pretty(state)?)?;
    Ok(())
}

pub fn remove() {
    let _ = fs::remove_file(state_path());
}

fn read() -> Option<BridgeState> {
    let data = fs::read_to_string(state_path()).ok()?;
    serde_json::from_str(&data).ok()
}

fn pid_alive(pid: u32) -> bool {
    // Send signal 0 — checks existence without disturbing the process.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

fn format_uptime(started_at: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let secs = now.saturating_sub(started_at);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

pub fn show(json: bool) -> Result<()> {
    match read() {
        None => {
            if json {
                println!("{}", serde_json::json!({"running": false}));
            } else {
                println!("hu-bridge: not running");
            }
        }
        Some(state) if !pid_alive(state.pid) => {
            // Stale state file — process is gone.
            remove();
            if json {
                println!(
                    "{}",
                    serde_json::json!({"running": false, "stale_pid": state.pid})
                );
            } else {
                println!(
                    "hu-bridge: not running (stale PID {} cleaned up)",
                    state.pid
                );
            }
        }
        Some(state) => {
            let uptime = format_uptime(state.started_at);
            if json {
                let mut v = serde_json::to_value(&state)?;
                v["running"] = serde_json::json!(true);
                v["uptime"] = serde_json::json!(uptime);
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                println!("hu-bridge: running (PID {}, uptime {})", state.pid, uptime);
                match &state.mode {
                    BridgeMode::CrossDistro {
                        pair,
                        source_endpoint,
                        target_endpoint,
                        domain,
                    } => {
                        println!("  mode:     cross-distro ({pair}), domain {domain}");
                        println!("  source:   {source_endpoint}");
                        println!("  target:   {target_endpoint}");
                    }
                    BridgeMode::CrossDds {
                        endpoint,
                        domain,
                        allow,
                        deny,
                    } => {
                        println!("  mode:     cross-DDS, domain {domain}");
                        println!("  endpoint: {endpoint}");
                        if let Some(r) = allow {
                            println!("  allow:    {r}");
                        }
                        if let Some(r) = deny {
                            println!("  deny:     {r}");
                        }
                    }
                    BridgeMode::Combined {
                        distro_pair,
                        source_endpoint,
                        target_endpoint,
                        dds_endpoint,
                        domain,
                    } => {
                        println!("  mode:     cross-distro + cross-DDS, domain {domain}");
                        println!(
                            "  distro:   {distro_pair} ({source_endpoint} ↔ {target_endpoint})"
                        );
                        println!("  dds:      {dds_endpoint}");
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
