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
| Each person's public identity key and device signatures | Yes (they are public) |
| History backup | No: encrypted on the device with a key from the recovery code |
| Recovery code | Never sent to the server |
| Which messages a person has backed up, and their size | Yes |
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

## Recovery code, verification and backup

Each person gets a **recovery code** the first time they sign in: 160 random
bits, shown once as 32 letters and digits. It never leaves their devices.
Two keys are derived from it (HKDF-SHA256):

- An **identity key** (Ed25519). Its public half is published. When a person
  types their code on a device, the device's key is signed with it. Other
  people's devices read the device list from the chat's own key group, not
  from the server, and check each device against its owner's identity key.
  A device the owner never confirmed, including one slipped in by the
  server, shows a warning in the chat.
- A **backup key** (AES-256-GCM). Each device uploads an encrypted copy of
  every message it could read. A new device with the code downloads and
  decrypts them. Each record is bound to its owner, chat and position, so
  the server cannot move or swap records unnoticed.

A device remembers the first identity key it sees for each person. If that
key later changes, the chat shows a warning until the person looks at it.
Two people can compare a **safety number** (sixty digits derived from both
identity keys) in person or on a call; once they match, the contact is marked
verified, and only an identity change can undo that.

The code is random instead of a chosen passphrase on purpose: the server
holds the encrypted backup, so a guessable passphrase could be attacked by
anyone who copies the server's disk.

Limits to know about:

- Lose the code and every device, and the history is gone. A new code can be
  made, which is a new identity that friends must verify again.
- Before two people compare safety numbers, the first identity key seen is
  taken on trust.
- Backed-up history is protected by the code alone, not by forward secrecy.
- Which contacts are verified is remembered per device, not shared between
  a person's devices.
- A warning does not stop an unconfirmed device from receiving messages.

## Signing a device out

Each person can see their signed-in devices (with a name they choose and when
each was last active, to the minute) and sign any of them out. A signed-out
device's login session ends, its key packages and pending invitations are
deleted, the server refuses it from then on, and the other members' devices
remove it from every chat's key group, so it cannot read anything sent
afterwards. If it is online it is told, and it erases its own keys and
messages. A device can also sign itself out and erase itself.

What it already decrypted and stored cannot be taken back from a device that
is offline or tampered with: signing out protects the future, not the past.

## Not done yet

These are known gaps, in rough priority order.

1. **Unconfirmed devices are warned about, not blocked.**
2. **Group names are visible to the server.**
3. **Keys at rest are not encrypted** beyond what the browser and operating
   system provide.
4. **The web app is delivered by the server.** Someone in control of the
   server could serve altered code. A strict Content-Security-Policy limits
   injection, and the planned desktop app ships its code separately.
5. **No independent security review** has been done.
