# nyarix-runtime

[![CI](https://github.com/F000NKKK/nyarix-runtime/actions/workflows/ci.yml/badge.svg)](https://github.com/F000NKKK/nyarix-runtime/actions/workflows/ci.yml)
[![License: AGPL v3](https://img.shields.io/badge/license-AGPL--3.0--or--later-blue.svg)](LICENSE)

Runtime and core platform for **Nyarix** — a modular runtime for executing
network stacks as graphs, not monolithic protocols. This repository is the
heart of the platform: the Flow Graph engine, the module contract every
transport/crypto/obfuscation/policy module implements, the packet model
that flows between them, and the machinery that ties it all together.

Nyarix is not a VPN. It's the runtime a VPN, proxy, relay, mesh overlay, or
gateway can all be *configurations of*. See [`view.md`](view.md) for the
full architectural vision and [`BACKLOG.md`](BACKLOG.md) for the milestone
breakdown this repo is built against.

## Workspace layout

```text
nyarix-runtime/
├── apps/
│   └── runtime-test/     # manual smoke-test binary
└── crates/
    ├── nyarix-core        # shared types: typed IDs, versioning, platform detection
    ├── nyarix-error       # layered error types (config/package/module/graph/runtime/...)
    ├── nyarix-config      # RuntimeConfig: global → profile → stack → device hierarchy
    ├── nyarix-packet      # Packet: payload + metadata + tags, zero-copy, poolable
    ├── nyarix-module-api  # the Module/Node contract every module implements
    ├── nyarix-graph       # Flow Graph engine: nodes, edges, execution
    └── nyarix-runtime     # module loader, scheduler, event bus — the Runtime itself
```

Each crate does one job. The Runtime never knows what protocol it's
running — it only knows how to load modules, wire them into a graph, and
push packets through it.

## Status

Early development (M0–M3 of the [backlog](BACKLOG.md)). The workspace
builds and its test suite passes, but most crates are intentionally partial:
gaps are documented inline (`// see #NN`) and tracked as GitHub issues
rather than guessed at ahead of time. Don't expect a running VPN yet —
expect a Packet model, a Module API, and the start of a Graph engine.

## Building

Requires a recent stable Rust toolchain (`rust-version` is pinned in the
workspace `Cargo.toml`).

```sh
cargo build --workspace
cargo test --workspace
```

## Development

```sh
cargo fmt --all              # formatting (see rustfmt.toml)
cargo fmt --all -- --check   # CI's formatting check
cargo clippy --workspace --all-targets
```

Lints are configured at `warn`, not `deny`, at this stage of development
(see the comment in the workspace `Cargo.toml`) — CI runs clippy but won't
fail the build on lint warnings yet.

## License

Licensed under the [GNU Affero General Public License v3.0 or later](LICENSE).
