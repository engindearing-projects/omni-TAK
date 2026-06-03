# Omni-TAK

**Military-Grade TAK Server Aggregator**

OmniTAK is a high-performance, memory-safe TAK (Team Awareness Kit) server aggregator written in Rust. It connects to multiple TAK servers simultaneously, aggregates CoT (Cursor on Target) messages, and provides a unified API for managing tactical data.

## Features

- **Multi-Protocol Support**: TCP, UDP, TLS, WebSocket
- **High Performance**: Handle 10,000+ concurrent connections with <1ms latency
- **Military-Grade Security**: TLS 1.3, client certificates, memory-safe Rust implementation
- **WASM Plugin System**: Extend functionality with sandboxed WebAssembly plugins
- **REST API**: Complete HTTP API for all operations
- **Web Interface**: Modern browser-based control panel
- **Desktop GUI**: Native application with command palette, dark mode, and keyboard shortcuts
- **Rich Mapping**: Interactive maps with drawing tools, Blue Force Tracking, and offline support
- **Natural Language Interface**: Create TAK objects using plain English commands
- **Real-Time Metrics**: Prometheus-compatible metrics and monitoring

## What's New in v0.2.0

### Desktop GUI Enhancements

**Command Palette (Ctrl+K)**
- Quick access to all commands with fuzzy search
- Execute actions without leaving the keyboard
- Organized by category: Navigation, Connections, View, Tools, Settings

**Keyboard Shortcuts**
- Navigate tabs with Ctrl+1 through Ctrl+5
- Ctrl+N to add connections, Ctrl+Shift+N for quick connect wizard
- Ctrl+R to refresh, Ctrl+E to export, Ctrl+I to import
- Full shortcut reference available in Settings

**Dark Mode and Theming**
- Toggle between light and dark themes with Ctrl+Shift+D
- Adjustable UI scale from 0.5x to 2.0x
- Theme preferences persist between sessions

**Interactive Map Features**
- Drawing tools: markers, lines, circles, polygons, range rings
- Measurement tool for distance calculations
- Blue Force Tracking with historical trails and velocity vectors
- Track selection and inspection panel

**Offline Map Support**
- Load MBTiles databases for offline tile storage
- Import GeoJSON files for vector overlays
- Import KML files (Google Earth format)
- Toggle layer visibility and manage multiple overlays

**Quick Connect Wizard**
- Simplified server setup with guided steps
- Automatic certificate configuration
- Connection testing before saving

## Screenshots

### Interactive Map View

![OmniTAK Map View](docs/screenshots/map-view.png)
*Interactive tactical map with real-time positioning and situational awareness*

### Dashboard & Monitoring

<p align="center">
  <img src="docs/screenshots/dashboard.png" width="32%" alt="Dashboard">
  <img src="docs/screenshots/connections.png" width="32%" alt="Connections">
  <img src="docs/screenshots/messages.png" width="32%" alt="Messages">
</p>

*Real-time dashboard, server connections management, and message monitoring*

## Quick Start

### Prerequisites

```bash
# 1. Install Rust (1.90+)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env

# 2. Install Protocol Buffers compiler
# macOS:
brew install protobuf

# Ubuntu/Debian:
sudo apt install protobuf-compiler

# 3. Clone the repository
git clone https://github.com/engindearing-projects/omniTAK.git
cd omniTAK

# 4. (Contributors) enable the pre-commit hook: runs cargo fmt + clippy,
#    the same gate as CI, before each commit.
git config core.hooksPath .githooks
```

### Option A: Docker (fastest)

Spin up the aggregator and its web management UI in one container — no Rust toolchain required:

```bash
docker compose up --build
```

Then open **http://localhost:8080** and sign in:
- Username: `admin`
- Password: `changeme`  (override with `OMNITAK_ADMIN_PASSWORD`)

```bash
# Use your own admin password:
OMNITAK_ADMIN_PASSWORD=your_secure_password docker compose up --build
```

The embedded web UI (login, dashboard, connection management) is served directly by the
binary at the same origin as the API, so there's nothing else to start. This is a test-mode
sample — TLS is off and the default credentials are well-known, so don't expose it to the
public internet as-is.

### Option B: Build from source

### Step 1: Start the Main Server

The main server provides the REST API that all interfaces use.

