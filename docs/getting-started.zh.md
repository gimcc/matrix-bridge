# 快速上手

本文档将引导你从零开始部署 Matrix Bridge，涵盖环境准备、配置说明、注册对接以及部署方式。

---

## 环境要求

| 依赖 | 最低版本 | 备注 |
|------|---------|------|
| Rust | 1.85+ | 仅源码编译时需要 |
| Synapse | 1.149+ | Matrix 服务端 |
| Docker | 24+ | 可选，推荐生产环境使用 |
| Docker Compose | 2.20+ | 可选，用于一体化部署 |

---

## 配置文件详解

Bridge 启动时会读取一个 TOML 配置文件（默认路径 `config.toml`，可通过 `BRIDGE_CONFIG` 环境变量覆盖）。

以下是完整的字段说明。

```toml
# ─── 主服务器 ─────────────────────────────────────────────────
[homeserver]
# Bridge 访问 Synapse 的内部地址。
# 在 Docker Compose 中通常填写容器名。
url = "http://matrix:8008"

# Synapse 实例的 server_name，
# 必须与 Synapse homeserver.yaml 中的设置一致。
domain = "im.fr.ds.cc"

# ─── 应用服务标识 ─────────────────────────────────────────────
[appservice]
# 本 appservice 的唯一标识符，必须与注册到 Synapse
# 的 registration.yaml 中的 id 保持一致。
id = "unified-bridge"

# Bridge 监听地址。容器内建议使用 0.0.0.0，
# 确保 Synapse 能够访问到。
address = "0.0.0.0"

# 监听端口，需要与 Dockerfile / Compose 中暴露的端口一致。
port = 29320

# Bridge 机器人的用户名（localpart）。
# 完整 MXID 为 @bridge_bot:<domain>。
sender_localpart = "bridge_bot"

# Appservice Token —— Synapse 用此令牌向 Bridge 发起请求。
# 请生成一个随机字符串。
as_token = "CHANGE_ME_AS_TOKEN"

# Homeserver Token —— Bridge 用此令牌向 Synapse 发起请求。
# 请生成一个随机字符串。
hs_token = "CHANGE_ME_HS_TOKEN"

# ─── 数据库 ───────────────────────────────────────────────────
[database]
# SQLite 数据库文件路径。
# 所在目录必须对 Bridge 进程可写。
path = "/data/bridge.db"

# ─── 日志 ─────────────────────────────────────────────────────
[logging]
# 日志级别：trace、debug、info、warn、error。
# 运行时可通过 RUST_LOG 环境变量覆盖。
level = "info"

# ─── 端到端加密 ───────────────────────────────────────────────
[encryption]
# 是否允许 Bridge 加入已加密的房间。
allow = true

# 是否在新建房间时默认开启加密。
default = true

# 启用 appservice 模式加密（推荐）。
appservice = true

# Olm/Megolm 会话数据的持久化目录。
crypto_store = "/data/crypto"

# 加密存储的保护口令，用于静态加密密钥材料。
# 请使用强随机字符串，妥善保管。
crypto_store_passphrase = "CHANGE_ME_CRYPTO_PASSPHRASE"

# Bridge 加密设备的显示名称。
device_display_name = "Matrix Bridge"

# ─── 访问控制 ───────────────────────────────────────────────
[permissions]
# 允许使用桥接器的 Matrix 用户白名单。
# 控制谁可以邀请 bot/傀儡用户，以及谁的消息
# 会被转发到外部平台。
#
# 支持的匹配模式：
#   "@admin:example.com"  — 精确匹配用户
#   "@*:example.com"      — 匹配该域名下所有用户
#   "*"                   — 所有人（等同于空列表）
#
# 空列表（默认）= 开放模式，允许所有人。
invite_whitelist = ["@*:example.com"]
```

> **安全提示：** 运行前务必替换所有 `CHANGE_ME_*` 占位符。可使用 `openssl rand -hex 32` 等工具生成令牌和口令。

---

## 应用服务注册

Synapse 需要一个注册文件来识别 Bridge。创建 `registration.yaml`：

```yaml
id: unified-bridge
url: "http://bridge:29320"        # Synapse 必须能访问此地址
as_token: "CHANGE_ME_AS_TOKEN"    # 必须与 config.toml 一致
hs_token: "CHANGE_ME_HS_TOKEN"    # 必须与 config.toml 一致
sender_localpart: bridge_bot
namespaces:
  users:
    - exclusive: true
      regex: "@bot_.*:.*"          # 傀儡用户命名空间
    - exclusive: true
      regex: "@bridge_bot:.*"     # Bridge 机器人本身
rate_limited: false
```

