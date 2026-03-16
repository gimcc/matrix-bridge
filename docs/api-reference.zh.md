# Matrix Bridge API 参考

基础地址：`http://<bridge-host>:29320`

除特别说明外，Bridge API 的请求与响应均为 JSON。

## 认证

`/api/v1/*` 下的 HTTP 路由可通过 `appservice.api_key` 启用可选认证。

启用后，请在请求头中携带：

```http
Authorization: Bearer <api_key>
```

说明：

- Bridge HTTP API 仅支持请求头认证。
- WebSocket 认证单独处理，通过首帧发送，不放在 URL 上。
- `hs_token` 只用于 Synapse 与 `/_matrix/app/v1/*` appservice 路由交互。

## 限流

Bridge HTTP API 按客户端 IP 限流：

- 每秒 `120` 次请求
- 突发 `300`

以下路由不受此限流影响：

- `/health`
- `/_matrix/app/v1/*`
- `/api/v1/ws`

## 端点总览

| 方法 | 路径 | 用途 |
|------|------|------|
| `GET` | `/health` | 存活检查 |
| `POST` | `/api/v1/message` | 将外部消息写入 Matrix |
| `POST` | `/api/v1/upload` | 上传媒体到 Matrix 媒体仓库 |
| `POST` | `/api/v1/rooms` | 创建或复用房间映射 |
| `DELETE` | `/api/v1/rooms/{id}` | 删除房间映射 |
| `POST` | `/api/v1/webhooks` | 注册或更新 webhook |
| `DELETE` | `/api/v1/webhooks/{id}` | 删除 webhook |
| `GET` | `/api/v1/ws` | WebSocket 订阅端点 |
| `GET` | `/api/v1/admin/info` | 运行时摘要 |
| `GET` | `/api/v1/admin/crypto` | 加密状态 |
| `GET` | `/api/v1/admin/rooms` | 分页查看房间映射 |
| `GET` | `/api/v1/admin/webhooks` | 分页查看 webhook |
| `GET` | `/api/v1/admin/puppets` | 分页查看 puppet 用户 |
| `GET` | `/api/v1/admin/messages` | 分页查看消息映射 |
| `GET` | `/api/v1/admin/spaces` | 平台 Space 映射 |
| `GET` | `/api/v1/admin/capabilities` | 聚合平台 capability |

## 健康检查

```http
GET /health
```

响应：

```json
{ "status": "ok" }
```

## 外部消息写入 Matrix

```http
POST /api/v1/message
```

用于把外部平台消息投递到已映射的 Matrix 房间。

### 请求体

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `platform` | string | 是 | 平台 ID，1..64 字符，仅允许字母数字、`_`、`-`、`.` |
| `room_id` | string | 是 | 外部房间 ID，不是 Matrix room ID |
| `sender.id` | string | 是 | 外部发送者 ID |
| `sender.display_name` | string | 否 | puppet 显示名 |
| `sender.avatar_url` | string | 否 | 外部头像 URL |
| `content` | object | 是 | 见下方“入站内容类型” |
| `external_message_id` | string | 否 | 去重键；缺省时自动生成 |
| `reply_to` | string | 否 | 被回复消息的外部消息 ID |

示例：

```json
{
  "platform": "telegram",
  "room_id": "chat_123",
  "sender": {
    "id": "user_42",
    "display_name": "Alice"
  },
  "content": {
    "type": "text",
    "body": "Hello",
    "html": "<b>Hello</b>"
  },
  "external_message_id": "msg-001"
}
```

响应：

```json
{
  "event_id": "$event:example.com",
  "message_id": "msg-001"
}
```

说明：

- `room_id` 和 `sender.id` 在用于存储和生成 puppet MXID 之前会先做清洗。
- `(platform, external_message_id)` 已存在时会被视为重复消息。
- `reply_to`、edit / reaction / redaction 的目标字段都指向外部消息 ID。

## 媒体上传

```http
POST /api/v1/upload
Content-Type: multipart/form-data
```

表单字段：

- `file`：必填，只处理第一个文件字段

响应：

```json
{
  "content_uri": "mxc://example.com/abc123",
  "filename": "photo.png",
  "size": 12345
}
```

说明：

- 最大大小为 `200 MiB`
- 文件会上传到 Matrix 媒体仓库
- 返回的 `content_uri` 可在 `image`、`file`、`video`、`audio` 类型中复用

