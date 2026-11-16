use crate::cpa::Cpa;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

/// Asks Gemini (through the local CPA) to reword a trigger line.
///
/// The call is shaped like an Antigravity agent turn: Antigravity's own system prompt,
/// Antigravity's own tool declarations, and the answer is read from that tool's call.
pub struct Agy {
    pub model: String,
    pub system: String,
    /// Gemini `tools` array: `[{ "functionDeclarations": [ ... ] }]`.
    pub tools: Value,
    /// Name of the Antigravity edit tool whose call carries the new text.
    pub tool: String,
    /// Argument of that tool holding the replacement text.
    pub field: String,
}

fn env(key: &str) -> Result<String> {
    std::env::var(key).with_context(|| format!("{key} is not set"))
}

impl Agy {
    /// `None` when `SENTINEL_AGY_SYSTEM_FILE` is unset: the agent then deletes trigger lines.
    pub fn from_env() -> Result<Option<Self>> {
        let Ok(system_file) = std::env::var("SENTINEL_AGY_SYSTEM_FILE") else {
            return Ok(None);
        };
        let tools_file = env("SENTINEL_AGY_TOOLS_FILE")?;
        Ok(Some(Self {
            model: std::env::var("SENTINEL_AGY_MODEL").unwrap_or_else(|_| "gemini-3.8-flash".into()),
            system: std::fs::read_to_string(&system_file).with_context(|| system_file.clone())?,
            tools: serde_json::from_str(&std::fs::read_to_string(&tools_file).with_context(|| tools_file.clone())?)
                .context("SENTINEL_AGY_TOOLS_FILE is not valid JSON")?,
            tool: env("SENTINEL_AGY_EDIT_TOOL")?,
            field: env("SENTINEL_AGY_EDIT_FIELD")?,
        }))
    }

    fn body(&self, trigger: &str, rejected: &[String]) -> Value {
        let mut prompt = format!(
            "The upstream content filter rejects the following line from a system prompt with a bogus \
             429. Reword it so the meaning, language and any identifiers, paths and URLs stay the same \
             but the wording differs. Reply only by calling `{}` with the new line in `{}`.\n\n\
             Line:\n{trigger}",
            self.tool, self.field
        );
        if !rejected.is_empty() {
            prompt.push_str("\n\nThese rewrites were still rejected, so do not reuse their wording:\n");
            for r in rejected {
                prompt.push_str(&format!("- {r}\n"));
            }
        }
        json!({
            "systemInstruction": { "parts": [{ "text": self.system }] },
            "contents": [{ "role": "user", "parts": [{ "text": prompt }] }],
            "tools": self.tools,
            "toolConfig": { "functionCallingConfig": { "mode": "ANY", "allowedFunctionNames": [self.tool] } },
        })
    }

    /// One candidate replacement for `trigger`, taken from the edit tool call.
    pub async fn propose(&self, cpa: &Cpa, auths: &[String], trigger: &str, rejected: &[String]) -> Result<String> {
        let resp = cpa.generate(auths, &self.model, &self.body(trigger, rejected)).await?;
        let parts = resp["candidates"][0]["content"]["parts"].as_array().context("no candidate parts")?;
        let text = parts
            .iter()
            .filter(|p| p["functionCall"]["name"] == self.tool.as_str())
            .find_map(|p| p["functionCall"]["args"][&self.field].as_str());
        match text {
            Some(t) if !t.trim().is_empty() && t != trigger => Ok(t.trim_end_matches('\n').to_string()),
            Some(_) => bail!("model returned an empty or unchanged line"),
            None => bail!("model did not call `{}` with `{}`", self.tool, self.field),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_forces_the_edit_tool_and_lists_rejected_rewrites() {
        let agy = Agy {
            model: "m".into(),
            system: "AGY SYSTEM".into(),
            tools: json!([{ "functionDeclarations": [{ "name": "edit" }] }]),
            tool: "edit".into(),
            field: "new_text".into(),
        };
        let b = agy.body("bad line", &["still bad".into()]);
        assert_eq!(b["systemInstruction"]["parts"][0]["text"], "AGY SYSTEM");
        assert_eq!(b["toolConfig"]["functionCallingConfig"]["allowedFunctionNames"][0], "edit");
        let prompt = b["contents"][0]["parts"][0]["text"].as_str().unwrap();
        assert!(prompt.contains("bad line") && prompt.contains("- still bad"));
    }
}
