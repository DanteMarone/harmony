# Harmony

A small, private, self-hosted chat app for a group of friends: direct messages,
group chats and voice calls, with end-to-end encryption. One Rust program, one
SQLite file, no third-party services.

**Status:** early development. Accounts, invites and end-to-end encrypted direct
messages and group chats work. File sharing and voice calls are next. Read
[how the encryption works](docs/encryption.md), including what is not done yet.

## Principles

- **Private by design.** A username and password are all Harmony asks for. No
  email, no phone number, no ID, no telemetry.
- **End-to-end encrypted.** Messages, files and calls are encrypted on your
  device. The server stores and forwards data it cannot read.
- **Light.** The server is a single program that runs comfortably on a small VPS.
- **Yours.** All data lives in one folder you can back up or delete.

## Layout

| Path | What |
| --- | --- |
| `server/` | Rust server (axum, SQLite via sqlx): API, real-time notices and static file hosting |
| `server/migrations/` | Database schema |
| `crypto/` | Encryption layer: a thin wrapper around OpenMLS, compiled to WebAssembly |
| `web/` | Web app (Svelte + TypeScript, built with Vite) |
| `docs/` | Design notes |

## Running locally

You need [Rust](https://rustup.rs) and [Node.js](https://nodejs.org) 22 or newer,
plus two one-time additions so the encryption code can be built for the browser:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.130 --locked
```

Then:

```sh
# 1. Build the web app (this also builds the encryption module)
cd web
npm install
npm run build

# 2. Run the server (from the server folder)
cd ../server
HARMONY_COOKIE_SECURE=false cargo run
```

On Windows PowerShell, set the variable first: `$env:HARMONY_COOKIE_SECURE="false"; cargo run`.

Open <http://localhost:8080>. On first start the server prints a one-time
invite link in the console; open it to create the admin account. The admin can
then create invite links for friends.

While working on the web app, run `npm run wasm` once and then `npm run dev` in
`web/` for live reload; it forwards API calls to the server on port 8080.

Harmony can only be open in one tab per account at a time, because two tabs
sharing one set of encryption keys would corrupt them.

## Tests

```sh
cd crypto && cargo test     # the encryption layer, with several simulated devices
cd server && cargo test     # the API, including full encrypted conversations
cd web && npm run check     # type checks
```

## Configuration

All settings are environment variables; see [`.env.example`](.env.example).

## Keeping secrets out of this repository

This repository is public. The rules:

- No passwords, keys, tokens, server addresses or personal details in code,
  comments, commit messages, tests or docs.
- Configuration comes from environment variables. `.env` files, the `data/`
  folder, databases, keys and logs are git-ignored.
- Invite codes and session tokens are generated at runtime and stored only as
  hashes; they never appear in logs.
- CI scans every push for accidentally committed secrets.

## Licence

Copyright (C) 2026 DanteMarone and contributors.

Harmony is free software, released under the
[GNU Affero General Public License, version 3](LICENSE). You may use, change
and share it; if you run a changed version for other people, you must make
your changes available to them under the same licence.
