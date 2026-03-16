# 快速开始

本文档按当前代码实现说明真实启动流程：配置生成、注册文件生成、Synapse 接入和运行行为。

## 前置条件

| 要求 | 说明 |
|------|------|
| Rust 1.88+ | 仅源码运行时需要 |
| Synapse | 任意支持 appservice 的 Matrix homeserver |
| SQLite | 内置，无需额外数据库服务 |
| Docker / Compose | 可选，仅在容器化部署时需要 |

## 启动行为

程序读取以下环境变量：

| 变量 | 默认值 | 用途 |
|------|--------|------|
| `BRIDGE_CONFIG` | `config.toml` | 主配置文件 |
| `BRIDGE_REGISTRATION` | `registration.yaml` | 生成的 appservice 注册文件 |
| `RUST_LOG` | 回退到 `logging.level` | 运行时日志过滤器 |

启动时会依次执行：

1. 如果 `BRIDGE_CONFIG` 不存在，自动写出带随机 token 的默认配置并退出。
2. 解析并校验配置。
3. 打开 SQLite 并执行 migration。
4. 如果 `registration.yaml` 缺失、过期，或显式使用 `--generate-registration`，则按当前配置重新生成。
5. 在 homeserver 上注册 bridge bot。
6. 在 `appservice.address:appservice.port` 启动 HTTP 服务。

在 Unix 下，自动生成的配置文件权限会设置为 `0600`。

## 最小配置

```toml
[homeserver]
url = "http://matrix:8008"
domain = "example.com"

[appservice]
id = "matrix-bridge"
sender_localpart = "bridge_bot"
as_token = "CHANGE_ME_AS_TOKEN"
hs_token = "CHANGE_ME_HS_TOKEN"

[database]
path = "/data/bridge.db"

[logging]
level = "info"

[encryption]
allow = false
default = false
appservice = true
crypto_store = "/data/crypto"

[permissions]
admin = ["@admin:example.com"]
relay = []
relay_min_power_level = 0
```

## 配置说明

### `[homeserver]`

| 字段 | 必填 | 说明 |
|------|------|------|
| `url` | 是 | bridge 访问 Synapse 的基础 URL |
| `domain` | 是 | Matrix homeserver 域名，用于生成 MXID |

### `[appservice]`

| 字段 | 默认值 | 说明 |
|------|--------|------|
| `id` | 无 | appservice 标识，同时用于注册文件 |
| `address` | `0.0.0.0` | HTTP 服务监听地址 |
| `port` | `29320` | HTTP 服务监听端口 |
| `sender_localpart` | 无 | bridge bot 用户 localpart |
| `as_token` | 无 | Synapse 调用 bridge 时使用的 token |
| `hs_token` | 无 | bridge 与 Synapse appservice 路由交互使用的 token |
| `puppet_prefix` | `bot` | puppet 用户 MXID 前缀 |
| `api_key` | 未设置 | Bridge API 认证密钥，仅支持请求头认证 |
| `webhook_ssrf_protection` | `false` | 拒绝指向 localhost / 私网 / 保留地址的 webhook URL |
| `auto_invite` | `[]` | bridge 自动建房时默认邀请的 Matrix 用户 |
| `allow_api_invite` | `false` | 是否允许 `POST /api/v1/rooms` 中的 `invite` 生效 |
| `allow_relay` | `false` | 是否允许一个外部平台的消息继续转发到另一个外部平台 |

### `[database]`

| 字段 | 必填 | 说明 |
|------|------|------|
| `path` | 是 | SQLite 文件路径 |

### `[logging]`

| 字段 | 默认值 | 说明 |
|------|--------|------|
| `level` | `info` | `trace`、`debug`、`info`、`warn`、`error` 之一 |

### `[encryption]`

| 字段 | 默认值 | 说明 |
|------|--------|------|
| `allow` | `false` | 是否启用端到桥加密支持 |
| `default` | `false` | bridge 自动创建的房间是否默认启用加密 |
| `appservice` | `true` | 是否使用 appservice 模式处理加密 |
| `crypto_store` | `/data/crypto` | 加密状态持久化目录 |
| `crypto_store_passphrase` | 未设置 | crypto store 加密口令 |
| `device_display_name` | `Matrix Bridge` | bridge bot 设备显示名 |
| `device_id` | `matrix_bridge` | bridge bot 设备 ID |
| `per_user_crypto` | `false` | 是否给每个 puppet 分配独立加密设备 |
| `puppet_device_prefix` | `puppet` | per-user 模式下设备 ID 前缀 |

### `[permissions]`

| 字段 | 默认值 | 说明 |
|------|--------|------|
| `admin` | `[]` | 完整权限：可邀请 bot、可用 DM 命令 |
| `relay` | `[]` | 较低权限层级；DM 命令与邀请仍然只允许 admin |
| `relay_min_power_level` | `0` | 普通房间消息转发所需的最小 Matrix power level |

`admin` 和 `relay` 支持的模式：

- 精确用户：`@alice:example.com`
- 域通配：`@*:example.com`
- 全局通配：`*`

如果两个列表都为空，则视为开放模式，所有人都被当作 admin。

### `[platforms]`

bridge 核心会接受任意平台自定义 TOML 子树：

```toml
[platforms.telegram]
bot_username = "my_bot"

[platforms.slack]
workspace = "example"
```

这些值会进入配置模型，并用于声明“已配置的平台”，具体语义由你的接入层自己解释。

## 生成注册文件

显式生成 appservice 注册文件：

```bash
BRIDGE_CONFIG=/data/config.toml \
BRIDGE_REGISTRATION=/data/registration.yaml \
cargo run -- --generate-registration
```

生成内容包括：

- bridge bot namespace
- 基于 `appservice.puppet_prefix` 的 puppet 用户 namespace
- 启用加密时所需的 MSC 字段

正常启动时也会校验 `registration.yaml` 是否仍与当前 `as_token`、`hs_token` 和加密模式一致；不一致会直接报错退出。

## Synapse 配置

将生成的注册文件加入 `homeserver.yaml`：

```yaml
app_service_config_files:
  - /data/registration.yaml
```

然后重启 Synapse。

## 源码运行

```bash
# 首次启动：若配置不存在会自动写出并退出
cargo run

# 显式生成注册文件
cargo run -- --generate-registration

# 正常运行
cargo run --release
```

常用开发命令：

```bash
just fmt
just test
just check
```

## 容器部署示例

仓库当前没有内置可直接运行的 `docker-compose.yml`，下面是与当前运行方式一致的最小示例：

```yaml
services:
  bridge:
    build: .
    restart: unless-stopped
    environment:
      BRIDGE_CONFIG: /data/config.toml
      BRIDGE_REGISTRATION: /data/registration.yaml
      RUST_LOG: info
    volumes:
      - ./data:/data
    ports:
      - "29320:29320"
```

你仍然需要把生成的 `registration.yaml` 挂载给 Synapse，并在 `homeserver.yaml` 中引用它。

## 首次联通性检查

```bash
curl http://localhost:29320/health

curl http://localhost:29320/api/v1/admin/info \
  -H "Authorization: Bearer <api_key>"
```

如果未设置 `appservice.api_key`，第二个请求可以去掉认证头。