### 在 Synapse 中注册

1. 将 `registration.yaml` 放到 Synapse 可读取的路径（如 `/data/registration.yaml`）。
2. 在 Synapse 的 `homeserver.yaml` 中添加：

   ```yaml
   app_service_config_files:
     - /data/registration.yaml
   ```

3. 重启 Synapse 使配置生效。

---

## 环境变量

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `BRIDGE_CONFIG` | `config.toml` | 配置文件路径 |
| `BRIDGE_REGISTRATION` | `registration.yaml` | 应用服务注册文件路径 |
| `RUST_LOG` | _(使用配置文件中的 `logging.level`)_ | 运行时覆盖日志级别（如 `debug`、`matrix_bridge=trace`） |

---

## Docker Compose 部署

以下 `docker-compose.yaml` 可同时运行 Synapse 和 Bridge：

```yaml
services:
  # ── Synapse ──────────────────────────────────────────────
  matrix:
    image: matrixdotorg/synapse:latest
    restart: unless-stopped
    volumes:
      - synapse_data:/data
      - ./registration.yaml:/data/registration.yaml:ro
    environment:
      SYNAPSE_SERVER_NAME: im.fr.ds.cc
      SYNAPSE_REPORT_STATS: "no"
    ports:
      - "8008:8008"

  # ── Bridge ───────────────────────────────────────────────
  bridge:
    build: .
    restart: unless-stopped
    depends_on:
      - matrix
    volumes:
      - bridge_data:/data
      - ./config.toml:/data/config.toml:ro
      - ./registration.yaml:/data/registration.yaml:ro
    environment:
      BRIDGE_CONFIG: /data/config.toml
      BRIDGE_REGISTRATION: /data/registration.yaml
      RUST_LOG: info
    ports:
      - "29320:29320"

volumes:
  synapse_data:
  bridge_data:
```

### 快速启动

```bash
# 1. 生成令牌
export AS_TOKEN=$(openssl rand -hex 32)
export HS_TOKEN=$(openssl rand -hex 32)
export CRYPTO_PASS=$(openssl rand -hex 32)

# 2. 用上面生成的令牌填写 config.toml 和 registration.yaml
#    （替换所有 CHANGE_ME_* 占位符）

# 3. 启动服务
docker compose up -d

# 4. 查看日志
docker compose logs -f bridge
```

---

## 源码编译

```bash
# 克隆仓库
git clone <repo-url> matrix-bridge
cd matrix-bridge

# Release 模式编译（需要 Rust 1.85+）
cargo build --release

# 编译产物位于：
#   target/release/matrix-bridge
```

### 直接运行

```bash
export BRIDGE_CONFIG=config.toml
export BRIDGE_REGISTRATION=registration.yaml

./target/release/matrix-bridge
```

---

## 首次启动流程

Bridge 首次启动时，会自动完成以下步骤：

1. **加载配置** -- 读取 `BRIDGE_CONFIG` 指定的配置文件，校验所有字段。
2. **加载注册信息** -- 读取 `BRIDGE_REGISTRATION`，确认令牌与配置一致。
3. **创建数据库** -- 若 `database.path` 指定的 SQLite 文件不存在，则自动创建并执行数据库迁移。
4. **初始化加密存储** -- 当加密功能开启时，在 `encryption.crypto_store` 路径下创建 Olm/Megolm 密钥存储，并使用配置的口令加密。
5. **注册机器人用户** -- 通过 appservice API 在服务端注册 `@bridge_bot:<domain>`（幂等操作，重复启动不会冲突）。
6. **启动 HTTP 监听** -- 在配置的地址和端口上开始接收请求。

### 验证是否正常运行

```bash
# Bridge 应响应健康检查
curl http://localhost:29320/health

# 机器人用户应已存在于服务端
curl "http://localhost:8008/_matrix/client/v3/profile/@bridge_bot:im.fr.ds.cc/displayname"
```

如果启动失败，请检查：

- `config.toml` 和 `registration.yaml` 中的令牌是否一致。
- Synapse 在配置的 `homeserver.url` 上是否可达。
- `/data` 目录是否具有写权限。
- 注册文件是否已添加到 Synapse 的 `homeserver.yaml` 中。
