use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::Duration;
use tracing::info;

/// Languages the model may choose from when asked to rewrite a line in another language.
const LANGUAGES: &str =
    "Uzbek, Japanese, Traditional Chinese, Simplified Chinese, Korean, Burmese, French, German";

const SYSTEM: &str = "You rewrite one line taken from an AI assistant's system prompt. \
    Reply with the new line only, on a single line: no quotes, no explanation, no markdown.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Style {
    /// The line in another language, picked by the model from `LANGUAGES`.
    OtherLanguage,
    /// A longer sentence that reuses no word seen so far.
    LongerFresh,
}

/// Asks an OpenAI-compatible chat API to rewrite a trigger line.
///
/// The API is a separate service, not the CPA being repaired: the upstream filter also blocks
/// any request that quotes a trigger line, so the rewrite cannot go through the same channel.
pub struct Rewriter {
    http: reqwest::Client,
    url: String,
    key: String,
    model: String,
}

impl Rewriter {
    /// `None` when `SENTINEL_REWRITE_URL` is unset: the agent then deletes trigger lines.
    pub fn from_env() -> Result<Option<Self>> {
        let Ok(base) = std::env::var("SENTINEL_REWRITE_URL") else {
            return Ok(None);
        };
        Ok(Some(Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(90))
                .build()?,
            url: format!("{}/chat/completions", base.trim_end_matches('/')),
            key: std::env::var("SENTINEL_REWRITE_KEY")
                .context("SENTINEL_REWRITE_KEY is not set")?,
            model: std::env::var("SENTINEL_REWRITE_MODEL")
                .context("SENTINEL_REWRITE_MODEL is not set")?,
        }))
    }

    fn body(&self, style: Style, trigger: &str, rejected: &[String]) -> Value {
        let task = match style {
            Style::OtherLanguage => format!(
                "Rewrite the line in another language: pick exactly one of {LANGUAGES} and write the \
                 line in it. It does not have to be faithful to the original."
            ),
            Style::LongerFresh => "Replace the line with one longer sentence on the same general topic. It \
                 does not have to be faithful, but it must be longer than the original and must not \
                 reuse any word that appears in the original line or in the rewrites listed below."
                .to_string(),
        };
        let mut prompt = format!("{task}\n\nLine:\n{trigger}");
        if !rejected.is_empty() {
            prompt.push_str(
                "\n\nThese rewrites were still rejected, so do not reuse their wording:\n",
            );
            for r in rejected {
                prompt.push_str(&format!("- {r}\n"));
            }
        }
        json!({
            "model": self.model,
            "temperature": 1.0,
            "messages": [
                { "role": "system", "content": SYSTEM },
                { "role": "user", "content": prompt },
            ],
        })
    }

    /// One candidate replacement for `trigger`. The style is drawn at random on every call:
    /// half the time another language, half the time a longer sentence that reuses none of the
    /// words seen so far. The rules are enforced here as well, not only asked for.
    pub async fn propose(&self, trigger: &str, rejected: &[String]) -> Result<String> {
        let style = if RandomState::new().build_hasher().finish() & 1 == 0 {
            Style::OtherLanguage
        } else {
            Style::LongerFresh
        };
        info!(?style, "asking the rewrite model");
        let resp = self
            .http
            .post(&self.url)
            .bearer_auth(&self.key)
            .json(&self.body(style, trigger, rejected))
            .send()
            .await
            .context("rewrite API request failed")?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            bail!(
                "rewrite API returned HTTP {status}: {}",
                text.chars().take(200).collect::<String>()
            );
        }
        let v: Value = serde_json::from_str(&text).context("rewrite API returned invalid JSON")?;
        let candidate = v["choices"][0]["message"]["content"]
            .as_str()
            .context("rewrite API returned no content")?
            .trim();
        check(style, trigger, rejected, candidate)?;
        Ok(candidate.to_string())
    }
}

/// Lowercased alphanumeric runs. CJK text has no spaces, so a whole run is one word.
fn words(s: &str) -> impl Iterator<Item = String> + '_ {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
}

/// Rejects candidates that break the rules of their style.
fn check(style: Style, trigger: &str, rejected: &[String], candidate: &str) -> Result<()> {
    if candidate.is_empty() || candidate == trigger {
        bail!("model returned an empty or unchanged line");
    }
    if candidate.contains('\n') {
        bail!("rewrite is not a single line");
    }
    if style == Style::LongerFresh {
        if candidate.chars().count() <= trigger.chars().count() {
            bail!("rewrite is not longer than the original line");
        }
        let seen: HashSet<String> = words(trigger)
            .chain(rejected.iter().flat_map(|r| words(r)))
            .collect();
        if let Some(w) = words(candidate).find(|w| seen.contains(w)) {
            bail!("rewrite reuses the word {w:?}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewriter() -> Rewriter {
        Rewriter {
            http: reqwest::Client::new(),
            url: "http://x/chat/completions".into(),
            key: "k".into(),
            model: "m".into(),
        }
    }

    #[test]
    fn body_quotes_the_line_and_lists_rejected_rewrites() {
        let b = rewriter().body(Style::OtherLanguage, "bad line", &["still bad".into()]);
        assert_eq!(b["model"], "m");
        assert_eq!(b["messages"][0]["role"], "system");
        let prompt = b["messages"][1]["content"].as_str().unwrap();
        assert!(prompt.contains("bad line") && prompt.contains("- still bad"));
        assert!(
            prompt.contains("Uzbek")
                && prompt.contains("Burmese")
                && prompt.contains("Traditional Chinese")
        );
        let fresh = rewriter().body(Style::LongerFresh, "bad line", &[]);
        assert!(
            fresh["messages"][1]["content"]
                .as_str()
                .unwrap()
                .contains("must not")
        );
    }

    #[test]
    fn longer_fresh_must_be_longer_and_share_no_word_with_earlier_text() {
        let rejected = vec!["Plain boring text".to_string()];
        let ok = |c| check(Style::LongerFresh, "Bad line here", &rejected, c).is_ok();
        assert!(ok("Quiet rivers carry small boats toward distant harbors"));
        assert!(!ok("Short"), "not longer");
        assert!(
            !ok("A very much longer sentence with LINE inside it"),
            "reuses a word, case-insensitively"
        );
        assert!(
            !ok("Enormous sentence that mentions boring things"),
            "reuses a rejected rewrite's word"
        );
        assert!(
            check(
                Style::OtherLanguage,
                "Bad line here",
                &[],
                "Schlechte Zeile"
            )
            .is_ok()
        );
        assert!(check(Style::OtherLanguage, "Bad line here", &[], "Bad line here").is_err());
        assert!(check(Style::OtherLanguage, "Bad line here", &[], "Zwei\nZeilen").is_err());
    }
}