```bash
# Create a basic config file
mkdir -p config
cat > config/omnitak.yaml << 'EOF'
api:
  bind_addr: "0.0.0.0:9443"
  enable_tls: false
  jwt_expiration: 86400

servers: []

logging:
  level: "info"
EOF

# Build and run the server
cargo run --bin omnitak --release -- \
  --config config/omnitak.yaml \
  --admin-password your_secure_password
```

**Wait for this message:**
```
Server listening address=0.0.0.0:9443
```

The server is now running! Keep this terminal open.

**Default credentials:**
- Username: `admin`
- Password: Whatever you set with `--admin-password`

---

### Step 2: Access the Web Interface

The web interface is a browser-based control panel for managing TAK server connections.

The UI is **embedded in the binary** and served by the server itself at the same address as
the API — just open the server's address in a browser:

**Open your browser:**
- URL: http://localhost:9443  (the `bind_addr` from Step 1)
- Login with: `admin` / `your_secure_password`

**Features:**
- Real-time dashboard with system metrics
- Add/remove TAK server connections
- View connection status
- Monitor message throughput

---

### Step 3 (Optional): Run the Desktop GUI

The native desktop application provides advanced features beyond the web interface, including command palette, keyboard shortcuts, and rich mapping capabilities.

```bash
# Build the GUI
cargo build --bin omnitak-gui --release

# Run it
./target/release/omnitak-gui
```

**Features:**
- Command palette with Ctrl+K for quick actions
- Keyboard shortcuts for efficient navigation
- Dark mode with adjustable UI scale
- Interactive map with drawing tools
- Blue Force Tracking visualization
- Offline map support (MBTiles, GeoJSON, KML)
- Real-time connection monitoring

**Default API:** http://localhost:9443 (configurable at login)

---

### Step 4 (Optional): Use Natural Language Commands

Create TAK objects using plain English commands.

```bash
# Install Python dependencies
cd claude-interface
pip3 install -r requirements.txt

# Run the interactive demo
python3 interactive_demo.py
```

**Example commands:**
```bash
# Create a 5km exclusion zone
circle 34.0522,-118.2437 5 "Exclusion Zone" red

# Create an area of operations
polygon 34.0,-118.0 34.0,-117.0 33.5,-117.0 "AO Alpha" blue

# Create a patrol route
route 34.0,-118.0:Start 34.1,-118.1:Mid 34.2,-118.2:End "Patrol 1" yellow

# Exit
quit
```

---

## Architecture

All interfaces connect to a single REST API backend:

```
┌─────────────────────────────────┐
│   Main Server (Port 9443)       │
│   - REST API                    │
│   - Connection Pool             │
│   - Message Aggregator          │
└────────┬────────────────────────┘
         │ REST API
    ┌────┼────┬────────┐
    │    │    │        │
Web UI  GUI  Python  Mobile
        CLI  (future)
```

## Adding a TAK Server Connection

### Via Web Interface

1. Open http://localhost:8080
2. Login with admin credentials
3. Click "Add Connection"
4. Fill in:
   - **Name**: Friendly name for the connection
   - **Address**: `hostname:port` (e.g., `takserver.local:8089`)
   - **Protocol**: TCP, TLS, UDP, or WebSocket
5. Click "Add Connection"

### Via API

```bash
# 1. Login to get a token
TOKEN=$(curl -s -X POST http://localhost:9443/api/v1/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username":"admin","password":"your_password"}' | \
  python3 -c "import sys, json; print(json.load(sys.stdin)['access_token'])")

# 2. Add a connection
curl -X POST http://localhost:9443/api/v1/connections \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Primary TAK Server",
    "address": "takserver.local:8089",
    "protocol": "tcp"
  }'

# 3. List all connections
curl -H "Authorization: Bearer $TOKEN" \
  http://localhost:9443/api/v1/connections
```

### Via ADB (Android Device)

**NEW!** Automatically pull TAK certificates from your ATAK device via USB:

```bash
# 1. Install Android SDK Platform-Tools
sudo apt-get install android-tools-adb  # Linux
brew install android-platform-tools     # macOS

# 2. Enable USB debugging on your Android device
#    Settings → Developer Options → USB Debugging

# 3. Connect device via USB and accept authorization prompt

# 4. Run the automated script
./examples/adb_pull_certs.sh

# Or use the CLI tool directly
cargo run --bin omnitak-adb-setup -- --output config/config.yaml
```

