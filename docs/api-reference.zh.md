# Matrix Bridge API 接口文档

基础地址：`http://<bridge_host>:<port>`（默认端口：**29320**）

除特别说明外，所有请求和响应均为 JSON 格式。

---

## 认证

Bridge API（`/api/v1/*`）支持可选的 API Key 认证，与 Matrix `hs_token` 独立配置。

| 配置字段 | 默认值 | 说明 |
|---|---|---|
| `appservice.api_key` | _（空）_ | 设置后，所有 `/api/v1/*` 请求需携带此密钥 |

配置了 `api_key` 后，每个请求需通过以下方式之一携带密钥：

```
Authorization: Bearer <api_key>
```

或作为查询参数：

```
GET /api/v1/admin/rooms?platform=myapp&access_token=<api_key>
```

未配置 `api_key` 时（默认），Bridge API 不需要认证。适用于内部/可信网络部署，由网络层（防火墙、反向代理等）控制访问。

> **注意：** `api_key` 与 `hs_token` 完全独立。`hs_token` 是 Matrix 协议密钥，仅用于 Synapse 与桥接器之间的 `/_matrix/app/v1/*` 路由。外部服务不应使用或知晓 `hs_token`。

---

## 速率限制

Bridge API 端点（`/api/v1/*`）启用了基于 IP 的速率限制（通过 tower_governor 实现）：

- **限额：** 每个 IP 每分钟 100 次请求
- 超过限额时返回 `429 Too Many Requests`

WebSocket 端点和 Matrix 应用服务端点（`/_matrix/app/v1/*`）不受此限制。

---

## 目录

