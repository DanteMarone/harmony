# Harmony

A small, private, self-hosted chat app for a group of friends: direct messages,
group chats and voice calls, with end-to-end encryption. One Rust program, one
SQLite file, no third-party services.

**Status:** early development. Accounts and invites work; messaging and voice are next.

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
| `server/` | Rust server (axum, SQLite via sqlx): API and static file hosting |
| `server/migrations/` | Database schema |
| `web/` | Web app (Svelte + TypeScript, built with Vite) |

## Running locally

You need [Rust](https://rustup.rs) and [Node.js](https://nodejs.org) 22 or newer.

```sh
# 1. Build the web app
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

While working on the web app, run `npm run dev` in `web/` for live reload; it
forwards API calls to the server on port 8080.

## Tests

```sh
cd server && cargo test
cd web && npm run check
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
