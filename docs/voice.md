# Voice calls

## How a call works

Calls run through [LiveKit](https://livekit.io), an open-source media server
that you host yourself next to Harmony. LiveKit is an SFU: each person sends
their audio once and LiveKit forwards it to everyone else. That is what keeps
a 20 or 40 person raid call workable, where a direct peer-to-peer mesh would
have every person upload their audio to every other person.

1. Someone presses **Call** in a chat. Their device asks Harmony for a
   LiveKit access token. Harmony only gives one to a device that holds the
   chat's encryption keys, and the token lets that device into one room (the
   chat's) for ten minutes.
2. The device derives the **call key** from the chat's MLS group (an MLS
   exporter secret for the current epoch). Every device in the chat derives
   the same key; nobody else can, including Harmony and LiveKit.
3. Every audio frame is encrypted with that key in the browser before it is
   sent (LiveKit's end-to-end encryption, using insertable streams). LiveKit
   forwards frames it cannot read.
4. When the chat's membership changes, its epoch changes, and every device in
   the call switches to the new epoch's key. A removed member's device cannot
   derive it, so it stops hearing the call. The last 16 epochs' keys are kept
   so that someone a second behind on a change is still heard.
5. While in a call, a device checks in with Harmony every 20 seconds, so the
   app can show who is in a call. That list lives in memory only.

## What the servers see

| | Harmony | LiveKit |
| --- | --- | --- |
| What is said | No | No (encrypted frames only) |
| Who is in a call, and when | Yes (in memory, not stored) | Yes |
| IP addresses of people in the call | From normal web traffic | Yes |
| Call key | No | No |

Limits:

- Everyone in a chat shares one call key, so a member could in principle
  pass their audio off as another member's to people in the call. The
  people in a call are named by LiveKit from Harmony's tokens.
- A device that is in the chat but has not yet caught up on a membership
  change may be silent for a moment.
- Calls need a browser with insertable streams: current Chrome, Edge, Firefox
  or Safari.

## Setting up LiveKit

LiveKit is a single Go program. A friend-group call uses little CPU, and a
few hundred MB of RAM is plenty.

1. Install `livekit-server` from its GitHub releases, or with
   `curl -sSL https://get.livekit.io | bash`.
2. Write its config file (for example `/etc/livekit.yaml`), readable only by
   the user LiveKit runs as:

   ```yaml
   port: 7880                 # signalling; only the reverse proxy talks to it
   bind_addresses: [127.0.0.1]
   rtc:
     tcp_port: 7881           # media over TCP, for networks that block UDP
     port_range_start: 50000  # media over UDP
     port_range_end: 50100    # 100 ports covers far more than 40 people
     use_external_ip: true
   keys:
     harmony: <a random secret of at least 32 characters>
   ```

   Generate the secret with `openssl rand -base64 36`.
3. Run it as a service (`livekit-server --config /etc/livekit.yaml`).
4. Have the reverse proxy send LiveKit's signalling path to it. With Caddy,
   inside the site block for Harmony:

   ```
   handle /rtc* {
       reverse_proxy 127.0.0.1:7880
   }
   ```

5. Open the media ports in the firewall: TCP 7881 and UDP 50000 to 50100.
6. Give Harmony the same key, in the server's environment only:

   ```sh
   HARMONY_LIVEKIT_URL=wss://<your Harmony address>
   HARMONY_LIVEKIT_API_KEY=harmony
   HARMONY_LIVEKIT_API_SECRET=<the same secret>
   ```

   Harmony refuses to start if only some of these are set, and logs
   "voice calls are off" if none are.

To check calls work, sign in on two devices, open a shared chat on both and
press **Call**. Each side should see the other in the call panel, and their
voice should come through.
