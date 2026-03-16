# Matrix Bridge 架构文档

## 系统概览

Matrix Bridge 是一个 Rust 编写的应用服务（Application Service），用于将 Matrix 房间与外部消息平台（Telegram、Slack、Discord 等）桥接。核心机制包括傀儡用户（puppet user）、基于 Webhook 和 WebSocket 的消息投递以及可选的端到桥加密（E2BE）。

```
                          +-------------------------------------------+
                          |              matrix-bridge (bin)           |
                          |                  src/main.rs               |
                          +--------+----------+-----------+-----------+
                                   |          |           |
            +----------------------+    +-----+-----+    +------------------+
            |   matrix-bridge-core |    |  appservice |    | matrix-bridge-   |
            |     crates/core      |    | crates/     |    |   store          |
            |                      |    | appservice  |    | crates/store     |
            +----------------------+    +-------------+    +------------------+
```

### Crate 结构

| Crate | 路径 | 职责 |
|-------|------|------|
| `matrix-bridge-core` | `crates/core` | 共享类型：`BridgeMessage`、`MessageContent`、`ExternalUser`、`AppConfig`、错误类型、注册文件生成、平台 ID 净化与傀儡 localpart 生成 |
| `matrix-bridge-store` | `crates/store` | SQLite 数据库层：`Database`、迁移脚本、room_mappings / message_mappings / puppets / webhooks 的增删改查 |
| `matrix-bridge-appservice` | `crates/appservice` | 应用服务运行时：HTTP 服务器（axum）、Dispatcher（拆分为多模块）、PuppetManager、MatrixClient、CryptoManagerPool、WsRegistry、Bridge HTTP API、认证中间件、速率限制 |
| `matrix-bridge`（二进制） | `src/main.rs` | 入口：加载配置、打开数据库、初始化所有组件、启动 HTTP 服务器 |


### 核心组件

```
+------------------+     +------------------+     +--------------------+
|   MatrixClient   |     |  PuppetManager   |     | CryptoManagerPool  |
|                  |     |                  |     |                    |
| Synapse CS API   |     | 首次使用时创建/  |     | 管理多个           |
| 的 HTTP 客户端。 |     | 更新傀儡用户。   |     | CryptoManager 实例 |
| as_token 认证，  |     | DashMap 内存缓存 |     | （bot + puppet）。 |
| MSC3202 设备     |     | + 数据库持久化。 |     | 基于 OlmMachine   |
| 伪装。           |     |                  |     | 的 E2BE 加密。     |
+------------------+     +------------------+     +--------------------+
         |                        |                        |
         +------------+-----------+------------------------+
                      |
              +-------v--------+          +------------------+
              |   Dispatcher   |          |    WsRegistry    |
              |                |          |                  |
              | 在 Matrix 与   |--------->| 管理 WebSocket   |
              | 外部平台之间   |          | 连接。按平台和   |
              | 路由事件。     |          | forward_sources  |
              | 访问控制、     |          | 分发消息。       |
              | 跨平台转发 +   |          |                  |
              | 回环防护。     |          +------------------+
              +-------+--------+
                      |
              +-------v--------+
              |    Database    |
              |                |
              | SQLite (WAL)。 |
              | room_mappings, |
              | message_map,   |
              | puppets,       |
              | webhooks.      |
              +----------------+
```

### Dispatcher 模块结构

`Dispatcher` 已拆分为多个子模块（位于 `crates/appservice/src/dispatcher/`）：

| 模块 | 文件 | 职责 |
|------|------|------|
| `mod` | `mod.rs` | `Dispatcher` 结构体定义、构造函数、共享字段 |
| `platform_events` | `platform_events.rs` | 入站处理：外部平台 -> Matrix（消息发送、傀儡管理） |
| `matrix_events` | `matrix_events.rs` | 出站处理：Matrix 事务事件分发与路由 |
| `outbound` | `outbound.rs` | Webhook 与 WebSocket 投递逻辑 |
| `commands` | `commands.rs` | 房间内命令（`!bridge link/unlink/status`）处理 |
| `matrix_content` | `matrix_content.rs` | Matrix 事件内容解析与转换 |
| `crypto_helpers` | `crypto_helpers.rs` | E2BE 加密/解密辅助函数 |

