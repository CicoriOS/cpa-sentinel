mod agent;
mod bisect;
mod cpa;
mod errlog;
mod rewrite;

use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tracing::{error, info};

const POLL: Duration = Duration::from_secs(5);
/// A system prompt that was already investigated is not investigated again for this long.
const RECHECK_AFTER: Duration = Duration::from_secs(30 * 60);

fn env(key: &str) -> Result<String> {
    std::env::var(key).with_context(|| format!("{key} is not set"))
}

async fn error_logs(dir: &PathBuf) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut rd = tokio::fs::read_dir(dir).await.with_context(|| format!("read {}", dir.display()))?;
    while let Some(e) = rd.next_entry().await? {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with("error-v1beta-models-") && name.ends_with(".log") {
            out.push(name);
        }
    }
    Ok(out)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_target(false).init();

    let cpa = cpa::Cpa::new(
        &std::env::var("CPA_URL").unwrap_or_else(|_| "http://127.0.0.1:8318".into()),
        env("CPA_MANAGEMENT_KEY")?,
        env("CPA_API_KEY")?,
    )?;
    let log_dir = PathBuf::from(std::env::var("CPA_LOG_DIR").unwrap_or_else(|_| "/opt/cliproxyapi/logs".into()));
    let opts = agent::Options {
        dry_run: std::env::var("SENTINEL_DRY_RUN").is_ok_and(|v| v == "1"),
        restart_cmd: std::env::var("SENTINEL_RESTART_CMD").ok().filter(|s| !s.is_empty()),
        agy: rewrite::Agy::from_env()?,
    };

    let mut seen: HashSet<String> = error_logs(&log_dir).await?.into_iter().collect();
    let mut handled: HashMap<u64, Instant> = HashMap::new();
    info!(dir = %log_dir.display(), existing = seen.len(), dry_run = opts.dry_run, "watching CPA error logs");

    loop {
        tokio::time::sleep(POLL).await;
        let mut fresh: Vec<String> = match error_logs(&log_dir).await {
            Ok(v) => v.into_iter().filter(|n| !seen.contains(n)).collect(),
            Err(e) => {
                error!("{e:#}");
                continue;
            }
        };
        fresh.sort();
        for name in fresh {
            // Let CPA finish writing the file.
            tokio::time::sleep(Duration::from_secs(1)).await;
            seen.insert(name.clone());
            let Ok(text) = tokio::fs::read_to_string(log_dir.join(&name)).await else { continue };
            let Some(blocked) = errlog::parse(&text, cpa::USER_AGENT) else { continue };
            let Some(system) = errlog::system_text(&blocked.body) else { continue };

            let mut h = DefaultHasher::new();
            system.hash(&mut h);
            let key = h.finish();
            if handled.get(&key).is_some_and(|t| t.elapsed() < RECHECK_AFTER) {
                continue;
            }
            handled.insert(key, Instant::now());

            info!(%name, model = %blocked.model, "Gemini 429 from upstream, investigating");
            if let Err(e) = agent::handle(&cpa, blocked, &opts).await {
                error!("{e:#}");
            }
            // Probes write their own error logs; do not treat them as new work.
            seen.extend(error_logs(&log_dir).await.unwrap_or_default());
        }
    }
}
