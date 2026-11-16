# cpa-sentinel

[English](README.md) | 简体中文

[CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI)(CPA)的轻量守护程序。它用来处理 **Antigravity** 渠道上的误报 `429 RESOURCE_EXHAUSTED`:这类 429 由系统提示词中的某些文字触发,与配额无关。

目前只支持 Google(Gemini)请求格式。

agent 不必直接删除触发行,也可以让同一个 CPA 上的 `gemini-3.8-flash` 改写它。这次调用会带上 Antigravity 自己的系统提示词和工具声明,并从 Antigravity 编辑工具的调用参数里取出新文本。只有当改写后的内容能让被拦截的行通过探测,才会采用;否则该行会被删除。

## 工作流程

1. 监听 CPA 的 `error-v1beta-models-*.log`,发现 Gemini 请求被上游返回 429。
2. 从日志里记录的请求体中取出系统提示词。
3. 先重置 CPA 本地冷却,再确认只发一句 `hi` 是否能通过。如果不能通过,说明是真实的配额耗尽,agent 直接停止。
4. 用户提示词固定为 `hi`,对系统提示词按行二分(delta debugging),找出触发拦截的那一行。
5. 重复上一步,直到整段系统提示词都能通过。然后把每条触发行对应的规则合并写入 `oauth.providers.antigravity.sensitive-words-replace`,调用 `PATCH /v8/management/config/oauth/providers/antigravity`。CPA 会热加载配置。
6. 验证:把原始系统提示词再通过 CPA 发送一次。只有仍然返回 429 且设置了 `SENTINEL_RESTART_CMD` 时,才会重启 CPA。

每次探测之间会调用 `POST /v8/management/routing/cooldown/reset`,所以探测遇到的 429 一定来自上游,二分过程也不用等待本地冷却结束。

## 配置

| 变量 | 默认值 | 说明 |
|---|---|---|
| `CPA_MANAGEMENT_KEY` | 必填 | CPA 管理密钥,明文 |
| `CPA_API_KEY` | 必填 | CPA `access.api-keys` 里的一个客户端 key,用于发探测请求 |
| `CPA_URL` | `http://127.0.0.1:8318` | CPA 地址 |
| `CPA_LOG_DIR` | `/opt/cliproxyapi/logs` | CPA 写入 `error-*.log` 的目录 |
| `SENTINEL_RESTART_CMD` | 未设置 | 兜底重启命令,例如 `systemctl restart cliproxyapi` |
| `SENTINEL_DRY_RUN` | 未设置 | 设为 `1` 时只查找触发行,不修改 CPA |
| `SENTINEL_AGY_SYSTEM_FILE` | 未设置 | 存放 Antigravity 系统提示词的文件;设置后启用 Gemini 改写(否则直接删除触发行) |
| `SENTINEL_AGY_TOOLS_FILE` | 上一项启用时必填 | Antigravity 工具声明的 JSON `tools` 数组,Gemini 格式 |
| `SENTINEL_AGY_EDIT_TOOL` | 上一项启用时必填 | 要强制调用的 Antigravity 编辑工具名 |
| `SENTINEL_AGY_EDIT_FIELD` | 上一项启用时必填 | 该工具中承载新文本的参数名 |
| `SENTINEL_AGY_MODEL` | `gemini-3.8-flash` | 用于改写的模型 |

`CPA_URL` 必须是明文 `http`(程序编译时不带 TLS,以便发布成静态 musl 二进制)。CPA 必须开启请求错误日志,这样才会生成 `error-*.log`;agent 必须和 CPA 运行在同一台机器上(或共享日志目录)。

## Antigravity 提示词与工具

`scripts/fetch-agy.py <目录>` 会从公开合集 [x1xhlol/system-prompts-and-models-of-ai-tools](https://github.com/x1xhlol/system-prompts-and-models-of-ai-tools) 下载 Antigravity 的系统提示词和工具声明,并写出 `<目录>/system.txt` 和 `<目录>/tools.json`(Gemini `functionDeclarations` 格式,共 22 个工具)。脚本支持 `HTTPS_PROXY`。这些文件不放在本仓库里。

```sh
python3 scripts/fetch-agy.py agy
export SENTINEL_AGY_SYSTEM_FILE=agy/system.txt SENTINEL_AGY_TOOLS_FILE=agy/tools.json \
       SENTINEL_AGY_EDIT_TOOL=replace_file_content SENTINEL_AGY_EDIT_FIELD=ReplacementContent
```

## 运行

```sh
cargo build --release
CPA_MANAGEMENT_KEY=... CPA_API_KEY=sk-... ./target/release/cpa-sentinel
```

如果目标机器的 glibc 较旧,可以改用静态构建:

```sh
cargo build --release --target x86_64-unknown-linux-musl
```

## 注意事项

- 替换规则会改变模型实际看到的提示词(删除或改写触发行)。所有规则都会记录在日志和 CPA 配置里,可以随时审查。
- 已经处理过的系统提示词会在 30 分钟内被跳过。
- 二分每一轮都要等探测通过或被拦截,单次被拦截的探测约需 30 秒。一个提示词里触发行越多,整体越慢。

## 许可证

仅限 AGPL-3.0。见 [LICENSE](LICENSE)。
