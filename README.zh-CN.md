# cpa-sentinel

[English](README.md) | 简体中文

[CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI)(CPA)的轻量守护程序。它用来处理 **Antigravity** 渠道上的误报 `429 RESOURCE_EXHAUSTED`:这类 429 由系统提示词中的某些文字触发,与配额无关。

目前只支持 Google(Gemini)请求格式。

agent 不必直接删除触发行,也可以让另一个 OpenAI 兼容的聊天 API 改写它。上游过滤器连引用了触发行的请求也会拦,所以不能把改写交给正在修复的这个 CPA。改写不必忠于原文。每次尝试随机选一种风格,各占一半概率:

- **换成其他语言**:由模型从乌兹别克语、日语、繁体中文、简体中文、韩语、缅甸语、法语、德语中选一种,用它写这一行。
- **更长的一句话**:比原句更长,且完全不使用原句和此前被拒绝的改写里出现过的词。agent 会自己校验,不合格的候选直接丢弃。

只有当改写后的内容能让被拦截的行通过探测,才会采用。连续 3 次尝试失败后,该行会被删除。触发行会被发送给这个 API,请使用你自己可控的服务。

## 工作流程

1. 监听 CPA 的 `error-v1beta-models-*.log`,发现 Gemini 请求被上游返回 429。
2. 从日志里记录的请求体中取出系统提示词。
3. 先重置 CPA 本地冷却,再确认只发一句 `hi` 是否能通过。如果不能通过,说明是真实的配额耗尽,agent 直接停止。
4. 用户提示词固定为 `hi`,对系统提示词按行二分(delta debugging),找出触发拦截的那一行。
5. 重复上一步,直到整段系统提示词都能通过。然后把每条触发行对应的规则(改写后的文本,或用空字符串表示删除)合并写入 `oauth.providers.antigravity.sensitive-words-replace`,调用 `PATCH /v8/management/config/oauth/providers/antigravity`。CPA 会热加载配置。
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
| `SENTINEL_REWRITE_URL` | 未设置 | OpenAI 兼容 API 的基础地址,例如 `http://host:3000/v1`;设置后启用改写(否则直接删除触发行) |
| `SENTINEL_REWRITE_KEY` | 上一项启用时必填 | 该 API 的密钥 |
| `SENTINEL_REWRITE_MODEL` | 上一项启用时必填 | 用于改写的模型 |

`CPA_URL` 和 `SENTINEL_REWRITE_URL` 必须是明文 `http`(程序编译时不带 TLS,以便发布成静态 musl 二进制)。CPA 必须开启请求错误日志,这样才会生成 `error-*.log`;agent 必须和 CPA 运行在同一台机器上(或共享日志目录)。

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
