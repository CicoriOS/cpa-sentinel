use serde_json::Value;

/// A Gemini-format request that CPA answered with an upstream 429.
#[derive(Debug)]
pub struct Blocked429 {
    pub model: String,
    pub body: Value,
}

/// Parse a CPA `error-*.log`. Returns `None` for anything that is not an Antigravity-style
/// Gemini 429 (`RESOURCE_EXHAUSTED`) or that was sent by this tool itself.
pub fn parse(text: &str, own_user_agent: &str) -> Option<Blocked429> {
    let (head, rest) = text.split_once("=== REQUEST BODY ===\n")?;
    let (body, tail) = rest.split_once("\n\n=== ")?;

    if head.lines().any(|l| l.starts_with("User-Agent:") && l.contains(own_user_agent)) {
        return None;
    }
    if !tail.contains("Status: 429") || !tail.contains("RESOURCE_EXHAUSTED") {
        return None;
    }

    let url = head.lines().find_map(|l| l.strip_prefix("URL: "))?;
    let model = url.split("/models/").nth(1)?.split([':', '?']).next()?.to_string();
    let body: Value = serde_json::from_str(body.trim()).ok()?;
    Some(Blocked429 { model, body })
}

/// System prompt text of a Gemini request (`systemInstruction` or `system_instruction`).
pub fn system_text(body: &Value) -> Option<String> {
    let si = body.get("systemInstruction").or_else(|| body.get("system_instruction"))?;
    let parts = si.get("parts")?.as_array()?;
    let text: String = parts.iter().filter_map(|p| p["text"].as_str()).collect();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(ua: &str, status: &str) -> String {
        format!(
            "=== REQUEST INFO ===\nURL: /v1beta/models/gemini-3.8-flash:streamGenerateContent?alt=sse\nMethod: POST\n\
             === HEADERS ===\nUser-Agent: {ua}\n\n=== REQUEST BODY ===\n\
             {{\"systemInstruction\":{{\"parts\":[{{\"text\":\"a\\nb\"}}]}},\"contents\":[]}}\n\n\
             === API RESPONSE ===\n{{\"error\":{{\"status\":\"RESOURCE_EXHAUSTED\"}}}}\n\n=== RESPONSE ===\nStatus: {status}\n"
        )
    }

    #[test]
    fn parses_upstream_429() {
        let b = parse(&log("Go-http-client/1.1", "429"), "cpa-sentinel").unwrap();
        assert_eq!(b.model, "gemini-3.8-flash");
        assert_eq!(system_text(&b.body).unwrap(), "a\nb");
    }

    #[test]
    fn ignores_own_and_non_429() {
        assert!(parse(&log("cpa-sentinel/0.1", "429"), "cpa-sentinel").is_none());
        assert!(parse(&log("Go-http-client/1.1", "500"), "cpa-sentinel").is_none());
    }
}
