use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;

pub struct Cpa {
    http: reqwest::Client,
    base: String,
    management_key: String,
    api_key: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Probe {
    Pass,
    Blocked,
}

pub const USER_AGENT: &str = "cpa-sentinel/0.1";

impl Cpa {
    pub fn new(base: &str, management_key: String, api_key: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(180))
            .build()?;
        Ok(Self { http, base: base.trim_end_matches('/').to_string(), management_key, api_key })
    }

    fn mgmt(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}/v8/management{path}", self.base))
            .bearer_auth(&self.management_key)
    }

    /// `auth_index` of every Antigravity credential.
    pub async fn antigravity_auths(&self) -> Result<Vec<String>> {
        let v: Value = self
            .mgmt(reqwest::Method::GET, "/credentials")
            .send()
            .await?
            .error_for_status()
            .context("list credentials")?
            .json()
            .await?;
        let files = v["files"].as_array().context("credentials: no files array")?;
        Ok(files
            .iter()
            .filter(|f| f["provider"] == "antigravity")
            .filter_map(|f| f["auth_index"].as_str().map(str::to_string))
            .collect())
    }

    pub async fn reset_cooldowns(&self, auths: &[String]) -> Result<()> {
        for a in auths {
            self.mgmt(reqwest::Method::POST, "/routing/cooldown/reset")
                .json(&json!({ "auth_index": a }))
                .send()
                .await?
                .error_for_status()
                .with_context(|| format!("reset cooldown {a}"))?;
        }
        Ok(())
    }

    /// Plain Gemini `generateContent` call through CPA; returns the response JSON.
    pub async fn generate(&self, auths: &[String], model: &str, body: &Value) -> Result<Value> {
        self.reset_cooldowns(auths).await?;
        let resp = self
            .http
            .post(format!("{}/v1beta/models/{model}:generateContent", self.base))
            .bearer_auth(&self.api_key)
            .json(body)
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            bail!("generate got HTTP {status}: {}", text.chars().take(200).collect::<String>());
        }
        Ok(serde_json::from_str(&text)?)
    }

    /// Send one Gemini `generateContent` request through CPA after clearing local cooldowns,
    /// so a 429 can only come from upstream.
    pub async fn probe(&self, auths: &[String], model: &str, body: &Value) -> Result<Probe> {
        for _ in 0..3 {
            self.reset_cooldowns(auths).await?;
            let resp = self
                .http
                .post(format!("{}/v1beta/models/{model}:generateContent", self.base))
                .bearer_auth(&self.api_key)
                .json(body)
                .send()
                .await?;
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            match status {
                200 => return Ok(Probe::Pass),
                // CPA's own "all credentials cooling down": reset raced with a retry, try again.
                429 if text.contains("model_cooldown") || text.contains("cooling down") => {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                429 => return Ok(Probe::Blocked),
                s => bail!("probe got HTTP {s}: {}", text.chars().take(200).collect::<String>()),
            }
        }
        bail!("probe kept hitting local cooldown after reset")
    }

    /// Merge literal replace rules into `oauth.providers.antigravity.sensitive-words-replace`.
    /// CPA hot-reloads the saved file; no restart.
    pub async fn add_replace_rules(&self, rules: &BTreeMap<String, String>) -> Result<()> {
        self.mgmt(reqwest::Method::PATCH, "/config/oauth/providers/antigravity")
            .json(&json!({ "sensitive-words-replace": rules }))
            .send()
            .await?
            .error_for_status()
            .context("patch sensitive-words-replace")?;
        Ok(())
    }
}
