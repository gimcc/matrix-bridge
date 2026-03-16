# 接入指南

本文档面向要把自有平台接入 Matrix 的外部服务，说明当前 Bridge API 的正确使用方式。

## 接入模型

```text
你的服务  <->  Matrix Bridge  <->  Synapse / Matrix
   REST / WS        appservice
```

常见流程：

1. 用 `POST /api/v1/rooms` 创建或复用房间映射
2. 注册 webhook 或建立 WebSocket 连接
3. 用 `POST /api/v1/message` 把外部消息送进 Matrix
4. 通过 webhook 或 WebSocket 接收 Matrix 侧消息

## 快速接入

### 1. 注册 webhook

```bash
curl -X POST http://localhost:29320/api/v1/webhooks \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer <api_key>" \
  -d '{
    "platform": "telegram",
    "url": "https://your-service.example.com/webhook",
    "forward_sources": ["matrix"],
    "capabilities": ["message", "image", "file", "reaction", "command"],
    "owner": "@bridge-admin:example.com"
  }'
```

字段说明：

| 字段 | 说明 |
|------|------|
| `platform` | 平台 ID，最大 64 字符，仅允许字母数字、`_`、`-`、`.` |
| `url` | 必须使用 `http` 或 `https` |
| `forward_sources` | 控制哪些非 Matrix 来源平台可以继续转发给该接入 |
| `capabilities` | 向 bridge 声明该接入支持的功能 |
| `owner` | 该平台 portal room 自动邀请的 Matrix 用户 |
| `events` | 目前只会保存到存储中；实际消息投递依赖路由与 `forward_sources`，不会按它做硬过滤 |

### 2. 创建房间映射

```bash
curl -X POST http://localhost:29320/api/v1/rooms \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer <api_key>" \
  -d '{
    "platform": "telegram",
    "external_room_id": "-1001234567890",
    "room_name": "Telegram / General"
  }'
```

说明：

- 省略 `matrix_room_id` 时，bridge 会自动创建 Matrix 房间
- 如果要绑定已有 Matrix 房间，直接传 `matrix_room_id`
- bridge 可能会自动创建平台 Matrix Space 并把该房间挂进去

### 3. 向 Matrix 发送消息

`POST /api/v1/message` 使用的是 `room_id`，不是 `external_room_id`。

```bash
curl -X POST http://localhost:29320/api/v1/message \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer <api_key>" \
  -d '{
    "platform": "telegram",
    "room_id": "-1001234567890",
    "sender": {
      "id": "12345",
      "display_name": "Alice",
      "avatar_url": "https://example.com/alice.jpg"
    },
    "content": {
      "type": "text",
      "body": "Hello from Telegram",
      "html": "<b>Hello from Telegram</b>"
    }
  }'
```

### 4. 接收 Matrix 消息

当 Matrix 用户在已映射房间发言时，bridge 会向你的 webhook 推送：

```json
{
  "event": "message",
  "platform": "telegram",
  "message": {
    "id": "$event_id",
    "sender": {
      "platform": "matrix",
      "external_id": "@alice:example.com",
      "display_name": null,
      "avatar_url": null
    },
    "room": {
      "platform": "telegram",
      "external_id": "-1001234567890",
      "name": null
    },
    "content": {
      "type": "text",
      "body": "Hello from Matrix",
      "formatted_body": "<b>Hello from Matrix</b>"
    },
    "timestamp": 1711234567000,
    "reply_to": null
  }
}
```

注意：

- 出站文本格式字段是 `formatted_body`，不是 `html`
- 只有跨平台转发时才会带 `source_platform`

## WebSocket 接入

如果你更适合长连接订阅而不是 webhook，可使用 WebSocket：

```text
ws://localhost:29320/api/v1/ws?platform=telegram&forward_sources=*&capabilities=message,image,command
```

查询参数：

| 参数 | 说明 |
|------|------|
| `platform` | 必填，订阅的平台键 |
| `forward_sources` | 可选，逗号分隔的来源允许列表 |
| `capabilities` | 可选，逗号分隔的 capability 列表 |

如果启用了 `appservice.api_key`，客户端必须在 10 秒内发送首帧：

```json
{ "access_token": "<api_key>" }
```

认证缺失或错误时，bridge 会主动关闭连接。

## 支持的入站内容类型

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

### reaction / edit / redaction

```json
{ "type": "reaction", "target_id": "msg-001", "emoji": "👍" }
{ "type": "redaction", "target_id": "msg-001" }
{
  "type": "edit",
  "target_id": "msg-001",
  "new_content": { "type": "text", "body": "corrected text" }
}
```

## 命令与权限

### Bridge bot 私聊命令

只有 `admin` 权限用户可以使用：

- `!help`
- `!platforms`
- `!rooms [platform]`
- `!spaces`
- `!<platform>`
- `!<platform> <command>`

### 房间命令

在已桥接房间中，bridge 识别以下命令：

| 命令 | 要求 |
|------|------|
| `!bridge link <platform> <external_id>` | 发送者 power level >= 50 |
| `!bridge unlink <platform>` | 发送者 power level >= 50 |
| `!bridge status` | 无额外 power level 限制 |

### Relay 行为

出站消息是否继续转发，主要受两个控制：

1. `appservice.allow_relay`
2. webhook / WS `forward_sources`

此外，`relay_min_power_level` 会为普通 Matrix 房间消息增加一个最小 power level 门槛。

## 认证总结

当配置了 `appservice.api_key`：

- HTTP Bridge API：使用 `Authorization: Bearer <api_key>`
- WebSocket：首帧必须是 `{"access_token":"<api_key>"}`，且需在 10 秒内发出

Matrix appservice 路由使用的是单独的 `hs_token`，外部服务不需要调用它们。