- [认证](#认证)
- [速率限制](#速率限制)
- [健康检查](#健康检查)
- [服务器信息](#服务器信息)
- [加密状态](#加密状态)
- [发送入站消息](#发送入站消息)
- [上传媒体文件](#上传媒体文件)
- [房间映射管理](#房间映射管理)
- [Webhook 管理](#webhook-管理)
- [傀儡用户管理](#傀儡用户管理)
- [消息映射查询](#消息映射查询)
- [WebSocket 实时推送](#websocket-实时推送)
- [Webhook 回调格式](#webhook-回调格式出站)
- [SSRF 防护](#ssrf-防护)
- [消息内容类型](#消息内容类型)
- [傀儡用户命名规则](#傀儡用户命名规则)
- [输入净化](#输入净化)

---

## 健康检查

```
GET /health
```

**响应** `200`

```json
{
  "status": "ok"
}
```

---

## 服务器信息

```
GET /api/v1/admin/info
```

返回服务器配置、功能开关和运行时统计信息。

**响应 `200`**

```json
{
  "version": "0.1.0",
  "homeserver": {
    "url": "https://matrix.example.com",
    "domain": "example.com"
  },
  "bot": {
    "user_id": "@bridge_bot:example.com",
    "puppet_prefix": "bot"
  },
  "features": {
    "encryption_enabled": true,
    "encryption_default": true,
    "webhook_ssrf_protection": false,
    "api_key_required": true,
    "websocket_enabled": true
  },
  "permissions": {
    "invite_whitelist": ["@admin:example.com"]
  },
  "platforms": {
    "configured": ["telegram", "slack"],
    "active": ["telegram"]
  },
  "stats": {
    "room_mappings": 5,
    "webhooks": 3,
    "message_mappings": 1024,
    "puppets": 42,
    "ws_clients": 2
  }
}
```

| 字段 | 说明 |
|------|------|
| `features.websocket_enabled` | WebSocket 实时推送是否可用 |
| `platforms.configured` | 配置文件中定义的平台 |
| `platforms.active` | 至少有一个房间映射的平台 |
| `stats.ws_clients` | 当前活跃的 WebSocket 连接数 |
| `stats.*` | 数据库中的记录数 |

---

## 加密状态

```
GET /api/v1/admin/crypto
```

返回桥接机器人和所有已初始化傀儡用户的加密密钥状态。会向 homeserver 查询实际的设备密钥状态。

**响应 `200`（加密已启用）**

```json
{
  "enabled": true,
  "per_user_crypto": true,
  "bot": {
    "user_id": "@bridge_bot:example.com",
    "device_id": "BRIDGE_DEV",
    "has_master_key": true,
    "has_self_signing_key": true,
    "has_user_signing_key": true,
    "device_keys_uploaded": true,
    "device_keys": {
      "algorithms": ["m.olm.v1.curve25519-aes-sha2", "m.megolm.v1.aes-sha2"],
      "keys": {
        "curve25519:BRIDGE_DEV": "...",
        "ed25519:BRIDGE_DEV": "..."
      },
      "signatures": { "..." : { "..." : "..." } }
    }
  },
  "puppets": [
    {
      "user_id": "@telegram_user123:example.com",
      "device_id": "PUP_abc123",
      "has_master_key": true,
      "has_self_signing_key": true,
      "has_user_signing_key": true,
      "device_keys_uploaded": true,
      "device_keys": { "..." }
    }
  ]
}
```

**响应 `200`（加密未启用）**

```json
{
  "enabled": false,
  "per_user_crypto": false,
  "bot": null,
  "puppets": []
}
```

| 字段 | 说明 |
|------|------|
| `enabled` | 配置中是否启用了 E2EE |
| `per_user_crypto` | 是否启用 per-user 加密模式（每个傀儡用户独立 OlmMachine） |
| `bot` | 桥接机器人的加密状态 |
| `puppets` | 已初始化的傀儡用户加密状态数组 |
| `has_master_key` | 本地存储中存在交叉签名主密钥 |
| `has_self_signing_key` | 存在自签名密钥 |
| `has_user_signing_key` | 存在用户签名密钥 |
| `device_keys_uploaded` | 设备密钥是否已上传到 homeserver |
| `device_keys` | 从 homeserver 查询到的原始设备密钥（algorithms、identity keys、signatures） |

---

## 发送入站消息

```
POST /api/v1/message
```

将外部平台的消息桥接到 Matrix。Bridge 会自动为发送者创建傀儡用户（puppet user），并将消息投递到对应的 Matrix 房间。

`sender.id` 和 `room_id` 中的特殊字符和控制字符会被自动净化（不会拒绝请求）。详见[输入净化](#输入净化)章节。

### 请求参数

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `platform` | string | 是 | 平台标识，纯小写字母（`[a-z]+`），如 `telegram`、`slack` |
| `room_id` | string | 是 | 外部平台的房间 ID（必须已建立房间映射） |
| `sender` | object | 是 | 发送者信息（见下表） |
| `content` | object | 是 | 消息内容（见[消息内容类型](#消息内容类型)） |
| `external_message_id` | string | 否 | 去重标识；重复的 ID 会被自动忽略 |
| `reply_to` | string | 否 | 所回复消息的 `external_message_id` |

**发送者对象**

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `id` | string | 是 | 用户在外部平台上的 ID |
| `display_name` | string | 否 | 傀儡用户的显示名称 |
| `avatar_url` | string | 否 | 傀儡用户的头像地址 |

### 请求示例

```json
{
  "platform": "telegram",
  "room_id": "chat_12345",
  "sender": {
    "id": "user789",
    "display_name": "Alice",
    "avatar_url": "https://cdn.example.com/avatars/alice.jpg"
  },
  "content": {
    "type": "text",
    "body": "Hello!"
  },
  "external_message_id": "msg_001",
  "reply_to": "msg_000"
}
```

上述请求会创建傀儡用户 `@bot_telegram_user789:<homeserver_domain>`。

### 响应 `200`

```json
{
  "event_id": "$abc123...",
  "message_id": "01J..."
}
```

| 字段 | 说明 |
|------|------|
| `event_id` | Matrix 事件 ID |
| `message_id` | Bridge 内部消息 ID |

---

## 上传媒体文件

```
POST /api/v1/upload
```

将文件上传到 Matrix 内容仓库。返回的 `content_uri` 可用于消息内容中的 `url` 字段（如 image、file、video、audio 类型）。

**最大文件大小：200 MB。** 超过限制的请求会收到 `413 Payload Too Large` 响应。

### 请求

使用 multipart form-data 格式，字段名为 `file`。

```bash
curl -X POST http://localhost:29320/api/v1/upload \
  -F "file=@photo.jpg"
```

### 响应 `200`

```json
{
  "content_uri": "mxc://example.com/abc123",
  "filename": "photo.jpg",
  "size": 12345
}
```

---

## 房间映射管理

房间映射将外部平台的房间与 Matrix 房间关联起来。只有建立了映射关系的房间，消息才会被桥接。

### 创建映射

```
POST /api/v1/rooms
```

幂等操作：如果 `(platform, external_room_id)` 的映射已存在，返回现有映射（`200`）。否则创建新映射（`201`）。不提供 `matrix_room_id` 时，Bridge 会自动创建 Matrix 房间。

自动创建房间时，`auto_invite` 配置中指定的用户会被自动邀请加入。

**请求参数**

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `platform` | string | 是 | 平台标识 |
| `external_room_id` | string | 是 | 外部平台的房间 ID |
| `matrix_room_id` | string | 否 | 指定 Matrix 房间 ID；不传则自动创建 |
| `room_name` | string | 否 | 自动创建时的房间名称（最长 255 字符；提供 `matrix_room_id` 时忽略） |
| `invite` | array | 否 | 自动创建时额外邀请的 Matrix 用户 ID（最多 50 个；需配置 `allow_api_invite = true`） |

**请求示例（指定房间）**

```json
{
  "platform": "telegram",
  "external_room_id": "chat_123",
  "matrix_room_id": "!abc:example.com"
}
```

**请求示例（自动创建）**

```json
{
  "platform": "telegram",
  "external_room_id": "chat_123",
  "room_name": "Telegram Chat",
  "invite": ["@admin:example.com"]
}
```

**响应 `201`**（新建）

```json
{
  "id": 1,
  "matrix_room_id": "!abc:example.com"
}
```

**响应 `200`**（已存在）

```json
{
  "id": 1,
  "matrix_room_id": "!abc:example.com"
}
```

### 查询映射列表

```
GET /api/v1/admin/rooms?platform=telegram
```

支持基于游标的分页查询。

| 参数 | 必填 | 默认值 | 说明 |
|------|------|--------|------|
| `platform` | 否 | — | 按平台筛选；不传则返回所有映射 |
| `after` | 否 | `0` | 游标：返回 `id > after` 的记录 |
| `limit` | 否 | `100` | 每页最大数量（上限 1000） |

**响应 `200`**

```json
{
  "rooms": [
    {
      "id": 1,
      "platform_id": "telegram",
      "external_room_id": "chat_123",
      "matrix_room_id": "!abc:example.com"
    }
  ],
  "next_cursor": 1
}
```

### 删除映射

```
DELETE /api/v1/rooms/{id}
```

**响应 `200`**

```json
{
  "deleted": true
}
```

**响应 `404`**（映射不存在时）

```json
{
  "error": "not found"
}
```

---

## Webhook 管理

Webhook 用于将 Matrix 侧产生的消息推送到外部平台（出站方向）。当已映射的 Matrix 房间中有新消息时，Bridge 会向所有匹配的 Webhook 发送 POST 请求。

### 注册 Webhook

```
POST /api/v1/webhooks
```

**请求参数**

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `platform` | string | 是 | 该 Webhook 所属的平台标识 |
| `url` | string | 是 | 接收回调的 URL（必须使用 `http` 或 `https` 协议；参见 [SSRF 防护](#ssrf-防护)） |
| `events` | string | 否 | 订阅的事件类型（默认 `"message"`） |
| `forward_sources` | array 或 string | 否 | 允许转发的来源平台白名单；空（默认）= 仅转发 Matrix 用户消息，`["*"]` = 转发所有，`["telegram", "discord"]` = 仅转发指定平台。支持 JSON 数组或逗号分隔字符串 |

**请求示例**

```json
{
  "platform": "myapp",
  "url": "http://myapp:8080/hook",
  "events": "message",
  "forward_sources": ["*"]
}
```

**响应 `201`**

```json
{
  "id": 1
}
```

### 查询 Webhook 列表

```
GET /api/v1/admin/webhooks?platform=myapp
```

支持基于游标的分页查询。

| 参数 | 必填 | 默认值 | 说明 |
|------|------|--------|------|
| `platform` | 否 | — | 按平台筛选；不传则返回所有 Webhook |
| `after` | 否 | `0` | 游标：返回 `id > after` 的记录 |
| `limit` | 否 | `100` | 每页最大数量（上限 1000） |

**响应 `200`**

```json
{
  "webhooks": [
    {
      "id": 1,
      "platform": "myapp",
      "url": "http://myapp:8080/hook",
      "events": "message",
      "forward_sources": ["*"]
    }
  ],
  "next_cursor": 1
}
```

### 删除 Webhook

```
DELETE /api/v1/webhooks/{id}
```

**响应 `200`**

```json
{
  "deleted": true
}
```

**响应 `404`**（Webhook 不存在时）

```json
{
  "error": "not found"
}
```

---

## 傀儡用户管理

傀儡用户是 Bridge 为外部平台用户自动创建的 Matrix 账号。

### 查询傀儡用户列表

```
GET /api/v1/admin/puppets?platform=telegram
```

支持基于游标的分页查询。

| 参数 | 必填 | 默认值 | 说明 |
|------|------|--------|------|
| `platform` | 否 | — | 按平台筛选；不传则返回所有傀儡用户 |
| `after` | 否 | `0` | 游标：返回 `id > after` 的记录 |
| `limit` | 否 | `100` | 每页最大数量（上限 1000） |

**响应 `200`**

```json
{
  "puppets": [
    {
      "id": 1,
      "matrix_user_id": "@bot_telegram_user123:example.com",
      "platform_id": "telegram",
      "external_user_id": "user123",
      "display_name": "Alice",
      "avatar_mxc": "mxc://example.com/abc123"
    }
  ],
  "next_cursor": 1
}
```

---

## 消息映射查询

消息映射记录 Matrix 事件与外部平台消息的对应关系。支持基于游标的分页查询。

### 查询消息映射列表

```
GET /api/v1/admin/messages?platform=telegram&room_mapping_id=1&after=0&limit=100
```

| 参数 | 必填 | 默认值 | 说明 |
|------|------|--------|------|
| `platform` | 否 | — | 按平台筛选 |
| `room_mapping_id` | 否 | — | 按房间映射 ID 筛选 |
| `after` | 否 | `0` | 游标：返回 `id > after` 的记录 |
| `limit` | 否 | `100` | 每页最大数量（上限 1000） |

**响应 `200`**

```json
{
  "messages": [
    {
      "id": 1,
      "matrix_event_id": "$event123",
      "platform_id": "telegram",
      "external_message_id": "msg_456",
      "room_mapping_id": 1
    }
  ],
  "next_cursor": 1
}
```

| 字段 | 说明 |
|------|------|
| `messages` | 消息映射对象数组 |
| `next_cursor` | 最后一条记录的 ID；作为下次请求的 `after` 参数。结果为空时为 `null` |

**分页示例：**

```
GET /api/v1/admin/messages?limit=100           -> next_cursor: 100
GET /api/v1/admin/messages?after=100&limit=100 -> next_cursor: 200
GET /api/v1/admin/messages?after=200&limit=100 -> next_cursor: null（无更多数据）
```

---

## WebSocket 实时推送

```
GET /api/v1/ws?platform=xxx&forward_sources=*
```

升级 HTTP 连接为 WebSocket，用于实时接收 Matrix 侧产生的消息。功能上是 Webhook 的替代方案，载荷格式与 [Webhook 回调](#webhook-回调格式出站)完全一致。

### 连接参数

| 参数 | 必填 | 说明 |
|------|------|------|
| `platform` | 是 | 平台标识（1-64 个字母数字或 `-_. ` 字符） |
| `forward_sources` | 否 | 允许转发的来源平台白名单，语义与 Webhook 的 `forward_sources` 相同。不传 = 仅转发 Matrix 用户消息，`*` = 转发所有来源 |

### 认证

当配置了 `api_key` 时，WebSocket 连接建立后，客户端必须在 **10 秒**内发送认证消息作为第一帧：

```json
{"access_token": "<api_key>"}
```

未在时限内发送有效认证消息的连接将被关闭（关闭码 `4001`）。

未配置 `api_key` 时，无需认证，连接建立后即可接收消息。

### 连接限制

- 最大并发连接数：**1000**
- 超过限制的新连接会被拒绝（关闭码 `4002`，原因 `"too many connections"`）

### 心跳

服务端每 **30 秒**发送一次 WebSocket Ping 帧。客户端应回复 Pong 帧以保持连接活跃。

### 消息格式

服务端推送的每条消息为一个 JSON 文本帧，格式与 Webhook 回调载荷完全一致。详见 [Webhook 回调格式](#webhook-回调格式出站)章节。

### 连接示例

```javascript
const ws = new WebSocket("ws://localhost:29320/api/v1/ws?platform=myapp&forward_sources=*");

// 如果配置了 api_key，先发送认证消息
ws.onopen = () => {
  ws.send(JSON.stringify({ access_token: "YOUR_API_KEY" }));
};

ws.onmessage = (event) => {
  const payload = JSON.parse(event.data);
  console.log("收到消息:", payload);
  // payload 格式与 Webhook 回调一致
};
```

---

## Webhook 回调格式（出站）

当已映射的 Matrix 房间中产生新消息时，Bridge 会向匹配的 Webhook 发送 JSON 回调（同时也会推送到匹配的 WebSocket 连接）。根据发送者身份的不同，回调格式略有差异。

### 真实 Matrix 用户发送的消息

一个 Matrix 原生用户在映射到 `myapp` 的房间中发消息：

```json
{
  "event": "message",
  "platform": "myapp",
  "message": {
    "id": "$event_id",
    "sender": {
      "platform": "matrix",
      "external_id": "@alice:example.com",
      "display_name": null,
      "avatar_url": null
    },
    "room": {
      "platform": "myapp",
      "external_id": "general"
    },
    "content": {
      "type": "text",
      "body": "Hello!"
    },
    "timestamp": 1710000000000,
    "reply_to": null
  }
}
```

### 跨平台转发的消息

一个 Telegram 傀儡用户在同时映射到 Slack 的房间中发消息，Slack 的 Webhook 会收到如下回调：

```json
{
  "event": "message",
  "platform": "slack",
  "source_platform": "telegram",
  "message": {
    "id": "$event_id",
    "sender": {
      "platform": "telegram",
      "external_id": "user123",
      "display_name": "Alice",
      "avatar_url": "mxc://example.com/abc123"
    },
    "room": {
      "platform": "slack",
      "external_id": "C123"
    },
    "content": {
      "type": "text",
      "body": "Hello!"
    },
    "timestamp": 1710000000000,
    "reply_to": null
  }
}
```

**关键区别：** 跨平台回调中会包含 `source_platform` 字段，标明消息的原始来源。`sender` 对象反映的是外部平台的真实用户身份，而非 Matrix 傀儡用户。

> **注意：** 跨平台转发需要配置 `allow_relay = true`，否则仅转发真实 Matrix 用户的消息。

### 回调字段说明

| 字段 | 类型 | 说明 |
|------|------|------|
| `event` | string | 事件类型（如 `"message"`） |
| `platform` | string | 目标平台（与 Webhook 注册时的 platform 一致） |
| `source_platform` | string | 仅跨平台消息包含此字段，表示消息的原始来源平台 |
| `message.id` | string | Matrix 事件 ID |
| `message.sender.platform` | string | 真实用户为 `"matrix"`，傀儡用户为其原始平台名 |
| `message.sender.external_id` | string | Matrix 用户 ID 或外部平台用户 ID |
| `message.sender.display_name` | string 或 null | 显示名称（如有） |
| `message.sender.avatar_url` | string 或 null | 头像地址（如有） |
| `message.room.platform` | string | 目标平台 |
| `message.room.external_id` | string | 房间映射中的外部房间 ID |
| `message.content` | object | 消息内容（见[消息内容类型](#消息内容类型)） |
| `message.timestamp` | number | Unix 时间戳（毫秒） |
| `message.reply_to` | string 或 null | 所回复消息的事件 ID |

---

## SSRF 防护

Webhook URL 始终要求使用 `http` 或 `https` 协议。当配置中设置 `appservice.webhook_ssrf_protection = true` 时，会额外拦截指向私有/保留网络的 URL：

- **拦截的 IP 范围：** 回环地址（127.0.0.0/8、::1）、RFC1918（10/8、172.16/12、192.168/16）、链路本地（169.254/16、fe80::/10）、CGNAT（100.64/10）、IPv6 ULA（fc00::/7）、未指定地址（0.0.0.0、::）、广播地址、文档保留地址、云元数据（169.254.169.254）
- **DNS 解析检查：** 域名会被解析，所有结果 IP 均经过检查，防止 DNS 重绑定攻击（如 `127.0.0.1.nip.io`）
- **IPv4 映射的 IPv6：** 如 `::ffff:10.0.0.1` 会被展开后按 IPv4 规则检查

默认值为 `false`（允许所有目标），适用于内部部署场景。

---

## 消息内容类型

| 类型 | 必填字段 | 可选字段 | Matrix 处理方式 |
|------|---------|---------|----------------|
| `text` | `body` | `html` | 发送为 `m.text`；`html` 字段经 ammonia 净化后作为 `formatted_body` |
| `image` | `url` | `caption`、`mimetype`（默认 `image/png`） | 发送为 `m.image` |
| `file` | `url`、`filename` | `mimetype`（默认 `application/octet-stream`） | 发送为 `m.file` |
| `video` | `url` | `caption`、`mimetype`（默认 `video/mp4`） | 发送为 `m.video` |
| `audio` | `url` | `mimetype`（默认 `audio/ogg`） | 发送为 `m.audio` |
| `location` | `latitude`、`longitude` | -- | 发送为 `m.location` |
| `notice` | `body` | -- | 发送为 `m.notice` |
| `emote` | `body` | -- | 发送为 `m.emote` |
| `reaction` | `target_id`、`emoji` | -- | **降级处理：** 作为 `m.text` 发送，body 为 emoji 内容 |
| `redaction` | `target_id` | -- | **降级处理：** 作为 `m.notice` 发送，body 为 `[message deleted]` |
| `edit` | `target_id`、`new_content` | -- | **降级处理：** `new_content` 作为新消息发送（不使用 Matrix 编辑协议） |

> **关于 reaction/redaction/edit：** 这些类型被接受但不会使用 Matrix 原生的 reaction、redaction 或 edit 协议。它们被转换为普通消息发送到 Matrix，以确保跨平台兼容性。

### 示例

**文本消息：**
```json
{ "type": "text", "body": "Hello!" }
```

**带 HTML 的文本：**
```json
{ "type": "text", "body": "Hello!", "html": "<b>Hello!</b>" }
```

> HTML 内容会使用 ammonia 库进行净化，去除潜在的 XSS 攻击载荷。

**图片（使用上传后的 mxc URI）：**
```json
{ "type": "image", "url": "mxc://example.com/abc123", "caption": "一张日落照片" }
```

**文件：**
```json
{ "type": "file", "url": "mxc://example.com/def456", "filename": "report.pdf", "mimetype": "application/pdf" }
```

**地理位置：**
```json
{ "type": "location", "latitude": 37.7749, "longitude": -122.4194 }
```

**表情回应：**
```json
{ "type": "reaction", "target_id": "msg_001", "emoji": "👍" }
```

> 在 Matrix 侧会作为普通文本消息发送，内容为 `👍`。

**编辑消息：**
```json
{ "type": "edit", "target_id": "msg_001", "new_content": { "type": "text", "body": "修改后的内容" } }
```

> `new_content` 会作为一条全新的消息发送到 Matrix，不会替换原消息。

**撤回消息：**
```json
{ "type": "redaction", "target_id": "msg_001" }
```

> 在 Matrix 侧会作为 `m.notice` 类型消息发送，内容为 `[message deleted]`。

---

## 傀儡用户命名规则

Bridge 会为外部平台的发送者自动创建 Matrix 傀儡用户，其用户名遵循以下格式：

```
@{puppet_prefix}_{platform}_{sender.id}:{homeserver_domain}
```

**格式约束：**

- `puppet_prefix`：可配置（默认 `bot`）
- `platform`：仅限小写字母（`[a-z]+`）
- `sender.id`：小写字母、数字以及 `.` `_` `-` `=` `/`（`[a-z0-9._\-=/]+`）

**自动净化与 SHA-256 哈希回退：**

当 `sender.id` 包含不合法的 Matrix localpart 字符时（如 `@`、`!`、非 ASCII 字符等），Bridge 不会拒绝请求，而是自动使用 SHA-256 哈希回退生成 localpart：

```
h_{hex(sha256(原始ID)[:16])}
```

该哈希取 SHA-256 的前 16 字节（128-bit），转为 32 位十六进制字符串。

**示例：**

| 平台 | 发送者 ID | Matrix 用户 ID |
|------|----------|----------------|
| telegram | `12345` | `@bot_telegram_12345:example.com` |
| slack | `u.bob` | `@bot_slack_u.bob:example.com` |
| discord | `98765` | `@bot_discord_98765:example.com` |
| custom | `user@name!` | `@bot_custom_h_<sha256-hex>:example.com` |
| custom | _(空字符串)_ | `@bot_custom_h_<sha256-hex>:example.com` |

---

## 输入净化

Bridge 对入站消息中的外部 ID 执行自动净化，不会因为特殊字符而拒绝请求。

### 外部 ID（`sender.id`、`room_id`）

- 控制字符（`\x00`-`\x1F` 等）会被静默移除
- 最长截取 255 字符
- 净化后结果为空时，使用 SHA-256 哈希回退：`h_{hex(sha256[:16])}`

**示例：**

| 输入 | 净化后 |
|------|--------|
| `room-123` | `room-123`（不变） |
| `room\x00\x01-123` | `room-123`（控制字符被移除） |
| `\x00\x01\x02` | `h_<sha256-hex>`（全部为控制字符，回退为哈希） |
| `房间-42` | `房间-42`（Unicode 保留） |

### 傀儡用户 localpart

参见[傀儡用户命名规则](#傀儡用户命名规则)章节。

### HTML 内容

当消息内容包含 `html` 字段（`formatted_body`）时，Bridge 使用 ammonia 库进行 HTML 净化，移除危险标签和属性，防止 XSS 攻击。