---

## 消息流

### 入站：外部平台 -> Matrix

外部服务通过 Bridge HTTP API 发送消息。

```
外部服务                        Bridge                              Synapse
     |                               |                                    |
     |  POST /api/v1/message         |                                    |
     |  {platform, room_id,          |                                    |
     |   sender, content}            |                                    |
     |------------------------------>|                                    |
     |                               |                                    |
     |                   bridge_api::handle_send_message                  |
     |                               |                                    |
     |                   Dispatcher::handle_incoming_http                 |
     |                               |                                    |
     |                     1. 净化 sender.id 和 room_id                  |
     |                     2. DB: find_room_by_external_id                |
     |                     3. PuppetManager::ensure_puppet_direct         |
     |                               |                                    |
     |                               |  POST /register (新傀儡用户)      |
     |                               |----------------------------------->|
     |                               |                                    |
     |                               |  PUT /profile/.../displayname     |
     |                               |----------------------------------->|
     |                               |                                    |
     |                     4. MatrixClient::join_room                     |
     |                               |  POST /join/{room_id}             |
     |                               |----------------------------------->|
     |                               |                                    |
     |                     5. Dispatcher::send_to_matrix                  |
     |                               |  PUT /rooms/{room}/send/...       |
     |                               |----------------------------------->|
     |                               |                                    |
     |                     6. DB: create_message_mapping                  |
     |                               |                                    |
     |  {event_id, message_id}       |                                    |
     |<------------------------------|                                    |
```

傀儡用户（如 `@bot_telegram_user123:domain`）会出现在 Matrix 房间中，看起来就像外部用户直接发送了消息。

### 出站：Matrix -> 外部平台（Webhook + WebSocket）

当 Matrix 用户在桥接房间中发送消息时，Synapse 通过事务端点将事件投递给应用服务。

```
Matrix 客户端       Synapse                  Bridge                  外部服务
     |                    |                       |                          |
     |  在 !room:domain   |                       |                          |
     |  中发送消息        |                       |                          |
     |------------------->|                       |                          |
     |                    |                       |                          |
     |                    |  PUT /transactions/N  |                          |
     |                    |  {events: [...]}      |                          |
     |                    |---------------------->|                          |
     |                    |                       |                          |
     |                    |         verify_hs_token (Bearer 或查询参数)      |
     |                    |                       |                          |
     |                    |         Dispatcher::handle_transaction           |
     |                    |         -> handle_event -> handle_room_message   |
     |                    |                       |                          |
     |                    |         1. 检查：发送者是 bridge_bot？跳过       |
     |                    |         2. 检查：邀请白名单（第零层）            |
     |                    |            傀儡用户绕过，其他必须匹配            |
     |                    |         3. 检查：发送者是傀儡用户？提取          |
     |                    |            source_platform 用于回环防护          |
     |                    |         4. 检查：allow_relay 是否启用            |
     |                    |            （未启用时跳过傀儡用户消息）          |
     |                    |         5. DB: find_all_mappings_by_matrix_id    |
     |                    |         6. 对每个映射：                          |
     |                    |            - 如果 mapping.platform == source     |
     |                    |              则跳过                              |
     |                    |            - 否则投递到 Webhook + WebSocket      |
     |                    |                       |                          |
     |                    |                       |  POST webhook_url       |
     |                    |                       |  {event, platform,      |
     |                    |                       |   source_platform,      |
     |                    |                       |   message: {...}}       |
     |                    |                       |------------------------->|
     |                    |                       |                          |
     |                    |                       |  WebSocket 推送          |
     |                    |                       |  (同一载荷格式)         |
     |                    |                       |------------------------->|
     |                    |                       |                          |
     |                    |         7. DB: create_message_mapping            |
     |                    |                       |                          |
     |                    |  200 OK {}            |                          |
     |                    |<----------------------|                          |
```

---

## 跨平台转发

这是桥接器的核心功能。一个 Matrix 房间可以同时桥接到多个外部平台，消息通过 Matrix 作为中枢在所有平台之间流转。

