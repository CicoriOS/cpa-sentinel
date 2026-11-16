# cpa-sentinel

English | [简体中文](README.zh-CN.md)

Lightweight agent for [CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI) (CPA). It fixes false-positive `429 RESOURCE_EXHAUSTED` responses on the **Antigravity** channel that are caused by text in the system prompt, not by quota.

Only the Google (Gemini) request format is supported.

Instead of deleting a trigger line, the agent can ask a separate OpenAI-compatible chat API to rewrite it. The upstream filter also blocks any request that quotes a trigger line, so the rewrite cannot be asked of the CPA that is being repaired. The rewrite does not have to stay faithful to the original. Each attempt picks one of two styles at random, half the time each:

- **Another language**: the model picks one of Uzbek, Japanese, Traditional Chinese, Simplified Chinese, Korean, Burmese, French or German and writes the line in it.
- **A longer sentence**: one sentence longer than the original that reuses no word from the original line or from earlier rejected rewrites. The agent checks this itself and discards candidates that break it.

A rewrite is used only if the blocking lines pass with it. After 3 failed attempts the line is deleted. The trigger lines are sent to that API, so use a service you control.

## What it does

1. Watches CPA's `error-v1beta-models-*.log` files for a Gemini request that got an upstream 429.
2. Pulls the system prompt out of the logged request body.
3. Resets CPA's local cooldowns, then checks that a bare `hi` passes. If it does not, the 429 is real quota and the agent stops.
4. Keeps the user prompt fixed at `hi` and bisects the system prompt line by line (delta debugging) to find the blocking line.
5. Repeats until the whole system prompt passes, then merges one rule per trigger line (its rewrite, or an empty string to delete it) into `oauth.providers.antigravity.sensitive-words-replace` with `PATCH /v8/management/config/oauth/providers/antigravity`. CPA hot-reloads the config.
6. Verifies by sending the original system prompt through CPA. Only if that still returns 429 and `SENTINEL_RESTART_CMD` is set does it restart CPA.

Between probes it calls `POST /v8/management/routing/cooldown/reset`, so a 429 seen by a probe always comes from upstream and the bisect does not wait out cooldowns.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `CPA_MANAGEMENT_KEY` | required | CPA management key, in plaintext |
| `CPA_API_KEY` | required | a client key from CPA `access.api-keys`, used for probes |
| `CPA_URL` | `http://127.0.0.1:8318` | CPA address |
| `CPA_LOG_DIR` | `/opt/cliproxyapi/logs` | where CPA writes `error-*.log` |
| `SENTINEL_RESTART_CMD` | unset | fallback command, e.g. `systemctl restart cliproxyapi` |
| `SENTINEL_DRY_RUN` | unset | `1` finds triggers but does not change CPA |
| `SENTINEL_REWRITE_URL` | unset | base URL of an OpenAI-compatible API, e.g. `http://host:3000/v1`; setting it turns on rewrites (otherwise lines are deleted) |
| `SENTINEL_REWRITE_KEY` | required with the above | API key for it |
| `SENTINEL_REWRITE_MODEL` | required with the above | model used for rewrites |

`CPA_URL` and `SENTINEL_REWRITE_URL` must be plain `http` (the agent is built without TLS so it ships as a static musl binary). CPA must have request error logging enabled so that `error-*.log` files are written, and the agent must run on the CPA host (or share the log directory).

## Run

```sh
cargo build --release
CPA_MANAGEMENT_KEY=... CPA_API_KEY=sk-... ./target/release/cpa-sentinel
```

## Notes

- A replace rule deletes the trigger line, which changes what the model sees. Rules are listed in the log and in CPA's config.
- A system prompt that was already investigated is skipped for 30 minutes.

## License

AGPL-3.0-only. See [LICENSE](LICENSE).
