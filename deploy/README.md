# Shardrail Relay & Web Console Deployment

This directory contains deployment assets for hosting the self-hosted Shardrail Blind WebSocket Relay (`shardrail-relay`) and Browser SPA console behind Nginx with TLS on a Linux VPS (e.g. ForkHorizon VPS at `143.14.22.61`).

## Architecture Guarantees

- **D67 Outbound Only**: Developer daemons (`shardraild`) make outbound-only WebSocket connections to the relay (`wss://shardrail.forkhorizon.com/ws`). No inbound open ports, port forwarding, or NAT traversal on developer machines required.
- **D69 End-to-End Encryption**: All protocol commands, outputs, terminal streams, and approvals are signed and encrypted between the browser and the daemon. The relay cannot decrypt or inspect payloads.
- **D70 Blind Routing**: The relay operates purely by envelope routing keyed on `machine_id` and `client_id`. It never parses application contents or cryptographic keys.

---

## 1. Building the Artifacts

### A. Relay Binary (Linux x86_64)
From the Shardrail repository root:
```bash
# Direct build on a Linux VPS:
cargo build --release -p shardrail_relay
# Or cross-compile from macOS:
cargo build --release -p shardrail_relay --target x86_64-unknown-linux-gnu
```
The compiled binary will be at `target/release/shardrail-relay` (or `target/x86_64-unknown-linux-gnu/release/shardrail-relay`).

### B. Web Console SPA
```bash
npm --prefix apps/web ci
npm --prefix apps/web run build
```
Static SPA files are generated in `apps/web/dist/`.

---

## 2. Server Installation (VPS)

### A. Install Relay Binary and Systemd Service
1. Create system user:
   ```bash
   sudo useradd -r -s /usr/sbin/nologin shardrail
   ```
2. Copy binary:
   ```bash
   sudo cp target/release/shardrail-relay /usr/local/bin/shardrail-relay
   sudo chmod +x /usr/local/bin/shardrail-relay
   ```
3. Install configuration and systemd service:
   ```bash
   sudo cp deploy/shardrail-relay.service /etc/systemd/system/shardrail-relay.service
   sudo mkdir -p /etc/default
   echo 'SHARDRAIL_RELAY_LISTEN=127.0.0.1:4000' | sudo tee /etc/default/shardrail-relay
   sudo systemctl daemon-reload
   sudo systemctl enable --now shardrail-relay
   ```
4. Verify service status:
   ```bash
   sudo systemctl status shardrail-relay
   ```

### B. Install Web Console Static Files
```bash
sudo mkdir -p /opt/shardrail/dist
sudo cp -r apps/web/dist/* /opt/shardrail/dist/
sudo chown -R www-data:www-data /opt/shardrail/dist
```

### C. Nginx & TLS (Certbot)
1. Point DNS for `shardrail.forkhorizon.com` to VPS IP (`143.14.22.61`).
2. Obtain Let's Encrypt certificate:
   ```bash
   sudo certbot certonly --webroot -w /var/www/html -d shardrail.forkhorizon.com
   ```
3. Install Nginx configuration:
   ```bash
   sudo cp deploy/nginx-shardrail.conf /etc/nginx/sites-available/shardrail
   sudo ln -s /etc/nginx/sites-available/shardrail /etc/nginx/sites-enabled/
   sudo nginx -t && sudo systemctl reload nginx
   ```

---

## 3. Connecting Developer Daemons

On the developer machine, launch `shardraild` with the outbound relay dialer enabled:

```bash
shardraild start \
  --relay-url wss://shardrail.forkhorizon.com/ws \
  --relay-machine-id 01950e3d-7b24-7df0-940e-36c5b0586e37
```

*(Optional: use `--relay-auth-token <secret>` if relay authentication is enabled).*

---

## 4. Connecting from Any Browser

Open the browser to the hosted web console:

```text
https://shardrail.forkhorizon.com/?relay=wss://shardrail.forkhorizon.com/ws&machine=01950e3d-7b24-7df0-940e-36c5b0586e37
```

The browser client connects over WSS to the relay, awaits confirmation that the target machine is online, performs the mutual Ed25519 cryptographic handshake directly with the daemon, and establishes the secure channel for session management, turn commands, and streaming output.
