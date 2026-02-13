# @tracker/sdk

TypeScript SDK for [tracker-core](../../README.md) — generate signed/encrypted tracking URLs and batch-send events.

**Zero external dependencies.** Uses Node.js built-in `crypto` module for all cryptographic operations.

## Install

```bash
npm install @tracker/sdk
```

## Quick Start

### Link Generation (Pure Functions, No Network)

```typescript
import { buildSignedClickUrl, buildPostbackUrl, buildImpressionUrl } from "@tracker/sdk";

// Signed click URL — destination visible, HMAC-protected
const clickUrl = buildSignedClickUrl(
  "https://track.example.com",
  "your-hmac-secret",
  "https://offer.example.com/landing",
  { offer_id: "123", aff_id: "456", sub1: "google" },
);
// => "https://track.example.com/t?url=https%3A%2F%2Foffer...&sig=abc123...&offer_id=123&aff_id=456&sub1=google"

// Postback URL for server-to-server conversions
const postbackUrl = buildPostbackUrl("https://track.example.com", {
  click_id: "abc123",
  payout: "2.50",
  status: "approved",
});

// Impression pixel for HTML/email embedding
const pixelUrl = buildImpressionUrl("https://track.example.com", {
  campaign_id: "789",
  placement: "header_banner",
});
const pixel = `<img src="${pixelUrl}" width="1" height="1" alt="" />`;
```

### Encrypted Click URLs

```typescript
import { buildEncryptedClickUrl } from "@tracker/sdk";

// Encrypted click URL — destination hidden inside AES-256-GCM blob
const clickUrl = buildEncryptedClickUrl(
  "https://track.example.com",
  "your-64-char-hex-key-here...", // 32 bytes as hex
  "https://offer.example.com/landing",
  { offer_id: "123" },
);
// => "https://track.example.com/t?d=base64urlblob&offer_id=123"
```

### Batch Client (Server-Side Event Ingestion)

```typescript
import { TrackerClient } from "@tracker/sdk";

const client = new TrackerClient({
  apiUrl: "https://track.example.com",
  mode: "signed",
  hmacSecret: "your-hmac-secret",
  batchSize: 100,       // flush every 100 events
  flushInterval: 1000,  // or every 1 second
  onError: (err, events) => {
    console.error(`Failed to send ${events.length} events:`, err.message);
  },
});

// Queue events — they flush automatically
client.track({
  event_type: "postback",
  ip: "203.0.113.1",
  user_agent: "Mozilla/5.0",
  referer: null,
  accept_language: "en-US",
  request_path: "/p",
  request_host: "track.example.com",
  params: { click_id: "abc123", payout: "2.50" },
});

// On shutdown — flush remaining events
await client.destroy();
```

## API Reference

### Link Builders

| Function | Description |
|----------|-------------|
| `buildSignedClickUrl(baseUrl, secret, destinationUrl, params?)` | HMAC-signed click redirect URL |
| `buildEncryptedClickUrl(baseUrl, keyHex, destinationUrl, params?)` | AES-GCM encrypted click URL |
| `buildPostbackUrl(baseUrl, params)` | Postback/conversion URL |
| `buildImpressionUrl(baseUrl, params)` | Impression pixel URL |

### Crypto Utilities

| Function | Description |
|----------|-------------|
| `signHmac(secret, url)` | Generate HMAC-SHA256 hex signature |
| `verifyHmac(secret, url, signature)` | Verify HMAC signature (constant-time) |
| `encryptUrl(keyHex, url)` | AES-256-GCM encrypt → base64url |
| `decryptUrl(keyHex, encoded)` | AES-256-GCM decrypt ← base64url |

### Batch Client

| Method | Description |
|--------|-------------|
| `new TrackerClient(config)` | Create a buffered client |
| `client.track(event)` | Queue an event (auto-flushes at batch size) |
| `client.flush()` | Immediately flush all buffered events |
| `client.pending` | Number of events currently buffered |
| `client.destroy()` | Stop timer + flush remaining events |

### TrackerConfig

| Option | Default | Description |
|--------|---------|-------------|
| `apiUrl` | — | Required. Tracker-core server URL |
| `mode` | — | Required. `"signed"` or `"encrypted"` |
| `hmacSecret` | — | Required for signed mode |
| `encryptionKey` | — | Required for encrypted mode |
| `batchSize` | `100` | Events per batch before auto-flush |
| `flushInterval` | `1000` | Ms between time-based flushes |
| `onError` | — | Callback for failed batch sends |

## Compatibility

- **Node.js** ≥ 18 (uses built-in `crypto` and `fetch`)
- **Zero dependencies** — only `@types/node` and `typescript` as dev deps
