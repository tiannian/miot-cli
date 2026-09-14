# miot-cli

`miot-cli` is a Rust-based command-line tool and asynchronous SDK for MIoT devices. It provides a unified interface for authentication, device discovery, property reads and writes, device-state subscriptions, and MIoT action invocation.

The repository has two crates:

- `miot-rs`: an asynchronous Rust SDK for embedding in applications. Its Rust crate name is `miot_rs`;
- `miot`: a CLI built as a thin layer on top of `miot-rs`, suitable for terminals, scripts, and automation.

> The project is currently in its architecture and bootstrap phase. The commands and APIs below describe the intended public interface; they are not all implemented yet.

## Planned Features

- OAuth login, automatic token refresh, and secure credential storage;
- Home and device discovery, device details, and MIoT Spec lookup;
- MIoT property reads and writes;
- Property-change, device-event, and availability subscriptions;
- MIoT action invocation;
- Unified routing across cloud, local LAN, and central-hub transports.

## CLI Examples

```bash
# Sign in
miot login --region cn

# List devices
miot devices list

# Inspect a device and its capabilities
miot devices get <did>
miot spec get <did>

# Read or write a property
miot props get <did> <siid> <piid>
miot props set <did> <siid> <piid> --value true

# Stream property updates as NDJSON
miot props watch <did> --siid 2 --piid 1

# Invoke a device action
miot actions invoke <did> <siid> <aiid> --input '["hello", true]'
```

Read-oriented commands will support `--format table|json|yaml`. Watch commands will emit NDJSON by default, making them easy to consume through `jq`, log collectors, and shell pipelines.

## SDK Example

```rust,no_run
use miot_rs::{MiotClient, PropertyId, TransportPreference};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = MiotClient::builder()
        .credentials_from_default_store()
        .transport_preference(TransportPreference::Auto)
        .build()
        .await?;

    for device in client.devices().list().await? {
        println!("{}: {}", device.did, device.name);
    }

    let did = "device-id";
    let property = PropertyId::new(2, 1);
    let value = client.properties().get(did, property).await?;
    println!("value: {value}");

    client.properties().set(did, property, json!(true)).await?;
    Ok(())
}
```

## Architecture

The repository is a Rust workspace. The SDK contains the implementation; the CLI only consumes the SDK's public API.

```text
CLI / Rust application
       │
miot-rs
       │
Transport router: Cloud / LAN / Gateway
       │
MIoT services and devices
```

See the design documents for the module boundaries, authentication flow, routing policy, data model, error handling, and delivery plan:

- [0001 SDK 架构设计](docs/0001-sdk-architecture.md)
- [0002 CLI 与交付设计](docs/0002-cli-delivery.md)

## Planned Layout

```text
.
├── crates/
│   ├── miot-rs/        # Async Rust SDK
│   └── miot/           # CLI
├── docs/
│   ├── 0001-sdk-architecture.md
│   └── 0002-cli-delivery.md
├── examples/
└── tests/
```

## Delivery Plan

1. Create the workspace, public data model, and error model.
2. Implement OAuth, credential storage, cloud device discovery, and Spec lookup.
3. Implement cloud property access, actions, and subscriptions as the MVP.
4. Add LAN and central-hub transports incrementally.

## Development Checks

Once implementation begins, run the following before committing:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```