The ADB integration will:
- ✅ Detect connected ATAK devices
- ✅ Pull certificates from `/sdcard/atak/cert/`
- ✅ Extract TAK server configuration
- ✅ Automatically create a connection
- ✅ Convert PKCS#12 to PEM if needed

**See [ADB_INTEGRATION.md](ADB_INTEGRATION.md) for complete documentation.**

## REST API Endpoints

**Base URL:** `http://localhost:9443`

### Authentication
```bash
POST /api/v1/auth/login
  Body: {"username": "admin", "password": "your_password"}
  Returns: {"access_token": "...", "expires_at": "...", "role": "admin"}
```

### System Status
```bash
GET /api/v1/health        # No auth required
GET /api/v1/status        # Requires auth
GET /api/v1/metrics       # Prometheus metrics
```

### Connection Management
```bash
GET    /api/v1/connections           # List all connections
POST   /api/v1/connections           # Add new connection
GET    /api/v1/connections/:id       # Get connection details
DELETE /api/v1/connections/:id       # Remove connection
```

### CoT Messages
```bash
POST /api/v1/cot/send    # Send CoT message to all connected servers
```

**Full API documentation:** http://localhost:9443/api-docs.html (when server is running)

## Configuration

**Main config file:** `config/omnitak.yaml`

```yaml
# API Server Configuration
api:
  bind_addr: "0.0.0.0:9443"    # API server address
  enable_tls: false             # Use TLS (recommended for production)
  jwt_expiration: 86400         # Token expiration (24 hours)
  rate_limit_rps: 100           # Rate limit requests per second
  enable_swagger: true          # Enable API documentation

# TAK Server Connections (managed via API/UI)
servers: []

# Logging
logging:
  level: "info"                 # trace, debug, info, warn, error
  format: "text"                # text or json

# Metrics
metrics:
  enabled: true
```

## TLS Configuration for TAK Servers

If connecting to a TAK server that requires TLS:

```bash
# 1. Obtain certificates from your TAK server admin
# You'll need:
#   - client.pem (client certificate)
#   - client.key (private key)
#   - ca.pem (CA certificate)

# 2. Convert to PEM format if needed
openssl pkcs12 -in client.p12 -out client.pem -clcerts -nokeys
openssl pkcs12 -in client.p12 -out client.key -nocerts -nodes
openssl rsa -in client.key -out client-rsa.key

# 3. Add via Web UI:
#    - Select "TLS (Secure)" as protocol
#    - Upload certificate files
#    - Enter certificate password if required

# 4. Or add via API with base64-encoded cert data
```

See `docs/ADB_SETUP.md` for automatic certificate extraction from Android devices.

## Building from Source

### Release Build (Optimized)
```bash
# Build everything
cargo build --release

# Build specific components
cargo build --bin omnitak --release         # Main server
cargo build --bin omnitak-gui --release     # Desktop GUI
cargo build --bin omnitak-gen --release     # CoT generator tool
cargo build --bin omnitak-adb-setup --release  # ADB setup tool
```

Binaries will be in `target/release/`

### Development Build
```bash
cargo build
```

## Running Tests

```bash
# Run all tests
cargo test --workspace

# Run specific crate tests
cargo test -p omnitak-cot
cargo test -p omnitak-client
cargo test -p omnitak-pool

# Run with logging
RUST_LOG=debug cargo test
```

## Platform Support

| Platform | Main Server | Web UI | Desktop GUI | Status |
|----------|------------|--------|-------------|--------|
| macOS (Intel) | ✅ | ✅ | ✅ | Tested |
| macOS (Apple Silicon) | ✅ | ✅ | ✅ | Tested |
| Ubuntu 20.04+ | ✅ | ✅ | ✅ | Tested |
| Windows 11 (WSL2) | ✅ | ✅ | ✅ | Tested |
| Windows (Native) | ⏳ | ✅ | ⏳ | In Progress |

## Troubleshooting

### Server won't start

**Port already in use:**
```bash
# Check what's using port 9443
lsof -i :9443

# Kill the process or change port in config.yaml
```

**Permission denied:**
```bash
# Don't use privileged ports (< 1024) without sudo
# Use ports like 8443, 9443 instead
```

