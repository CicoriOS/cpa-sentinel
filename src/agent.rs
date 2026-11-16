use crate::bisect::minimal_blocking;
use crate::cpa::{Cpa, Probe};
use crate::errlog::{Blocked429, system_text};
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;
use tracing::{info, warn};

const MAX_ROUNDS: usize = 12;

pub struct Options {
    pub dry_run: bool,
    /// Shell command run when a hot reload did not clear the 429 (e.g. `systemctl restart cliproxyapi`).
    pub restart_cmd: Option<String>,
    /// Rewrites trigger lines with Gemini; `None` deletes them.
    pub agy: Option<crate::rewrite::Agy>,
}

fn request(system: Option<&str>) -> Value {
    let mut body = json!({ "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }] });
    if let Some(s) = system {
        body["systemInstruction"] = json!({ "parts": [{ "text": s }] });
    }
    body
}

/// Find the system-prompt lines that make Antigravity answer 429, then hot-patch
/// `sensitive-words-replace` so CPA deletes them before forwarding.
pub async fn handle(cpa: &Cpa, blocked: Blocked429, opts: &Options) -> Result<()> {
    let Some(original) = system_text(&blocked.body) else {
        info!("429 request has no system prompt, nothing to bisect");
        return Ok(());
    };
    let model = blocked.model;
    let auths = cpa.antigravity_auths().await?;
    if auths.is_empty() {
        bail!("no antigravity credentials found");
    }

    // User prompt is always "hi". If even that is refused with no system prompt, it is real quota.
    if cpa.probe(&auths, &model, &request(None)).await? == Probe::Blocked {
        info!("bare 'hi' is blocked too: genuine quota/account problem, not a prompt trigger");
        return Ok(());
    }
    if cpa.probe(&auths, &model, &request(Some(&original))).await? == Probe::Pass {
        info!("full system + 'hi' passes: the 429 was not caused by the system prompt");
        return Ok(());
    }

    let mut lines: Vec<String> = original.split('\n').map(str::to_string).collect();
    let mut rules: BTreeMap<String, String> = BTreeMap::new();

    for round in 1..=MAX_ROUNDS {
        let probe_model = model.clone();
        let blocking = minimal_blocking(lines.clone(), |subset| {
            let body = request(Some(&subset.join("\n")));
            let (auths, model) = (&auths, &probe_model);
            async move { Ok(cpa.probe(auths, model, &body).await? == Probe::Pass) }
        })
        .await?;
        let trigger = blocking[0].clone();
        info!(round, lines = blocking.len(), %trigger, "found blocking line");
        // Gemini (with Antigravity's own prompt and tools) proposes a rewording; a candidate
        // is kept only if the blocking lines pass with it. Otherwise the line is deleted.
        let mut replacement = String::new();
        if let Some(agy) = &opts.agy {
            let mut rejected = Vec::new();
            for _ in 0..3 {
                let candidate = match agy.propose(cpa, &auths, &trigger, &rejected).await {
                    Ok(c) => c,
                    Err(e) => {
                        warn!("rewrite generation failed: {e:#}");
                        break;
                    }
                };
                let mut subset = blocking.clone();
                subset[0] = candidate.clone();
                let ok = blocking.len() > 1
                    || cpa.probe(&auths, &model, &request(Some(&subset.join("\n")))).await? == Probe::Pass;
                if ok {
                    replacement = candidate;
                    break;
                }
                rejected.push(candidate);
            }
            if replacement.is_empty() {
                warn!(%trigger, "no accepted rewrite, deleting the line instead");
            }
        }
        // The replace rule rewrites every occurrence of the line text.
        lines.iter_mut().filter(|l| **l == trigger).for_each(|l| l.clone_from(&replacement));
        rules.insert(trigger, replacement);

        if cpa.probe(&auths, &model, &request(Some(&lines.join("\n")))).await? == Probe::Pass {
            return apply(cpa, &auths, &model, &original, &rules, opts).await;
        }
    }
    bail!("still blocked after {MAX_ROUNDS} rounds; rules found so far: {rules:?}")
}

async fn apply(
    cpa: &Cpa,
    auths: &[String],
    model: &str,
    original: &str,
    rules: &BTreeMap<String, String>,
    opts: &Options,
) -> Result<()> {
    if opts.dry_run {
        info!(?rules, "dry run: not patching CPA");
        return Ok(());
    }
    cpa.add_replace_rules(rules).await?;
    info!(?rules, "replace rules written, CPA hot-reloads the config");
    tokio::time::sleep(Duration::from_secs(3)).await;

    // The real check: the ORIGINAL system prompt through CPA, which now rewrites it.
    if cpa.probe(auths, model, &request(Some(original))).await? == Probe::Pass {
        info!("verified: original system prompt now returns 200");
        return Ok(());
    }
    let Some(cmd) = &opts.restart_cmd else {
        bail!("hot reload did not clear the 429 and no SENTINEL_RESTART_CMD is set");
    };
    warn!(%cmd, "hot reload did not clear the 429, restarting CPA");
    let status = tokio::process::Command::new("sh").arg("-c").arg(cmd).status().await?;
    if !status.success() {
        bail!("restart command failed: {status}");
    }
    tokio::time::sleep(Duration::from_secs(8)).await;
    if cpa.probe(auths, model, &request(Some(original))).await? == Probe::Pass {
        info!("verified after restart: original system prompt returns 200");
        return Ok(());
    }
    bail!("still 429 after restart; replace rule did not take effect")
}
