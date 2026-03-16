# Matrix Bridge 架构说明

本文档说明当前代码实际实现的架构，而不是历史版本的设计。

## 总体结构

```text
外部服务
   |  REST / webhook / WebSocket
   v
matrix-bridge-appservice
   |  Matrix client-server + appservice API
   v
Synapse / Matrix Homeserver
```

仓库由三个 workspace crate 加一个二进制入口组成。

| 组件 | 路径 | 责任 |
|------|------|------|
| `matrix-bridge-core` | `crates/core` | 配置模型、共享消息类型、注册文件生成、ID 清洗工具 |
| `matrix-bridge-store` | `crates/store` | SQLite schema 与各种映射、puppet、webhook、space 的持久化 |
| `matrix-bridge-appservice` | `crates/appservice` | HTTP 服务、dispatcher、Matrix client、webhook / WS 投递、加密运行时 |
| `matrix-bridge` | `src/main.rs` | 启动编排、配置加载、注册文件管理、服务启动 |

## 运行时核心组件

### `MatrixClient`

封装 Matrix client-server 与 appservice 相关 HTTP 调用，负责：

- 用户与设备注册
- 房间创建、加入、离开、邀请、Space 操作
- 消息发送
- 媒体上传与下载
- 加密相关接口和 MSC 查询参数

### `PuppetManager`

按需创建和更新 puppet 用户。外部用户会被表示为：

```text
@{puppet_prefix}_{platform}_{sanitized_external_id}:{domain}
```

该组件会缓存已知 puppet，并把 profile 数据落到 SQLite。

### `Dispatcher`

它是整个桥接逻辑的中心路由器，负责：

- 外部 -> Matrix 投递
- Matrix -> webhook / WebSocket 投递
- bot 命令处理
- 跨平台 relay 决策
- 权限检查
- 平台 Space 维护

### `WsRegistry`

按平台追踪活跃的 WebSocket 订阅，包括：

- forward source 过滤
- capability 声明
- 连接数量与上限控制

### `CryptoManager` 与 `CryptoManagerPool`

用于端到桥加密：

- 单设备模式：所有加密发送共用 bridge bot 设备
- per-user 模式：每个 puppet 可拥有独立设备和 crypto store

## 存储模型

SQLite 中主要有五张核心表。

| 表 | 用途 |
|----|------|
| `room_mappings` | `(platform_id, external_room_id)` 与 `matrix_room_id` 的映射 |
| `message_mappings` | 按平台记录外部消息 ID 与 Matrix event ID 的对应关系 |
| `puppets` | puppet MXID 与缓存的 profile 数据 |
| `webhooks` | 已注册的出站 HTTP 集成、过滤条件、capability、owner |
| `platform_spaces` | 每个平台对应一个 Matrix Space |

几个关键约束：

- `room_mappings` 同时对 “Matrix 房间 + 平台” 以及 “平台 + 外部房间” 去重
- `message_mappings` 同时对 `(platform_id, external_message_id)` 和 `(matrix_event_id, platform_id)` 去重
- `webhooks` 在 `(platform_id, webhook_url)` 上做 upsert

## 启动顺序

`src/main.rs` 启动时会：

1. 读取 `BRIDGE_CONFIG`；若缺失则自动生成默认配置
2. 解析并校验配置
3. 打开 SQLite 并执行 migration
4. 构建 Matrix client
5. 初始化 puppet manager
6. 生成或校验 `registration.yaml`
7. 注册 bridge bot；启用加密时会一并处理设备信息
8. 启用加密时初始化 crypto pool
9. 构建 dispatcher 与共享状态
10. 启动 Axum HTTP 服务

## HTTP 暴露面

服务对外暴露三类路由。

### Matrix appservice 路由

由 `hs_token` 保护：

- `PUT /_matrix/app/v1/transactions/{txnId}`
- `GET /_matrix/app/v1/users/{userId}`
- `GET /_matrix/app/v1/rooms/{roomAlias}`

### Bridge HTTP API 路由

可选由 `appservice.api_key` 保护，并叠加按 IP 限流：

- `/api/v1/*` 下的业务路由
- `/api/v1/admin/*` 下的只读管理路由

### WebSocket 路由

- `GET /api/v1/ws`
- 如启用 `api_key`，通过首个 WS 帧认证，不走 query 参数

