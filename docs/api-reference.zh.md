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
GET /api/v1/rooms?platform=myapp&access_token=<api_key>
```

未配置 `api_key` 时（默认），Bridge API 不需要认证。适用于内部/可信网络部署，由网络层（防火墙、反向代理等）控制访问。

> **注意：** `api_key` 与 `hs_token` 完全独立。`hs_token` 是 Matrix 协议密钥，仅用于 Synapse 与桥接器之间的 `/_matrix/app/v1/*` 路由。外部服务不应使用或知晓 `hs_token`。

---

## 目录

- [认证](#认证)
- [健康检查](#健康检查)
- [发送入站消息](#发送入站消息)
- [上传媒体文件](#上传媒体文件)
- [房间映射管理](#房间映射管理)
- [Webhook 管理](#webhook-管理)
- [Webhook 回调格式](#webhook-回调格式出站)
- [SSRF 防护](#ssrf-防护)
- [消息内容类型](#消息内容类型)
- [傀儡用户命名规则](#傀儡用户命名规则)

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

## 发送入站消息

```
POST /api/v1/message
```

将外部平台的消息桥接到 Matrix。Bridge 会自动为发送者创建傀儡用户（puppet user），并将消息投递到对应的 Matrix 房间。

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

上述请求会创建傀儡用户 `@telegram_user789:<homeserver_domain>`。

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

**请求体**

```json
{
  "platform": "telegram",
  "external_room_id": "chat_123",
  "matrix_room_id": "!abc:example.com"
}
```

**响应 `201`**

```json
{
  "id": 1
}
```

### 查询映射列表

```
GET /api/v1/rooms?platform=telegram
```

| 参数 | 必填 | 说明 |
|------|------|------|
| `platform` | 是 | 按平台筛选 |

**响应 `200`**

```json
{
  "rooms": [
    {
      "id": 1,
      "platform": "telegram",
      "external_room_id": "chat_123",
      "matrix_room_id": "!abc:example.com"
    }
  ]
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
| `exclude_sources` | array 或 string | 否 | 需要排除的来源平台；支持 JSON 数组 `["telegram", "discord"]` 或逗号分隔字符串 `"telegram,discord"` |

**请求示例**

```json
{
  "platform": "myapp",
  "url": "http://myapp:8080/hook",
  "events": "message",
  "exclude_sources": ["telegram", "discord"]
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
GET /api/v1/webhooks?platform=myapp
```

| 参数 | 必填 | 说明 |
|------|------|------|
| `platform` | 否 | 按平台筛选；不传则返回所有 Webhook |

**响应 `200`**

```json
{
  "webhooks": [
    {
      "id": 1,
      "platform": "myapp",
      "url": "http://myapp:8080/hook",
      "events": "message",
      "exclude_sources": ["telegram", "discord"]
    }
  ]
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

## Webhook 回调格式（出站）

当已映射的 Matrix 房间中产生新消息时，Bridge 会向匹配的 Webhook 发送 JSON 回调。根据发送者身份的不同，回调格式略有差异。

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

| 类型 | 必填字段 | 可选字段 |
|------|---------|---------|
| `text` | `body` | `html` |
| `image` | `url` | `caption`、`mimetype`（默认 `image/png`） |
| `file` | `url`、`filename` | `mimetype`（默认 `application/octet-stream`） |
| `video` | `url` | `caption`、`mimetype`（默认 `video/mp4`） |
| `audio` | `url` | `mimetype`（默认 `audio/ogg`） |
| `location` | `latitude`、`longitude` | -- |
| `notice` | `body` | -- |
| `emote` | `body` | -- |
| `reaction` | `target_id`、`emoji` | -- |
| `redaction` | `target_id` | -- |
| `edit` | `target_id`、`new_content` | -- |

### 示例

**文本消息：**
```json
{ "type": "text", "body": "Hello!" }
```

**带 HTML 的文本：**
```json
{ "type": "text", "body": "Hello!", "html": "<b>Hello!</b>" }
```

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

**编辑消息：**
```json
{ "type": "edit", "target_id": "msg_001", "new_content": { "type": "text", "body": "修改后的内容" } }
```

**撤回消息：**
```json
{ "type": "redaction", "target_id": "msg_001" }
```

---

## 傀儡用户命名规则

Bridge 会为外部平台的发送者自动创建 Matrix 傀儡用户，其用户名遵循以下格式：

```
@{platform}_{sender.id}:{homeserver_domain}
```

**格式约束：**

- `platform`：仅限小写字母（`[a-z]+`）
- `sender.id`：小写字母、数字以及 `.` `_` `-` `=` `/`（`[a-z0-9._\-=/]+`）

**示例：**

| 平台 | 发送者 ID | Matrix 用户 ID |
|------|----------|----------------|
| telegram | `12345` | `@telegram_12345:example.com` |
| slack | `u.bob` | `@slack_u.bob:example.com` |
| discord | `98765` | `@discord_98765:example.com` |