### Web UI can't connect

**Check server is running:**
```bash
curl http://localhost:9443/api/v1/health
```

Should return: `{"status":"healthy","timestamp":"..."}`

**CORS issues:**
- Web UI must be served from same host as API, or
- Configure CORS in `config.yaml`

### Authentication errors

**Invalid credentials:**
- Check username is `admin`
- Verify password matches what you set with `--admin-password`

**Token expired:**
- Tokens expire after 24 hours by default
- Login again to get a new token

### Connection to TAK server fails

**TCP connections:**
- Verify TAK server address and port
- Check firewall allows outbound connections
- Test with: `telnet takserver.local 8089`

**TLS connections:**
- Ensure certificates are in correct format (PEM)
- Verify certificate matches server hostname
- Check certificate is not expired
- Enable debug logging: `RUST_LOG=debug cargo run ...`

## Performance Tuning

For high-throughput scenarios:

```yaml
# config.yaml
application:
  max_connections: 10000        # Maximum TAK server connections
  worker_threads: 8             # CPU cores * 2 recommended

performance:
  buffer_size: 16384            # Increase for high message rates
  max_message_size: 2097152     # 2MB max message size
  connection_timeout: 30
  keepalive_interval: 60
```

## Security Best Practices

### Production Deployment

1. **Enable TLS for API:**
```yaml
api:
  enable_tls: true
  tls_cert_path: "/path/to/api-cert.pem"
  tls_key_path: "/path/to/api-key.pem"
```

2. **Use strong passwords:**
```bash
# Generate a random password
openssl rand -base64 32

# Set via environment variable
export OMNITAK_ADMIN_PASSWORD='your_strong_password'
cargo run --bin omnitak --release
```

3. **Enable audit logging:**
```yaml
security:
  audit_logging: true
```

4. **Restrict CORS origins:**
```yaml
security:
  cors_origins:
    - "https://your-dashboard.example.com"
```

5. **Use API keys for automation:**
```bash
# Create an API key via the API
curl -X POST http://localhost:9443/api/v1/auth/api-keys \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"name": "automation-key", "role": "operator"}'
```

## WASM Plugin System

OmniTAK features a powerful plugin system based on WebAssembly Components, allowing you to extend functionality with custom message filters, transformers, and analyzers written in any WASM-compatible language.

### Why WASM Plugins?

- **Sandboxed Execution**: Plugins run in isolated environments with no access to filesystem or network unless explicitly granted
- **Language Agnostic**: Write plugins in Rust, C/C++, Go, Python, or any language that compiles to WASM
- **Near-Native Performance**: WASM compiles to machine code for fast execution
- **Hot-Reload**: Update plugins without restarting the server
- **Type-Safe Interfaces**: WIT (WebAssembly Interface Types) provides strong typing between host and plugins

### Quick Start: Building a Plugin

```bash
# 1. Install the WASM target
rustup target add wasm32-wasip2

# 2. Navigate to the example plugin
cd plugins/example-filter

# 3. Build the plugin
cargo build --target wasm32-wasip2 --release

# 4. The WASM file is ready!
ls target/wasm32-wasip2/release/example_filter.wasm
```

### Example Plugin: Hostile Keyword Filter

The included example plugin demonstrates message filtering by detecting hostile keywords in CoT messages:

```rust
// Detects keywords: hostile, enemy, threat, attack, danger
// Tags detected messages with: hostile_detected="true" keywords="..."

impl Guest for ExampleFilterPlugin {
    fn filter_message(cot_xml: String) -> Result<String, String> {
        // Plugin logic here
        if contains_hostile_keywords(&cot_xml) {
            Ok(tag_as_hostile(cot_xml))
        } else {
            Ok(cot_xml) // Pass through unchanged
        }
    }
}
```

### Creating Your Own Plugin

**1. Create a new plugin project:**
```bash
cargo new --lib my-filter-plugin
cd my-filter-plugin
```

**2. Configure Cargo.toml:**
```toml
[package]
name = "my-filter-plugin"
version = "0.1.0"
edition = "2021"

[dependencies]
wit-bindgen = "0.47.0"

[lib]
crate-type = ["cdylib"]
```

