# How Harmony's encryption works

Harmony encrypts messages on the sender's device and decrypts them on the
recipients' devices. The server stores and forwards ciphertext it cannot read.

This page describes what is built today, what the server can and cannot see,
and what is not done yet. It is meant to be honest rather than reassuring.

## Building blocks

- **MLS** ([RFC 9420](https://www.rfc-editor.org/rfc/rfc9420)), the IETF
  standard for end-to-end encrypted group messaging, through the
  [OpenMLS](https://github.com/openmls/openmls) library.
- Ciphersuite `MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519`, the one every
  MLS implementation must support.
- Harmony contains no cryptography of its own. `crypto/` is a thin wrapper
  around OpenMLS, compiled to WebAssembly for the browser. The same code runs
  natively in the test suites.

## Concepts

- A **device** is one browser (later, one app install). Each device creates
  its own keys on first use. Private keys never leave the device.
- A **chat** (DM or group) is one MLS group. Its group id is the chat id.
- A device's MLS identity is `u<user id>:d<device id>`. When a device adds
  another to a chat, it checks that the key package it was handed really
  carries the identity the server claimed, so a key package cannot be passed
  off under another device's name.
- **Key packages** are public, single-use tokens a device uploads so that
  others can add it to a chat while it is offline. A reusable "last resort"
  package covers the case where the single-use ones run out.

## Life of a chat

1. Someone creates a chat. Their device starts the MLS group alone.
2. The server keeps the member list. Any member's device that notices a
   device missing from the key group (a new member, or a member's new device)
   claims a key package for it and posts a **commit** adding it, plus a
   **welcome** that only the added device can open.
3. Devices that should no longer have the keys (a removed member) are removed
   by a commit the same way. From that point they cannot read new messages,
   even with everything they previously held.
4. Every event carries the **epoch** (key generation) it was made in. The
   server refuses events made in an old epoch, so all devices apply key
   changes in the same order. A refused device catches up and retries.
5. If a device loses track of a chat's keys, it asks to be let back in;
   another member's device removes its old entry and adds it fresh.

## What the server stores

| Data | Readable by the server? |
| --- | --- |
| Message text | No |
| Key changes (commits, welcomes) | No, beyond their size |
| Usernames and display names | Yes |
| Who is in which chat, and group names | Yes |
| Which device sent an event, when, and how large it was | Yes |
| Password | No: only an Argon2id hash |
| Invite codes and session tokens | No: only SHA-256 hashes |

## What the device stores

A device keeps its private keys and its decrypted messages in the browser's
own storage (IndexedDB). MLS destroys message keys after use, which protects
old messages if a key leaks later, and also means the server's copy can never
be decrypted a second time. The local copy is the only readable one.

## Properties you get

- **Confidentiality:** only devices in the chat's key group can read messages.
- **Authenticity:** the sender shown for a message comes from its signature,
  not from the server's say-so.
- **Forward secrecy:** keys for past messages are deleted after use.
- **Removal works:** a removed member's devices cannot read later messages.
- **New members cannot read earlier messages.**

## Not done yet

These are known gaps, in rough priority order.

1. **No verification of devices.** The server tells devices which devices
   belong to which user. A malicious or compromised server could add an extra
   device to an account and have it added to that person's chats. Planned:
   safety numbers that friends compare once, and a warning when a contact's
   device list changes.
2. **No history on a new device, and no recovery.** A device only reads
   messages sent after it joined a chat. Clearing browser data loses that
   browser's history. Planned: an encrypted backup unlocked by a recovery
   passphrase.
3. **No device management.** Old devices stay in chats until removed.
   Planned: a device list with sign-out and removal.
4. **Group names are visible to the server.**
5. **Keys at rest are not encrypted** beyond what the browser and operating
   system provide.
6. **The web app is delivered by the server.** Someone in control of the
   server could serve altered code. A strict Content-Security-Policy limits
   injection, and the planned desktop app ships its code separately.
7. **No independent security review** has been done.
