# OmniTAK + iTAK Quick Start Guide

This guide walks you through connecting your iPhone (iTAK) to a TAK server using OmniTAK as a bridge, or creating a data package to connect iTAK directly.

## Prerequisites

- macOS, Linux, or Windows with WSL
- Rust toolchain installed (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)
- Your TAK server credentials (P12 certificate file and CA certificate)
- iPhone with iTAK installed

## Part 1: Install and Build OmniTAK

### Step 1: Clone the repository

```bash
git clone https://github.com/engindearing-projects/omniTAK.git
cd omniTAK
```

### Step 2: Build the release binary

```bash
cargo build --release
```

This takes a few minutes. The binary will be at `./target/release/omnitak`.

## Part 2: Prepare Your Certificates

You need three things from your TAK server:
1. **Client certificate** (`.p12` file) - identifies you to the server
2. **CA certificate** (`.pem` or in the `.p12`) - verifies the server
3. **Certificate password** - usually `atakatak` for TAK servers

### If you have a P12 file from your TAK server:

Copy it to the certs folder:

```bash
mkdir -p certs
cp /path/to/your-certificate.p12 certs/client.p12
cp /path/to/ca.pem certs/ca.pem
```

### If you need to extract PEM from P12:

```bash
# Extract client certificate
openssl pkcs12 -in certs/client.p12 -out certs/client.pem -clcerts -nokeys -passin pass:atakatak -legacy

# Extract private key
openssl pkcs12 -in certs/client.p12 -out certs/client-key.pem -nocerts -nodes -passin pass:atakatak -legacy

# Convert key to RSA format (required)
openssl rsa -in certs/client-key.pem -out certs/client-key-rsa.pem
```

## Part 3: Create Configuration File

Create a file called `config-myserver.yaml`:

```bash
cat > config-myserver.yaml << 'EOF'
# OmniTAK Configuration
application:
  max_connections: 50
  worker_threads: 4

# Your TAK Server Connection
servers:
  - id: my-tak-server
    name: "My TAK Server"
    address: "YOUR_SERVER_ADDRESS:8089"    # <-- CHANGE THIS
    protocol: tls
    auto_reconnect: true
    reconnect_delay_ms: 5000
    tls:
      cert_path: "certs/client.p12"        # Path to your P12 file
      key_path: "certs/client.p12"         # Same path for P12
      ca_path: "certs/ca.pem"              # Path to CA certificate
      verify_server: false                  # Set to true in production

# Listener for iTAK connections (local network)
listeners:
  - id: tcp-listener
    enabled: true
    bind_addr: "0.0.0.0:8087"
    protocol: tcp
    max_connections: 20

# API settings
api:
  bind_addr: "127.0.0.1:9443"
  enable_tls: false

auth:
  admin_user: "admin"
  admin_password: "changeme"    # <-- CHANGE THIS

logging:
  level: "info"

filters:
  mode: whitelist
  rules:
    - id: allow-all
      type: affiliation
      allow: [friend, assumedfriend, hostile, neutral, unknown, pending]
      destinations: [my-tak-server]
EOF
```

**Edit the file** and replace:
- `YOUR_SERVER_ADDRESS:8089` with your actual TAK server address
- Update certificate paths if different
- Change the admin password

## Part 4: Start OmniTAK

```bash
./target/release/omnitak --config config-myserver.yaml
```

You should see output like:
```
INFO omnitak: TCP listener 'tcp-listener' started successfully
INFO omnitak: Connecting to TAK server: my-tak-server
INFO omnitak: TLS handshake successful
INFO omnitak: Successfully connected to TAK server: my-tak-server
```

If you see "TLS handshake successful" - you're connected!

### Troubleshooting Connection Issues

**"Certificate not trusted" error:**
- Make sure `verify_server: false` is in your config
- Check that your P12 password is correct (try `atakatak` or empty)

**"Connection refused" error:**
- Verify the server address and port
- Check firewall settings
- Confirm the TAK server is running

## Part 5: Create iTAK Data Package

Now create a ZIP file that iTAK can import to connect directly to the TAK server.

### Step 1: Create a working directory

```bash
mkdir -p datapackage-iphone
cd datapackage-iphone
```

### Step 2: Copy your certificates

```bash
cp ../certs/client.p12 ./iphone.p12
cp /path/to/truststore-root.p12 ./truststore-root.p12
```

If you don't have a truststore P12, create one from your CA PEM:
```bash
# Skip if you already have truststore-root.p12
openssl pkcs12 -export -nokeys -in ../certs/ca.pem -out truststore-root.p12 -passout pass:atakatak -name "TAK-CA"
```

### Step 3: Create server-connection.pref

