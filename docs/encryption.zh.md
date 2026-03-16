# 端到桥加密 (E2BE)

## 概述

Matrix Bridge 使用 `matrix-sdk-crypto`（OlmMachine）直接实现端到桥加密，不依赖完整的 `matrix-sdk` 客户端。这是 Rust appservice 的推荐方式，因为 `matrix-sdk-appservice` 已于 2023 年 9 月从 SDK 中移除。

## 架构

### 当前：单设备模式

所有加密操作使用桥接 bot 用户的单个 `OlmMachine` 实例。Puppet 用户通过 MSC3202/MSC4190 设备伪装（device masquerading）以桥接 bot 的设备发送加密消息。

```
┌─────────────┐     ┌──────────────┐     ┌────────────┐
│  Homeserver  │────▶│ CryptoManager │────▶│ OlmMachine │
│  (Synapse)   │◀────│  (bridge bot) │◀────│  (single)  │
└─────────────┘     └──────────────┘     └────────────┘
       │                    │
       │  MSC3202           │  为所有 puppet 加密
       │  device_id QP      │  通过设备伪装
       ▼                    ▼
  ┌─────────┐        ┌──────────┐
  │ Puppet A │        │ Puppet B │
  │ (无独立  │        │ (无独立  │
  │  设备)   │        │  设备)   │
  └─────────┘        └──────────┘
```

### Per-User 模式（可选）

每个 puppet 可以拥有独立的 `OlmMachine` 和设备密钥。通过配置 `per_user_crypto = true` 启用。这消除了 MismatchedSender 警告，提高客户端兼容性。

```
┌─────────────────────────────────────────────┐
│            CryptoManagerPool                │
│                                             │
│  bot: Arc<CryptoManager>  (始终初始化)       │
│  puppets: HashMap<UserId, Arc<CryptoMgr>>   │
│           (按需懒加载)                       │
│                                             │
│  to-device 路由:                             │
│    txn.to_user_id → 查找 OlmMachine          │
│    → 每个 machine 独立 receive_sync_changes   │
│                                             │
│  加密: puppet 自己的 OlmMachine               │
│  解密: bot 的 OlmMachine (始终在房间中)        │
└─────────────────────────────────────────────┘
```

**模式对比：**

| | 单设备模式（默认） | Per-User 模式 |
|---|---|---|
| 配置 | `per_user_crypto = false` | `per_user_crypto = true` |
| OlmMachine 实例数 | 1 | 每个活跃 puppet 1 个（懒加载） |
| Crypto 存储 | 1 个 SQLite DB | `{crypto_store}/puppets/{localpart}/` 每 puppet |
| MSC 依赖 | MSC3202/MSC4190 伪装 | MSC3202 to-device 路由 |
| 客户端兼容性 | MismatchedSender 警告 | 所有客户端正常 |
| to-device 路由 | 所有事件到 bridge bot | 按 `to_user_id` 路由 |
| OTK 管理 | 单一池 | 每用户独立池（MSC3202 事务） |
| 设备 ID | 配置的 `device_id` | `{puppet_device_prefix}_{sha256(localpart)[0:16]}` |

## 加密流程

### 启动

```
1. 加载配置 (EncryptionConfig)
2. 在 MatrixClient 上设置 device_id (MSC3202)
3. 注册 bridge bot 及设备 (register_puppet_with_device)
4. 初始化 CryptoManager:
   a. 打开/创建 SqliteCryptoStore（带密码）
   b. 构建 OlmMachine（user_id + device_id）
   c. 处理出站请求（上传设备密钥）
   d. 验证 homeserver 上的设备密钥
   e. 如果密钥缺失：从头重建 crypto store
5. 将 crypto 连接到 Dispatcher
```

### 入站加密消息（解密）

```
收到事务 (PUT /_matrix/app/v1/transactions/{txnId})
  │
  ├─ 1. 处理 MSC2409/3202 加密数据（始终处理，即使为空）:
  │     - to-device 事件（Olm 密钥交换）
  │     - 设备列表变更
  │     - OTK 计数 → 不足时补充
  │     - fallback key 类型
  │
  ├─ 2. receive_sync_changes() [持有写锁]
  │     → OlmMachine 处理密钥交换
  │     → process_outgoing_requests() (密钥声明、上传)
  │
  ├─ 3. 对每个 m.room.encrypted 事件:
  │     a. 确保房间标记为已加密
  │     b. 更新跟踪用户（设备密钥查询）
  │     c. decrypt() → DecryptedEvent
  │     d. 路由解密后的内部事件 (m.room.message → webhooks)
  │
  └─ 4. 刷新出站加密请求
```