## 房间映射

### 创建或复用映射

```http
POST /api/v1/rooms
```

请求体：

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `platform` | string | 是 | 外部平台 ID |
| `external_room_id` | string | 是 | 外部房间标识 |
| `matrix_room_id` | string | 否 | 已存在的 Matrix 房间 ID；省略则自动创建 |
| `room_name` | string | 否 | 自动建房时使用的房间名 |
| `invite` | array<string> | 否 | 额外邀请的 Matrix 用户；只有 `allow_api_invite = true` 时才生效 |

示例：

```json
{
  "platform": "slack",
  "external_room_id": "C123456",
  "room_name": "Slack / General",
  "invite": ["@ops:example.com"]
}
```

行为：

- 新建映射返回 `201`
- 已存在映射返回 `200`
- 省略 `matrix_room_id` 时，bridge 会自动创建 Matrix 房间
- 自动创建的房间始终会带上 `appservice.auto_invite`
- 若启用 `allow_api_invite = true`，请求内 `invite` 会与 `auto_invite` 合并
- 新建映射时还会尝试把房间挂到该平台的 Matrix Space 下

响应：

```json
{
  "id": 1,
  "matrix_room_id": "!room:example.com"
}
```

### 删除映射

```http
DELETE /api/v1/rooms/{id}
```

响应：

```json
{ "deleted": true }
```

### 查询映射

```http
GET /api/v1/admin/rooms?platform=telegram&after=0&limit=100
```

查询参数：

- `platform`：可选过滤条件
- `after`：游标，默认 `0`
- `limit`：页大小，限制在 `1..1000`

响应：

```json
{
  "rooms": [
    {
      "id": 1,
      "matrix_room_id": "!room:example.com",
      "platform_id": "telegram",
      "external_room_id": "chat_123",
      "created_at": "2026-03-27 00:00:00"
    }
  ],
  "next_cursor": 1
}
```

## Webhook

### 注册或更新 webhook

```http
POST /api/v1/webhooks
```

请求体：

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `platform` | string | 是 | 目标平台 |
| `url` | string | 是 | `http` 或 `https` webhook URL |
| `events` | string | 否 | 仅保存到存储，默认 `message`；当前实现不会用它硬过滤出站事件 |
| `forward_sources` | string 或 array<string> | 否 | 非 Matrix 来源平台允许列表 |
| `capabilities` | string 或 array<string> | 否 | 该接入声明支持的功能 |
| `owner` | string | 否 | 自动邀请进该平台 portal room 的 Matrix 用户 |

示例：

```json
{
  "platform": "telegram",
  "url": "https://hooks.example.com/tg",
  "events": "message,redaction",
  "forward_sources": ["matrix", "slack"],
  "capabilities": ["message", "image", "command"],
  "owner": "@bridge-admin:example.com"
}
```

语义：

- Matrix 用户消息始终可参与转发
- `forward_sources` 只控制非 Matrix 来源的跨平台转发
- 空 `forward_sources` 表示“仅 Matrix”
- `"*"` 表示允许所有来源
- 重复注册相同 `(platform, url)` 会更新配置并重新启用该 webhook

当 `appservice.webhook_ssrf_protection = true` 时，指向 localhost、metadata 域名、私网、回环、链路本地、CGNAT 以及其他保留地址的 URL 会被拒绝。

响应：

```json
{ "id": 1 }
```

### 删除 webhook

```http
DELETE /api/v1/webhooks/{id}
```

响应：

```json
{ "deleted": true }
```

### 查询 webhook

```http
GET /api/v1/admin/webhooks?platform=telegram&after=0&limit=100
```

响应：

```json
{
  "webhooks": [
    {
      "id": 1,
      "platform_id": "telegram",
      "webhook_url": "https://hooks.example.com/tg",
      "events": "message,redaction",
      "enabled": true,
      "forward_sources": "matrix,slack",
      "capabilities": "message,image,command",
      "owner": "@bridge-admin:example.com"
    }
  ],
  "next_cursor": 1
}
```

## WebSocket

```http
GET /api/v1/ws?platform=telegram&forward_sources=*&capabilities=message,image,command
```

查询参数：

| 参数 | 必填 | 说明 |
|------|------|------|
| `platform` | 是 | 订阅的平台键 |
| `forward_sources` | 否 | 逗号分隔的来源允许列表 |
| `capabilities` | 否 | 逗号分隔的 capability 列表 |

