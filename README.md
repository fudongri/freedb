# FreeDB

[English](README.md) | [中文](README_zh.md)

---

Cross-platform desktop database client for MySQL and PostgreSQL.

FreeDB is a lightweight, fast database client built with Rust and [egui](https://github.com/emilk/egui/). It runs on macOS, Windows, and Linux.

### Features

- **Multi-database** — MySQL and PostgreSQL support
- **Query editor** — SQL editor with syntax highlighting, autocomplete, and multi-statement execution
- **Multiple tabs** — work with several queries and connections simultaneously
- **Connection management** — save, edit, organize, and group database connections
- **Connection pooling** — cached connections with health checks and auto-retry
- **Table explorer** — browse tables, views, and schemas in the sidebar
- **Table views** — data preview, structure, indexes, and DDL
- **Data filtering** — multi-clause filters with AND/OR and rich operators
- **Inline editing** — edit cells, insert rows, delete rows directly
- **Saved queries** — save, rename, and organize frequently used SQL
- **Query history** — persistent history with execution time tracking
- **Copy options** — copy as INSERT statements, TSV, or export to CSV
- **Dark mode** — built-in light and dark themes
- **Zoom** — adjustable zoom level (0.5x – 3.0x)
- **Cross-platform** — macOS, Windows, and Linux builds

### Install

**macOS (Homebrew)**

```bash
brew install --cask fudongri/tap/freedb
```

**Windows**

Download the installer or portable ZIP from [Releases](https://github.com/fudongri/freeDB/releases).

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (edition 2024)

### Build

```bash
git clone https://github.com/fudongri/freedb.git
cd freedb
cargo build --release
```

### Run

```bash
cargo run --release
```

Or run the binary directly:

```bash
./target/release/freedb
```

### Data Location

Connection profiles and history are stored locally, never uploaded. Password storage is plaintext for now — treat it as a local developer tool.

| Platform | Path |
|----------|------|
| macOS | `~/Library/Application Support/freedb/` |
| Windows | `C:\Users\<user>\AppData\Local\freedb\` |
| Linux | `$XDG_DATA_HOME/freedb/` or `~/.local/share/freedb/` |

| File | Content |
|------|---------|
| `freedb.sqlite3` | Connection profiles, query history, UI state |
| `credentials.json` | Saved passwords |

### Project Structure

```
freedb/
├── apps/desktop/          # Desktop GUI application (egui/eframe)
├── crates/
│   ├── app-services/      # Application service layer
│   ├── connection-pool/   # Connection pooling
│   ├── connection-store/  # Saved connection persistence
│   ├── core-domain/       # Shared domain types
│   ├── driver-api/        # Database driver abstraction
│   ├── driver-mysql/      # MySQL driver
│   ├── driver-postgres/   # PostgreSQL driver
│   ├── export-service/    # CSV/data export
│   ├── history-store/     # Query history
│   ├── secure-store/      # Credential storage
│   ├── session-manager/   # Session lifecycle
│   └── ssh-tunnel/        # SSH tunnel support
└── scripts/               # Build and packaging scripts
```

### Star History

[![Star History Chart](https://api.star-history.com/svg?repos=fudongri/freeDB&type=Date)](https://star-history.com/#fudongri/freeDB&Date)

### License

MIT