> **前提条件：** 需要配置 `allow_relay = true` 才能启用跨平台转发。当 `allow_relay = false`（默认）时，仅转发真实 Matrix 用户的消息。

### 场景示例

房间 `!room:domain` 同时链接到 Telegram (`chat_123`) 和 Slack (`C456`)。

Telegram 用户 "Alice"（ID `user123`）发送了一条消息：

```
1. Telegram 机器人收到消息
2. POST /api/v1/message 到桥接器
   {platform: "telegram", sender: {id: "user123", display_name: "Alice"}, ...}

3. 桥接器创建/复用傀儡用户 @bot_telegram_user123:domain
4. 傀儡用户在 !room:domain 中发送消息
5. Synapse 通过 /transactions 将事件回传给桥接器

6. Dispatcher 收到来自 @bot_telegram_user123:domain 的事件
7. puppet_source_platform("@bot_telegram_user123:domain") => Some("telegram")
8. 检查 allow_relay = true（否则跳过傀儡用户消息）
9. DB 返回映射：[{platform: "telegram", ...}, {platform: "slack", ...}]

10. 跳过：platform "telegram" == source "telegram"（回环防护）
11. 转发：platform "slack" != source "telegram"（跨平台投递）

12. 发送到 Slack 服务的 Webhook/WebSocket 载荷：
    {
      "event": "message",
      "platform": "slack",
      "source_platform": "telegram",
      "message": {
        "sender": {
          "platform": "telegram",
          "external_id": "user123",
          "display_name": "Alice"
        },
        "content": { "type": "text", "body": "Hello from Telegram!" },
        ...
      }
    }
```

关键实现细节：

- **来源平台检测**：`platform::puppet_source_platform()` 解析傀儡用户的 Matrix 用户 ID（`@{prefix}_{platform}_{userid}:domain`）以提取发起平台。
- **原始发送者保留**：跨平台转发时，桥接器从数据库中查询傀儡用户的原始 `platform`、`external_id` 和 `display_name`；载荷携带的是真实身份，而非 Matrix 傀儡 ID。
- **消息映射**：`UNIQUE(matrix_event_id, platform_id)` 约束允许同一个 Matrix 事件同时映射到多个平台。
- **双通道投递**：出站消息同时通过 Webhook（HTTP POST）和 WebSocket（实时推送）两个通道投递，由 `Dispatcher::outbound` 模块统一处理。

### 流程概览

```
    Telegram                    Matrix 房间                      Slack
       |                       !room:domain                        |
       |                            |                              |
  Alice 发送     傀儡用户           |                              |
  "Hello"  ----> @bot_telegram_     |                              |
       |         user123 发送 ----->|                              |
       |                            |----> Dispatcher              |
       |                            |      source = "telegram"     |
       |      跳过（回环）<--------|                              |
       |                            |-----> webhook 到 Slack ----->|
       |                            |-----> ws 推送到 Slack ------>|
       |                            |       (原始发送者信息)       |
       |                            |                              |
       |                            |<---- @bot_slack_bob456 <-----|
       |                            |      发送（来自 Slack Bob）  |
       |  webhook 到 Telegram <-----|                              |
       |  ws 推送到 Telegram <------|                              |
       |  (原始发送者：Bob)         |-----> 跳过（回环）          |
```

---

## 访问控制（邀请白名单）

桥接器通过可配置的白名单控制谁能与桥接机器人和傀儡用户交互。该功能在 `PermissionsConfig`（`crates/core/src/config.rs`）中实现，由 `Dispatcher` 负责执行检查。

### 配置

```toml
[permissions]
invite_whitelist = ["@*:example.com"]
```

### 匹配模式

| 模式 | 匹配范围 |
|------|----------|
| _（空列表）_ | 所有人（开放模式，默认） |
| `"*"` | 所有人（显式通配符） |
| `"@admin:example.com"` | 仅精确匹配该用户 |
| `"@*:example.com"` | 该域名下的所有用户 |

可组合多个模式：

```toml
invite_whitelist = ["@admin:a.com", "@*:b.com"]
# @admin:a.com  -> 允许
# @other:a.com  -> 拒绝
# @anyone:b.com -> 允许
```

### 三个执行检查点