### 出站加密消息（加密）

```
send_to_matrix() 被调用（来自 bridge API 或 webhook 响应）
  │
  ├─ 1. 检查房间加密状态（本地存储 → 服务器状态事件）
  │
  ├─ 2. encrypt() [在整个流程中持有写锁]:
  │     a. update_tracked_users(room_members)
  │     b. process_keys_query_requests() — 获取设备密钥
  │     c. claim_missing_sessions() — 建立 Olm 会话
  │     d. share_room_key() — 分发 Megolm 会话密钥
  │     e. encrypt_room_event_raw() — 加密内容
  │     f. process_outgoing_requests() — 刷新剩余请求
  │
  └─ 3. send_encrypted_message() 带 device_id 查询参数
```

## 并发模型

使用 `tokio::sync::RwLock` 序列化加密操作，防止同步处理和加密之间的竞争（借鉴 matrix-bot-sdk 的 `AsyncLock`）：

- **`receive_sync_changes()`** — 获取写锁
- **`encrypt()`** — 在所有 5 个准备步骤 + 加密过程中持有写锁
- **`update_tracked_users()`** — 获取写锁
- **`decrypt()`** — 不需要锁（对 OlmMachine 来说是只读的）

## 房间加密检测

借鉴 matrix-bot-sdk 的 `RoomTracker`：

1. **快速路径：** 检查本地 crypto store（`is_room_encrypted_local`）
2. **服务器查询：** `GET /rooms/{roomId}/state/m.room.encryption/`
3. **自动同步：** 如果服务器显示已加密但本地存储不一致，更新本地状态

此机制处理桥接启动后加密的房间，以及桥接中途加入的房间。

## MSC 字段名兼容性

事务处理器同时支持不稳定和稳定的 MSC 前缀：

| 数据 | 不稳定前缀 | 稳定前缀 |
|------|-----------|---------|
| to-device 事件 | `de.sorunome.msc2409.to_device` | `org.matrix.msc2409.to_device` |
| 设备列表 | `de.sorunome.msc3202.device_lists` | `org.matrix.msc3202.device_lists` |
| OTK 计数 | `de.sorunome.msc3202.device_one_time_keys_count` | `org.matrix.msc3202.device_one_time_keys_count` |
| fallback keys | `de.sorunome.msc3202.device_unused_fallback_key_types` | `org.matrix.msc3202.device_unused_fallback_key_types` |

同时处理某些 Synapse 版本使用的 `device_one_time_key_counts`（带 `s`）变体。

## 配置

```toml
[encryption]
allow = true                         # 启用端到桥加密
default = true                       # 新建房间自动加密
appservice = true                    # 使用 appservice 模式 (MSC2409/MSC3202)
crypto_store = "/data/crypto"        # crypto store 目录
crypto_store_passphrase = "..."      # store 加密密码
device_id = "matrix_bridge"          # bridge bot 设备 ID
per_user_crypto = false              # 启用 per-user 模式
puppet_device_prefix = "puppet"      # puppet 设备 ID 前缀
```

## 依赖

| Crate | 版本 | 用途 |
|-------|------|------|
| `matrix-sdk-crypto` | 0.16 | OlmMachine, E2EE 状态机 |
| `matrix-sdk-sqlite` | 0.16 | SqliteCryptoStore（密码加密） |
| `ruma` | 0.14 | Matrix 协议类型（事件、API 请求/响应） |

## 参考

- [matrix-bot-sdk 加密实现](https://github.com/turt2live/matrix-bot-sdk/tree/main/src/e2ee) — TypeScript 参考，使用 `@matrix-org/matrix-sdk-crypto-nodejs`
- [MSC2409](https://github.com/matrix-org/matrix-spec-proposals/pull/2409) — Appservice to-device 事件
- [MSC3202](https://github.com/matrix-org/matrix-spec-proposals/pull/3202) — Appservice 设备列表/OTK 计数
- [MSC4190](https://github.com/matrix-org/matrix-spec-proposals/pull/4190) — Appservice 设备管理