```bash
cat > server-connection.pref << 'EOF'
<?xml version='1.0' standalone='yes'?>
<preferences>
  <preference version="1" name="cot_streams">
    <entry key="count" class="class java.lang.Integer">1</entry>
    <entry key="description0" class="class java.lang.String">TAK Server</entry>
    <entry key="enabled0" class="class java.lang.Boolean">true</entry>
    <entry key="connectString0" class="class java.lang.String">YOUR_SERVER_ADDRESS:8089:ssl</entry>
  </preference>
  <preference version="1" name="com.atakmap.app_preferences">
    <entry key="displayServerConnectionWidget" class="class java.lang.Boolean">true</entry>
    <entry key="caLocation" class="class java.lang.String">/storage/emulated/0/atak/cert/truststore-root.p12</entry>
    <entry key="certificateLocation" class="class java.lang.String">/storage/emulated/0/atak/cert/iphone.p12</entry>
    <entry key="clientPassword" class="class java.lang.String">atakatak</entry>
  </preference>
</preferences>
EOF
```

**Edit** and replace `YOUR_SERVER_ADDRESS:8089:ssl` with your actual server.

### Step 4: Create manifest.xml

```bash
cat > manifest.xml << 'EOF'
<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<MissionPackageManifest version="2">
   <Configuration>
      <Parameter name="uid" value="itak-server-config"/>
      <Parameter name="name" value="iTAK Server Configuration"/>
      <Parameter name="onReceiveDelete" value="true"/>
   </Configuration>
   <Contents>
      <Content ignore="false" zipEntry="iphone.p12"/>
      <Content ignore="false" zipEntry="truststore-root.p12"/>
      <Content ignore="false" zipEntry="server-connection.pref"/>
   </Contents>
</MissionPackageManifest>
EOF
```

### Step 5: Create the ZIP file

```bash
zip -r ../itak-config.zip manifest.xml iphone.p12 truststore-root.p12 server-connection.pref
cd ..
```

### Step 6: Verify the ZIP

```bash
unzip -l itak-config.zip
```

Should show:
```
  Length      Date    Time    Name
---------  ---------- -----   ----
      xxx  xx-xx-xxxx xx:xx   manifest.xml
      xxx  xx-xx-xxxx xx:xx   iphone.p12
      xxx  xx-xx-xxxx xx:xx   truststore-root.p12
      xxx  xx-xx-xxxx xx:xx   server-connection.pref
```

## Part 6: Transfer ZIP to iPhone

### Option A: Quick HTTP Server (Recommended)

Start a simple web server:
```bash
python3 -m http.server 8888 --bind 0.0.0.0
```

Find your computer's IP:
```bash
# macOS
ipconfig getifaddr en0

# Linux
hostname -I | awk '{print $1}'
```

On your iPhone Safari, go to:
```
http://YOUR_COMPUTER_IP:8888/itak-config.zip
```

### Option B: AirDrop
Right-click the ZIP file and AirDrop to your iPhone.

### Option C: Email
Email the ZIP file to yourself and open on iPhone.

## Part 7: Import into iTAK

1. Download/receive the ZIP file on your iPhone
2. Open **iTAK**
3. Go to **Settings** (gear icon)
4. Tap **Data Package**
5. Tap **Import** and select the ZIP file
6. When prompted for password, enter: `atakatak`
7. The server connection will be configured automatically

## Part 8: Verify Connection

In iTAK:
1. Go to **Settings** → **Network Preferences** → **Manage Server Connections**
2. You should see your TAK server listed
3. Tap to connect if not already connected
4. The connection indicator should turn green

## Quick Reference

| Item | Default Value |
|------|--------------|
| Certificate password | `atakatak` |
| TAK streaming port | `8089` |
| TAK API port | `8443` |
| OmniTAK API port | `9443` |
| OmniTAK listener port | `8087` |

## File Structure Summary

```
omniTAK/
├── target/release/omnitak     # Built binary
├── config-myserver.yaml       # Your config file
├── certs/
│   ├── client.p12            # Your TAK certificate
│   └── ca.pem                # CA certificate
├── datapackage-iphone/
│   ├── iphone.p12
│   ├── truststore-root.p12
│   ├── server-connection.pref
│   └── manifest.xml
└── itak-config.zip           # Final package for iTAK
```

## Running OmniTAK as a Background Service

To keep OmniTAK running after closing the terminal:

```bash
# Start in background
nohup ./target/release/omnitak --config config-myserver.yaml > omnitak.log 2>&1 &

# Check if running
ps aux | grep omnitak

# View logs
tail -f omnitak.log

# Stop
pkill omnitak
```

## Need Help?

- Check logs for error messages
- Verify certificate passwords
- Ensure TAK server is reachable (`ping YOUR_SERVER_ADDRESS`)
- Test port connectivity (`nc -zv YOUR_SERVER_ADDRESS 8089`)
