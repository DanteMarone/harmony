# Running Harmony on a server

Harmony runs as one small program behind a web server that handles HTTPS.
These steps assume a Linux VPS (x86-64) with root access and a domain name you
control. Replace `harmony.example.com` with your own subdomain throughout.

## 1. DNS

Point an `A` record (and `AAAA` if the server has IPv6) for your subdomain at
the server. Wait until `dig +short harmony.example.com` returns the server's
address before asking for a certificate.

## 2. Install the server

Releases are built by GitHub Actions (see `.github/workflows/release.yml`), so
nothing is compiled on the server.

```sh
curl -fsSLO https://raw.githubusercontent.com/DanteMarone/harmony/main/deploy/install.sh
sudo bash install.sh v0.1.0
```

The script creates a `harmony` system user, installs the release under
`/opt/harmony/releases/<tag>`, points `/opt/harmony/current` at it, writes
settings to `/etc/harmony/harmony.env` (first run only) and starts the
`harmony` systemd service. The database lives in `/var/lib/harmony`. The
server only listens on `127.0.0.1:8080`.

To update, run the script again with the new tag. To roll back, run it with
the old tag.

## 3. HTTPS

Use whichever web server is already on the machine.

- **Caddy** (simplest if nothing else is installed): add the block from
  `deploy/Caddyfile` to `/etc/caddy/Caddyfile` and run
  `systemctl reload caddy`. Caddy fetches the certificate itself.
- **nginx**: copy `deploy/nginx.conf` to `/etc/nginx/sites-available/harmony`,
  link it into `sites-enabled`, run `nginx -t && systemctl reload nginx`, then
  `certbot --nginx -d harmony.example.com`.

Both pass WebSockets (`/api/ws`) through, which Harmony needs for live updates.

## 4. Firewall

Only the web server should be reachable from outside. With `ufw`, after
checking that your SSH port is already allowed (`ufw status`):

```sh
ufw allow 80/tcp
ufw allow 443/tcp
```

Do not open port 8080; Harmony is reached through the web server.

## 5. First admin account

On its very first start, with no accounts yet, the server prints a one-time
invite link (also shown at the end of `install.sh`):

```sh
journalctl -u harmony -o cat | grep '/#/invite/'
```

Open `https://harmony.example.com/#/invite/<code>` to create the admin account.
The link works once and expires after 7 days. The admin can then invite
friends from the app.

## Backups

Everything is in `/var/lib/harmony`. Stop the service (or use
`sqlite3 /var/lib/harmony/harmony.db ".backup backup.db"`) and copy the folder.
The database holds only encrypted messages, but it does hold account data, so
keep backups private.

## Voice calls

Voice uses a self-hosted [LiveKit](https://livekit.io) server on the same
machine. Follow the steps in [voice.md](voice.md#setting-up-livekit); in short:

- `livekit-server` listens for signalling on `127.0.0.1:7880` only, run by
  `deploy/livekit.service` with its config in `/etc/livekit.yaml`.
- The web server sends `/rtc*` on the Harmony site to it (already in
  `deploy/Caddyfile`).
- The firewall opens the media ports:

  | Port | Protocol | Purpose |
  | --- | --- | --- |
  | 7881 | TCP | Media over TCP, for networks that block UDP |
  | 50000-50100 | UDP | Media |

- `/etc/harmony/harmony.env` gets `HARMONY_LIVEKIT_URL`,
  `HARMONY_LIVEKIT_API_KEY` and `HARMONY_LIVEKIT_API_SECRET`.

The API secret is generated on the server and stored only in
`/etc/livekit.yaml` and `/etc/harmony/harmony.env` there, never in this
repository.