当配置了 `api_key` 时，客户端必须在 10 秒内发送首帧：

```json
{ "access_token": "<api_key>" }
```

说明：

- 不要把 API key 放在 WebSocket URL 上
- bridge 全局最多接受 `1000` 个 WebSocket 客户端
- WebSocket 接收到的消息结构与 webhook 回调一致

## 管理端点

### 运行时摘要

```http
GET /api/v1/admin/info
```

示例响应：

```json
{
  "version": "0.1.0",
  "homeserver": {
    "url": "http://matrix:8008",
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
    "admin": ["@admin:example.com"],
    "relay": ["@*:trusted.example"]
  },
  "platforms": {
    "configured": ["telegram", "slack"],
    "active": ["telegram"]
  },
  "stats": {
    "room_mappings": 5,
    "webhooks": 2,
    "message_mappings": 120,
    "puppets": 40,
    "ws_clients": 1
  }
}
```

### 加密状态

```http
GET /api/v1/admin/crypto
```

关键字段：

- `enabled`：是否启用加密
- `per_user_crypto`：是否为每个 puppet 分配独立设备
- `bot`：bridge bot 加密状态或 `null`
- `puppets`：已初始化的 puppet 加密设备状态

### 查询 puppet 用户

```http
GET /api/v1/admin/puppets?platform=telegram&after=0&limit=100
```

### 查询消息映射

```http
GET /api/v1/admin/messages?platform=telegram&room_mapping_id=1&after=0&limit=100
```

### 查询平台 Space

```http
GET /api/v1/admin/spaces
```

响应：

```json
{
  "spaces": [
    {
      "id": 1,
      "platform_id": "telegram",
      "matrix_space_id": "!space:example.com"
    }
  ]
}
```

### 查询聚合 capability

```http
GET /api/v1/admin/capabilities?platform=telegram
```

响应：

```json
{
  "platform": "telegram",
  "capabilities": ["command", "image", "message"]
}
```

## Webhook / WebSocket 回调格式

### 消息事件

```json
{
  "event": "message",
  "platform": "telegram",
  "source_platform": "slack",
  "message": {
    "id": "$event:example.com",
    "sender": {
      "platform": "slack",
      "external_id": "alice",
      "display_name": "Alice",
      "avatar_url": "https://example.com/alice.png"
    },
    "room": {
      "platform": "telegram",
      "external_id": "chat_123",
      "name": null
    },
    "content": {
      "type": "text",
      "body": "Hello",
      "formatted_body": "<b>Hello</b>"
    },
    "timestamp": 1710000000000,
    "reply_to": null
  }
}
```

`source_platform` 仅在跨平台转发时出现。

### 命令事件

```json
{
  "event": "command",
  "platform": "telegram",
  "sender": "@user:example.com",
  "command": "/start",
  "room_id": "!dm:example.com"
}
```

## 入站内容类型

这些结构可用于 `POST /api/v1/message`。

### 文本

```json
{ "type": "text", "body": "Hello", "html": "<b>Hello</b>" }
```

### notice / emote

```json
{ "type": "notice", "body": "Bridge notice" }
{ "type": "emote", "body": "waves" }
```

### 媒体

```json
{ "type": "image", "url": "mxc://example.com/abc", "caption": "Photo", "mimetype": "image/png" }
{ "type": "file", "url": "mxc://example.com/abc", "filename": "doc.pdf", "mimetype": "application/pdf" }
{ "type": "video", "url": "mxc://example.com/abc", "caption": "Clip", "mimetype": "video/mp4", "duration": 30 }
{ "type": "audio", "url": "mxc://example.com/abc", "mimetype": "audio/ogg", "duration": 5 }
```

### 位置

```json
{ "type": "location", "latitude": 48.8566, "longitude": 2.3522 }
```

### reaction / redaction / edit

```json
{ "type": "reaction", "target_id": "msg-001", "emoji": "👍" }
{ "type": "redaction", "target_id": "msg-001" }
{
  "type": "edit",
  "target_id": "msg-001",
  "new_content": { "type": "text", "body": "corrected text" }
}
```

## 出站内容说明

Webhook 和 WebSocket 的消息内容使用内部 `MessageContent` 结构：

- 文本格式字段输出为 `formatted_body`，不是 `html`
- 编辑消息使用 `new_content`
- 所有内容类型仍沿用相同的 `type` 标签