白名单在 Dispatcher 的三个关键位置被检查：

```
                        邀请事件                       消息事件
                             |                               |
                    +--------v--------+             +--------v--------+
                    | 目标是 bot 还是 |             | 发送者是        |
                    | 傀儡用户？      |             | 傀儡用户？      |
                    +--+----------+---+             +--+----------+---+
                    否 |          | 是              是 |          | 否
                       |          v                     |          v
                    忽略    +------------+          绕过    +------------+
                            | 邀请者是   |                  | 发送者在   |
                            | bridge_bot?|                  | 白名单中？ |
                            +--+------+--+                  +--+------+--+
                            是 |      | 否                  是 |      | 否
                               v      v                       v      v
                            接受    检查                    转发    阻止
                                    白名单
```

**检查点 1：Bot 邀请** -- 当有人邀请 `@bridge_bot:domain` 进入房间时，邀请者必须在白名单中。

**检查点 2：傀儡用户邀请** -- 当有人邀请傀儡用户（如 `@bot_telegram_123:domain`）时，邀请者必须在白名单中。桥接机器人自身始终绕过此检查（它作为正常操作的一部分邀请傀儡用户）。

**检查点 3：消息转发** -- 当 Matrix 用户在桥接房间中发送消息时，只有发送者在白名单中，消息才会被转发到外部平台的 Webhook 和 WebSocket。傀儡用户绕过此检查，因为它们中继的是来自已授权外部平台的消息。

### 安全意义

如果没有白名单，任何 Matrix 用户都可以：
- 将桥接机器人邀请到任意房间并桥接到外部平台
- 直接邀请傀儡用户，绕过正常桥接流程
- 通过桥接房间向外部平台发送消息

白名单确保只有授权用户（例如本服务器上的用户）才能使用桥接器。

### 实现位置

- `PermissionsConfig::is_invite_allowed()` -- `crates/core/src/config.rs`（模式匹配逻辑）
- `Dispatcher::handle_membership()` -- `crates/appservice/src/dispatcher/matrix_events.rs`（邀请执行）
- `Dispatcher::handle_room_message()` -- `crates/appservice/src/dispatcher/matrix_events.rs`（转发执行）

---

## 四层过滤机制

桥接器使用四个互补机制来控制消息流转。第零层（访问控制）决定 _谁_ 可以使用桥接器。第一层控制是否允许跨平台转发。第二层和第三层决定消息 _投递到哪里_。

### 第零层：访问控制（邀请白名单）