## 消息流

### 外部 -> Matrix

1. 外部服务调用 `POST /api/v1/message`
2. dispatcher 清洗外部 sender 和 room 标识
3. 从 SQLite 查找房间映射
4. 必要时创建或刷新 puppet
5. 确保 puppet 已加入目标 Matrix 房间
6. 发送消息到 Matrix；如房间启用加密则走加密发送
7. 写入 `message_mappings`，用于去重和反向查询

### Matrix -> 外部

1. Synapse 通过 appservice transaction 路由推送事件
2. 服务先对 transaction ID 去重
3. dispatcher 跳过 bridge 自己产生的环路事件，并执行权限与 relay 判断
4. 根据 Matrix 房间查出所有外部映射
5. 向匹配的 webhook 发 HTTP 回调
6. 向匹配的 WebSocket 客户端广播同样的 payload
7. 按目标平台写入 `message_mappings`

## 跨平台 Relay

跨平台 relay 指：

- Telegram 消息先进入 Matrix
- 同一个 Matrix 事件再继续转发给 Slack 等其他外部平台

它由两层控制：

1. 全局配置 `appservice.allow_relay`
2. 单个接入的 webhook / WS `forward_sources`

规则：

- Matrix 用户消息始终可参与外发
- 非 Matrix 来源消息只有在 `allow_relay = true` 时才允许继续外发
- `forward_sources = []` 表示仅接收 Matrix 来源
- `forward_sources = ["*"]` 表示允许任意来源平台

## 权限模型

当前实现已经不再使用历史上的 `invite_whitelist`，而是：

```toml
[permissions]
admin = ["@admin:example.com"]
relay = ["@*:trusted.example"]
relay_min_power_level = 0
```

语义：

- `admin`：完整权限，包括 bot 私聊命令和邀请 bridge 进入房间
- `relay`：较低权限层级；bot 私聊命令和邀请仍然只允许 admin
- `relay_min_power_level`：普通 Matrix 用户消息外发所需的房间 power level 下限
- 两个列表都为空时：开放模式，所有人都视为 admin

## 平台 Space

bridge 可为每个平台维护一个 Matrix Space。新建房间映射时会：

1. 创建或复用该平台对应的 Space
2. 将新映射的房间挂到该 Space 下

这样多房间、多平台接入时，客户端侧不需要自己维护组织结构。

## 安全模型

### 认证分离

- `hs_token` 只用于 Matrix appservice 流量
- `api_key` 只用于外部服务访问 Bridge API

### Webhook 校验

当 `appservice.webhook_ssrf_protection = true` 时，注册 webhook 会拦截：

- localhost
- `metadata.google.internal`
- IPv4 私网地址
- 回环与链路本地地址
- CGNAT 地址
- 文档 / 保留地址段
- IPv6 回环与 unique-local 地址
- 解析后落到受限地址的域名

### 输入加固

bridge 会：

- 在把外部 ID 用于存储或 puppet MXID 前先做清洗
- 限制请求字段长度
- 限制 reaction emoji 长度
- 将上传体大小限制在 `200 MiB`

### 限流

Bridge HTTP API 使用 governor 层限流：

- 每秒 `120` 次请求
- `300` burst

## 加密模式

关闭加密时，消息按普通 Matrix API 明文发送。

启用加密时：

- bridge bot 会拥有持久化 crypto store
- 注册文件会加入 appservice 相关 MSC 字段
- 房间是否加密会动态检测
- 出站加密消息通过 crypto manager 处理

当前有两种运行模式：

| 模式 | 配置 | 行为 |
|------|------|------|
| 单设备模式 | `per_user_crypto = false` | puppet 通过 bridge bot 设备伪装发送 |
| per-user 模式 | `per_user_crypto = true` | 每个活跃 puppet 都会有独立派生设备 ID 和 crypto store |

更多实现细节见 [encryption.zh.md](encryption.zh.md)。

## 设计取向

这套设计刻意保持窄接口：

- 外部集成只需要会 HTTP 或 WebSocket
- Matrix 侧复杂度收敛在 bridge 内部
- 持久化保持 SQLite，降低部署成本
- 加密能力保持可选，不污染普通消息路径

因此常见接入路径足够轻，而在需要时仍能支持 relay、Space 和加密房间。