**3. Implement the message filter interface:**
```rust
// src/lib.rs
wit_bindgen::generate!({
    world: "message-filter",
    path: "../wit"
});

struct MyFilterPlugin;

impl Guest for MyFilterPlugin {
    fn filter_message(cot_xml: String) -> Result<String, String> {
        // Your custom filtering logic
        Ok(cot_xml)
    }

    fn get_name() -> String {
        "My Custom Filter".to_string()
    }

    fn get_version() -> String {
        "0.1.0".to_string()
    }
}

export!(MyFilterPlugin);
```

**4. Build and deploy:**
```bash
cargo build --target wasm32-wasip2 --release
cp target/wasm32-wasip2/release/my_filter_plugin.wasm /path/to/omnitak/plugins/
```

### Plugin API (Host Functions)

Plugins can call these functions provided by OmniTAK:

```rust
// Simple logging
log("Processing message...");

// Structured logging with properties
log_structured("Hostile detected", vec![
    ("keyword", "enemy"),
    ("severity", "high")
]);

// Access configuration
if let Some(threshold) = get_config("sensitivity_threshold") {
    // Use config value
}
```

### Plugin Capabilities

Example use cases for plugins:

1. **Geofencing**: Filter messages based on geographic coordinates
2. **Content Sanitization**: Redact sensitive information from messages
3. **Protocol Translation**: Convert between different CoT versions
4. **Rate Limiting**: Throttle messages from specific sources
5. **Threat Detection**: Analyze patterns for security threats
6. **Data Enrichment**: Add metadata from external sources
7. **Message Routing**: Direct messages to specific servers based on content
8. **Compliance**: Enforce organizational policies on message content

### Loading Plugins at Runtime

```rust
// In your application code
use omnitak_core::plugins::PluginManager;

let mut manager = PluginManager::new(
    "plugins/",  // Plugin directory
    config,      // Configuration map
    servers      // Server list
);

// Load all plugins
manager.load_all()?;

// Filter a message through all plugins
if let Some(filtered) = manager.filter_message(&cot_xml, metadata)? {
    // Use filtered message
}

// Hot-reload a specific plugin
manager.reload_plugin("example-filter")?;
```

### Plugin Security

OmniTAK's WASM runtime provides strong security guarantees:

- **Memory Isolation**: Plugins cannot access OmniTAK's memory
- **No System Calls**: Plugins cannot make arbitrary system calls
- **No Network Access**: Plugins cannot open sockets or make HTTP requests
- **No File Access**: Plugins cannot read/write files (unless via WASI with explicit permissions)
- **Resource Limits**: Memory and CPU usage can be capped per plugin
- **Deterministic Execution**: No threads or timers available to plugins

### Performance

- **Fast Load Times**: Plugins load in milliseconds
- **Low Overhead**: Minimal performance impact on message processing
- **Compiled Code**: WASM compiles to native machine code
- **Example Metrics**: Example filter processes 100,000 messages/sec

### Documentation

For detailed plugin development guides:
- [Plugin Development Guide](plugins/example-filter/README.md)
- [Build Instructions](plugins/example-filter/BUILD.md)
- [Usage Examples](plugins/example-filter/EXAMPLES.md)
- [WIT Interface Reference](wit/omnitak-plugins.wit)

## Documentation

- **API Docs:** http://localhost:9443/api-docs.html (when server running)
- **Setup Guides:**
  - [macOS Setup](SETUP_MACOS.md)
  - [Ubuntu Setup](SETUP_UBUNTU.md)
  - [Windows Setup](SETUP_WINDOWS.md)
- **Feature Guides:**
  - [ADB Certificate Setup](docs/ADB_SETUP.md)
  - [GUI Features](docs/GUI_FEATURES.md)
  - [Filtering Guide](docs/FILTERING.md)
  - [Plugin Development](plugins/example-filter/README.md)

## Contributing

Contributions welcome! Please:

1. Fork the repository
2. Create a feature branch
3. Follow Rust style guidelines (`cargo fmt`)
4. Pass all tests (`cargo test`)
5. Pass clippy lints (`cargo clippy`)
6. Update documentation
7. Submit a Pull Request

## License

Licensed under either of:
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.

## Support

- **Issues:** [GitHub Issues](https://github.com/engindearing-projects/omniTAK/issues)
- **Documentation:** See docs/ directory

---

**Built with Rust for reliability and performance in tactical environments.**