参见上方的[访问控制](#访问控制邀请白名单)章节。这是对邀请和消息转发应用的第一道检查。

### 第一层：`allow_relay` 开关

全局控制。当 `allow_relay = false`（默认）时，来自傀儡用户（即跨平台消息）的出站事件会被直接跳过，仅转发真实 Matrix 用户的消息。

```toml
[appservice]
allow_relay = false   # 默认：禁止跨平台转发
```

当 `allow_relay = true` 时，傀儡用户的消息会进入后续的回环防护和 `forward_sources` 过滤流程。

### 第二层：内置回环防护

自动生效。当 Dispatcher 处理来自傀儡用户的出站事件时，它从傀儡用户的 Matrix 用户 ID 中提取来源平台，并跳过向同一平台的转发。

```
puppet_source_platform("@bot_telegram_user123:domain")  =>  Some("telegram")

对每个映射：
    如果 mapping.platform_id == source_platform：
        跳过   // 防止 Telegram -> Matrix -> Telegram 回环
    否则：
        转发   // 投递到其他平台
```

此机制始终生效，无法禁用。

### 第三层：按 Webhook/WebSocket 的 `forward_sources` 白名单过滤

可配置。每个 Webhook 和 WebSocket 连接指定允许转发的来源平台：

- **空**（默认）= 仅转发 Matrix 用户消息。
- `"*"` = 转发所有来源。
- `"telegram,matrix"` = 仅转发指定平台。

```
POST /api/v1/webhooks
{
  "platform": "slack",
  "url": "https://slack-bot.example.com/webhook",
  "forward_sources": ["telegram", "matrix"]
}
```

在此示例中，Slack Webhook 将接收来自 Telegram 和原生 Matrix 用户的消息，但不接收来自 Discord 的消息。

检查逻辑在 `Dispatcher` 的 `outbound` 模块中：

```
对每个 webhook / ws 连接：
    如果 NOT should_forward_source(source_platform)：
        跳过
    否则：
        POST 到 webhook.url / 推送到 WebSocket
```

### 过滤流程示例

```
来自 @bot_telegram_user123:domain 的消息，房间桥接到 Slack + Discord：

第零层（访问控制）：
  - @bot_telegram_user123 是傀儡用户 -> 绕过白名单检查

第一层（allow_relay）：
  - allow_relay = true -> 允许继续

第二层（回环防护）：
  - telegram 映射：跳过（source == telegram）
  - slack 映射：  通过
  - discord 映射：通过

第三层（每个 webhook/ws 的 forward_sources）：
  - Slack webhook (forward_sources="*")：投递
  - Discord webhook (forward_sources="matrix")：跳过（telegram 不在白名单中）

结果：消息仅投递到 Slack
```

```
来自 @alice:example.com（不在白名单中）在桥接房间中的消息：

第零层（访问控制）：
  - @alice:example.com 不在 invite_whitelist 中 -> 阻止

结果：消息不会转发到任何 Webhook 或 WebSocket
```

---

## 端到桥加密（E2BE）

桥接器支持端到桥加密，采用 mautrix 方案。消息在 Matrix 客户端和桥接器之间加密，在桥接器处解密后再转发到外部平台。

### 架构

```
Matrix 客户端 A                桥接器 Bot                   外部平台
     |                            |                               |
     |  Olm 密钥交换              |                               |
     |  (to-device 事件)          |                               |
     |<-------------------------->|                               |
     |                            |                               |
     |  Megolm 加密消息           |                               |
     |  m.room.encrypted          |                               |
     |--------------------------->|                               |
     |                     CryptoManagerPool                      |
     |                     .decrypt()                             |
     |                            |                               |
     |                     明文消息                               |
     |                            |                               |
     |                     转发到 webhook/ws  ------------------>|
     |                            |                               |
     |                     来自平台的入站消息  <------------------|
     |                            |                               |
     |                     CryptoManagerPool                      |
     |                     .encrypt()                             |
     |                            |                               |
     |  m.room.encrypted          |                               |
     |<---------------------------|                               |
```

### 关键 MSC

| MSC | 用途 | 实现方式 |
|-----|------|----------|
| MSC2409 | 通过应用服务事务传递 to-device 事件 | 事务载荷中的 `de.sorunome.msc2409.to_device` 字段，由 `CryptoManagerPool` 处理 |
| MSC3202 | 应用服务的设备列表变更和 OTK 计数 | 事务中的 `de.sorunome.msc3202.device_lists`、`device_one_time_keys_count`、`device_unused_fallback_key_types` |
| MSC3202 | 设备伪装 | 通过 `MatrixClient::e2ee_query_params()` 在 E2EE API 调用中附加 `user_id` + `device_id` 查询参数 |

### 实现细节

- **CryptoManagerPool**：管理多个 `CryptoManager` 实例的池子。bot 拥有一个专属实例，每个启用加密的傀儡用户也可拥有独立实例。
- **持久化加密存储**：Olm/Megolm 密钥存储在 SQLite 加密存储（`matrix-sdk-sqlite`）中，路径由 `crypto_store` 配置，必须设置加密口令。
- **密钥管理**：启动时上传设备密钥和一次性密钥。`process_outgoing_requests()` 方法处理密钥上传、查询、认领和 to-device 发送。
- **房间加密状态追踪**：当收到 `m.room.encryption` 状态事件时，房间在 `OlmMachine` 的房间设置中被标记为已加密。
- **自动启用**：当 `encryption.default = true` 时，通过 `!bridge link` 链接房间时自动发送 `m.room.encryption` 状态事件。
- **认证**：需要 Synapse 1.149+ 以支持应用服务请求的 `Authorization: Bearer` 头部（用于所有密钥管理端点）。

---

## 傀儡用户管理

傀儡（ghost）用户在 Matrix 房间中代表外部平台用户。

### 命名规则

```
@{prefix}_{platform}_{external_user_id}:{server_name}
```

前缀可通过 `appservice.puppet_prefix` 配置（默认值：`"bot"`）。

示例：
- `@bot_telegram_user123:im.fr.ds.cc`
- `@bot_slack_U05ABC:im.fr.ds.cc`
- `@bot_discord_123456789:im.fr.ds.cc`

localpart 必须匹配 Matrix 规范要求的 `[a-z0-9._\-=/]+`。当外部用户 ID 包含不合法字符时，自动使用 SHA-256 哈希回退：`h_{hex(sha256[:16])}`（128-bit，32 位十六进制字符串）。

### 生命周期

```
1. 收到平台用户 "Alice" 的入站消息 (platform=telegram, id=user123)

2. 净化 sender.id（移除控制字符，必要时哈希回退）

3. PuppetManager::ensure_puppet_direct("bot_telegram_user123", ...)
   a. 检查 DashMap 内存缓存 -> 未命中
   b. DB: find_puppet_by_external_id("telegram", "user123") -> 未命中
   c. MatrixClient::register_puppet("bot_telegram_user123")
      POST /_matrix/client/v3/register {type: "m.login.application_service", username: "bot_telegram_user123"}
   d. MatrixClient::set_display_name("@bot_telegram_user123:domain", "Alice")
   e. MatrixClient::set_avatar("@bot_telegram_user123:domain", "mxc://...")
   f. DB: upsert_puppet(...)
   g. 缓存写入："telegram:user123" -> "@bot_telegram_user123:domain"

4. 后续消息：缓存命中，跳过注册。

5. 如果 display_name 或头像变更：通过 CS API + 数据库更新。
```

### 存储

傀儡用户存储在 `puppets` 表中，`(platform_id, external_user_id)` 有唯一约束，`matrix_user_id` 也有独立的唯一约束。

---

## 数据库 Schema

SQLite，启用 WAL 模式和外键。四张表，跨四次迁移。

### 表结构

#### `room_mappings`

将 Matrix 房间与外部平台房间关联。一个 Matrix 房间可以链接到多个平台（每个平台一条记录）。

```sql
CREATE TABLE room_mappings (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    matrix_room_id    TEXT NOT NULL,
    platform_id       TEXT NOT NULL,
    external_room_id  TEXT NOT NULL,
    created_at        TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(matrix_room_id, platform_id),
    UNIQUE(platform_id, external_room_id)
);
```

#### `message_mappings`

追踪 Matrix 事件与外部消息的对应关系。唯一约束为 `(matrix_event_id, platform_id)`，允许一个 Matrix 事件同时映射到多个平台（跨平台转发的必要条件）。

```sql
CREATE TABLE message_mappings (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    matrix_event_id       TEXT NOT NULL,
    platform_id           TEXT NOT NULL,
    external_message_id   TEXT NOT NULL,
    room_mapping_id       INTEGER NOT NULL REFERENCES room_mappings(id),
    created_at            TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(matrix_event_id, platform_id),
    UNIQUE(platform_id, external_message_id)
);
```

#### `puppets`

存储傀儡用户的身份映射和资料数据。

```sql
CREATE TABLE puppets (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    matrix_user_id    TEXT NOT NULL UNIQUE,
    platform_id       TEXT NOT NULL,
    external_user_id  TEXT NOT NULL,
    display_name      TEXT,
    avatar_mxc        TEXT,
    updated_at        TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(platform_id, external_user_id)
);
```

#### `webhooks`

注册的 Webhook 端点，用于接收出站消息（Matrix -> 外部）。

```sql
CREATE TABLE webhooks (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    platform_id      TEXT NOT NULL,
    webhook_url      TEXT NOT NULL,
    secret           TEXT,
    events           TEXT NOT NULL DEFAULT 'message',
    enabled          INTEGER NOT NULL DEFAULT 1,
    forward_sources  TEXT NOT NULL DEFAULT '',  -- 白名单: 空=仅Matrix用户, "*"=全部, "telegram,matrix"=指定平台
    created_at       TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(platform_id, webhook_url)
);
```

### 实体关系

```
room_mappings 1---* message_mappings
    |                    (通过 room_mapping_id 外键)
    |
    +-- UNIQUE(matrix_room_id, platform_id)
    +-- UNIQUE(platform_id, external_room_id)

puppets
    +-- UNIQUE(matrix_user_id)
    +-- UNIQUE(platform_id, external_user_id)

webhooks
    +-- UNIQUE(platform_id, webhook_url)

message_mappings
    +-- UNIQUE(matrix_event_id, platform_id)  -- 每个平台一条记录
    +-- UNIQUE(platform_id, external_message_id)
```

---

## HTTP 端点

### 应用服务端点（hs_token 认证）

| 方法 | 路径 | 用途 |
|------|------|------|
| PUT | `/_matrix/app/v1/transactions/{txnId}` | 接收 Synapse 事件（包括 MSC2409/3202 E2EE 数据） |
| GET | `/_matrix/app/v1/users/{userId}` | 用户存在性查询 |
| GET | `/_matrix/app/v1/rooms/{roomAlias}` | 房间别名查询 |

### Bridge API 端点（可选 `api_key` 认证，速率限制 100 次/分钟/IP）

当配置了 `appservice.api_key` 时，所有 `/api/v1/*` 请求必须通过 `Authorization: Bearer <api_key>` 或 `?access_token=<api_key>` 携带密钥。未配置时（默认），Bridge API 无需认证——适用于内部/可信网络部署。

**操作端点（写操作）：**

| 方法 | 路径 | 用途 |
|------|------|------|
| POST | `/api/v1/message` | 从外部平台向 Matrix 发送消息 |
| POST | `/api/v1/upload` | 上传媒体文件（最大 200 MB），返回 `mxc://` URI |
| POST | `/api/v1/rooms` | 创建房间映射 |
| DELETE | `/api/v1/rooms/{id}` | 删除房间映射 |
| POST | `/api/v1/webhooks` | 注册 Webhook |
| DELETE | `/api/v1/webhooks/{id}` | 删除 Webhook |

**管理端点（只读查询）：**

| 方法 | 路径 | 用途 |
|------|------|------|
| GET | `/api/v1/admin/info` | 服务器信息（版本、配置、统计） |
| GET | `/api/v1/admin/rooms?platform=X` | 列出房间映射（分页） |
| GET | `/api/v1/admin/webhooks?platform=X` | 列出 Webhook（分页） |
| GET | `/api/v1/admin/puppets?platform=X` | 列出傀儡用户（分页） |
| GET | `/api/v1/admin/messages?...` | 列出消息映射（分页） |
| GET | `/api/v1/admin/crypto` | 加密状态 |

**WebSocket 端点：**

| 方法 | 路径 | 用途 |
|------|------|------|
| GET | `/api/v1/ws?platform=X&forward_sources=*` | WebSocket 实时消息推送 |

**其他：**

| 方法 | 路径 | 用途 |
|------|------|------|
| GET | `/health` | 健康检查 |

### Webhook SSRF 防护

当 `appservice.webhook_ssrf_protection = true` 时，注册 Webhook URL 会拦截以下目标：
- RFC1918 私有地址（10.0.0.0/8、172.16.0.0/12、192.168.0.0/16）
- 回环地址（127.0.0.0/8、::1）、链路本地（169.254.0.0/16、fe80::/10）
- CGNAT（100.64.0.0/10）、IPv6 ULA（fc00::/7）
- 云元数据端点（169.254.169.254、metadata.google.internal）
- IPv4 映射的 IPv6 地址（::ffff:x.x.x.x）
- 解析到上述地址的域名（防止 DNS 重绑定攻击）

默认值为 `false`（允许私有 IP），适用于 Webhook 目标位于同一内网的内部部署场景。当桥接器暴露到不可信网络时应启用此选项。

### 房间内命令

| 命令 | 权限要求 | 操作 |
|------|----------|------|
| `!bridge link <platform> <external_room_id>` | 权限等级 >= 50 | 创建房间映射 |
| `!bridge unlink <platform>` | 权限等级 >= 50 | 删除房间映射 |
| `!bridge status` | 任意 | 显示已注册的平台 |
