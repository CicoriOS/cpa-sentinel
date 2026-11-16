use anyhow::Result;

/// Delta-debugging minimisation over `lines`.
///
/// `passes(subset)` must return `true` when the upstream accepts a request
/// carrying exactly `subset` as the system prompt, `false` when it blocks it.
/// The full `lines` slice must be blocking. Returns a 1-minimal blocking subset:
/// removing any single remaining line makes the request pass.
pub async fn minimal_blocking<F, Fut>(lines: Vec<String>, mut passes: F) -> Result<Vec<String>>
where
    F: FnMut(Vec<String>) -> Fut,
    Fut: Future<Output = Result<bool>>,
{
    let mut cur = lines;
    let mut chunks = 2usize;
    while cur.len() > 1 {
        let size = cur.len().div_ceil(chunks);
        let parts: Vec<Vec<String>> = cur.chunks(size).map(<[String]>::to_vec).collect();

        let mut reduced = false;
        for part in &parts {
            if !passes(part.clone()).await? {
                cur = part.clone();
                chunks = 2;
                reduced = true;
                break;
            }
        }
        if reduced {
            continue;
        }

        if parts.len() > 2 {
            for skip in 0..parts.len() {
                let comp: Vec<String> = parts
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != skip)
                    .flat_map(|(_, p)| p.iter().cloned())
                    .collect();
                if !passes(comp.clone()).await? {
                    cur = comp;
                    chunks = (chunks - 1).max(2);
                    reduced = true;
                    break;
                }
            }
            if reduced {
                continue;
            }
        }

        if chunks >= cur.len() {
            break;
        }
        chunks = (chunks * 2).min(cur.len());
    }
    Ok(cur)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("line {i}")).collect()
    }

    async fn run(lines: Vec<String>, triggers: &[&str]) -> Vec<String> {
        minimal_blocking(lines, |subset| {
            let blocked = triggers.iter().all(|t| subset.iter().any(|l| l == t));
            async move { Ok(!blocked) }
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn finds_single_trigger() {
        for at in [0, 1, 137, 418] {
            let mut lines = doc(419);
            lines[at] = "BAD".into();
            assert_eq!(run(lines, &["BAD"]).await, vec!["BAD".to_string()]);
        }
    }

    #[tokio::test]
    async fn finds_jointly_blocking_pair() {
        let mut lines = doc(200);
        lines[10] = "A".into();
        lines[150] = "B".into();
        let mut got = run(lines, &["A", "B"]).await;
        got.sort();
        assert_eq!(got, vec!["A".to_string(), "B".to_string()]);
    }
}
