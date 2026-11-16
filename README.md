# cpa-sentinel

Lightweight agent for [CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI) (CPA). It fixes false-positive `429 RESOURCE_EXHAUSTED` responses on the **Antigravity** channel that are caused by text in the system prompt, not by quota.

Only the Google (Gemini) request format is supported.

Instead of deleting a trigger line, the agent can ask `gemini-3.8-flash` on the same CPA to reword it. That call carries Antigravity's own system prompt and tool declarations and reads the new text from the Antigravity edit tool's call. A rewrite is used only if the blocking lines pass with it; otherwise the line is deleted.

## What it does

1. Watches CPA's `error-v1beta-models-*.log` files for a Gemini request that got an upstream 429.
2. Pulls the system prompt out of the logged request body.
3. Resets CPA's local cooldowns, then checks that a bare `hi` passes. If it does not, the 429 is real quota and the agent stops.
4. Keeps the user prompt fixed at `hi` and bisects the system prompt line by line (delta debugging) to find the blocking line.
5. Repeats until the whole system prompt passes, then merges one delete rule per trigger line into `oauth.providers.antigravity.sensitive-words-replace` with `PATCH /v8/management/config/oauth/providers/antigravity`. CPA hot-reloads the config.
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
| `SENTINEL_AGY_SYSTEM_FILE` | unset | file with Antigravity's system prompt; setting it turns on Gemini rewrites (otherwise lines are deleted) |
| `SENTINEL_AGY_TOOLS_FILE` | required with the above | JSON `tools` array with Antigravity's tool declarations, Gemini format |
| `SENTINEL_AGY_EDIT_TOOL` | required with the above | name of the Antigravity edit tool to force |
| `SENTINEL_AGY_EDIT_FIELD` | required with the above | that tool's argument that holds the new text |
| `SENTINEL_AGY_MODEL` | `gemini-3.8-flash` | model used for rewrites |

CPA must have request error logging enabled so that `error-*.log` files are written, and the agent must run on the CPA host (or share the log directory).

## Antigravity prompt and tools

`scripts/fetch-agy.py <dir>` downloads Antigravity's system prompt and tool declarations from the public [x1xhlol/system-prompts-and-models-of-ai-tools](https://github.com/x1xhlol/system-prompts-and-models-of-ai-tools) collection and writes `<dir>/system.txt` and `<dir>/tools.json` (22 tools in Gemini `functionDeclarations` format). It honours `HTTPS_PROXY`. The files are not stored in this repository.

```sh
python3 scripts/fetch-agy.py agy
export SENTINEL_AGY_SYSTEM_FILE=agy/system.txt SENTINEL_AGY_TOOLS_FILE=agy/tools.json \
       SENTINEL_AGY_EDIT_TOOL=replace_file_content SENTINEL_AGY_EDIT_FIELD=ReplacementContent
```

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
