# Matrix Bridge

一个面向 HTTP 集成的双向 Matrix Bridge。

English documentation: [README.md](README.md).

外部服务通过 REST、webhook 或 WebSocket 与 bridge 通信；bridge 负责 Matrix appservice 协议、puppet 用户、房间映射、可选端到桥加密，以及跨平台消息分发。

## 功能概览

- 面向任意 HTTP 服务的通用 Bridge API
- 双向消息流转：外部 -> Matrix，Matrix -> 外部
- puppet 用户命名格式：`@{prefix}_{platform}_{id}:domain`
- 支持自动建房与平台 Matrix Space 组织
- 支持通过共享 Matrix 房间做跨平台 relay
- webhook 与 WebSocket 客户端可声明 capability
- 支持 bot 私聊命令与平台命令透传
- 使用 `admin / relay / relay_min_power_level` 权限模型
- 可选端到桥加密，支持单设备和 per-user crypto
- 支持文本、图片、文件、音视频、reaction、edit、redaction、location 等内容类型

## 快速入口

首次运行时，如果 `config.toml` 不存在，程序会自动生成默认配置并退出：

```bash
BRIDGE_CONFIG=/data/config.toml cargo run
```

随后可生成 appservice 注册文件：

```bash
BRIDGE_CONFIG=/data/config.toml \
BRIDGE_REGISTRATION=/data/registration.yaml \
cargo run -- --generate-registration
```

正常运行：

```bash
BRIDGE_CONFIG=/data/config.toml \
BRIDGE_REGISTRATION=/data/registration.yaml \
cargo run --release
```

## 中文文档

| 文档 | 说明 |
|------|------|
| [快速开始](docs/getting-started.zh.md) | 配置项、注册文件、Synapse 配置、运行方式 |
| [接入指南](docs/integration-guide.zh.md) | 外部服务如何接 webhook / WebSocket / Bridge API |
| [API 参考](docs/api-reference.zh.md) | 所有公开端点、请求结构、回调格式 |
| [架构说明](docs/architecture.zh.md) | crate 结构、dispatcher、权限、relay、存储模型 |
| [加密说明](docs/encryption.zh.md) | 端到桥加密实现、crypto 模式与关键流程 |

## 英文文档

| Document | Description |
|----------|-------------|
| [README](README.md) | English overview and primary index |
| [Getting Started](docs/getting-started.md) | Setup, configuration, registration, and running |
| [Integration Guide](docs/integration-guide.md) | How an external service should integrate |
| [API Reference](docs/api-reference.md) | Public Bridge API and payload formats |
| [Architecture](docs/architecture.md) | Runtime model, storage, routing, permissions |
| [Encryption](docs/encryption.md) | E2BE implementation and crypto modes |

## 开发

```bash
just fmt
just test
just check
```

## License

Apache-2.0
