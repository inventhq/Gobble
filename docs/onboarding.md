# Tracker Onboarding Guide

Complete integration guide for adding event tracking to your website or platform using the **inventhq/tracker** system. This guide covers both client-side automatic tracking (pageviews, clicks) and server-side ad click tracking with full attribution.

---

## Table of Contents

1. [Overview](#overview)
2. [Prerequisites](#prerequisites)
3. [Step 1: Get Your Tenant Credentials](#step-1-get-your-tenant-credentials)
4. [Step 2: Add Client-Side Tracking (t.js Beacon)](#step-2-add-client-side-tracking-tjs-beacon)
5. [Step 3: Add Server-Side Click Tracking (SDK)](#step-3-add-server-side-click-tracking-sdk)
6. [Step 4: End-to-End Attribution (Session Stitching)](#step-4-end-to-end-attribution-session-stitching)
7. [Step 5: Verify Your Integration](#step-5-verify-your-integration)
8. [Configuration Reference](#configuration-reference)
9. [Event Schema Reference](#event-schema-reference)
10. [Troubleshooting](#troubleshooting)

---

## Overview

The tracker system has two complementary packages:

| Package | Purpose | Runtime | Install |
|---------|---------|---------|---------|
| **[@inventhq/tracker-beacon](https://www.npmjs.com/package/@inventhq/tracker-beacon)** | Automatic pageview & outbound click tracking on your website | Browser (client-side) | `<script>` tag or npm |
| **[@inventhq/tracker-sdk](https://www.npmjs.com/package/@inventhq/tracker-sdk)** | Generate signed/encrypted click URLs, postback URLs, impression pixels, and batch event delivery | Node.js (server-side) | npm |

**How they work together:**

```
┌─────────────────────────────────────────────────────────────────────┐
│                        YOUR AD / EMAIL / PAGE                       │
│                                                                     │
│  Server-side (SDK):                                                 │
│    buildSignedClickUrl() → https://track.juicyapi.com/t?url=...    │
│                                    │                                │
│                              User clicks                            │
│                                    ▼                                │
│                          tracker-core /t redirect                   │
│                          (logs click event)                         │
│                          appends ?ad_click_id=...                   │
│                                    │                                │
│                                    ▼                                │
│  Client-side (Beacon):                                              │
│    t.js loads on landing page                                       │
│    reads ad_click_id from URL                                       │
│    sends pageview + click events with ad_click_id                   │
│    → full attribution chain from ad → site → conversion             │
└─────────────────────────────────────────────────────────────────────┘
```

---

## Prerequisites

- **Tenant account** on the tracker platform (provides your `key_prefix` and HMAC secret)
- **Node.js ≥ 18** (for server-side SDK only)
- **tracker-core** running at a known URL (default: `https://track.juicyapi.com`)

---

## Step 1: Get Your Tenant Credentials

Contact your platform administrator to obtain:

| Credential | Example | Used By |
|------------|---------|---------|
| `key_prefix` | `6vct` | Both beacon and SDK — identifies your tenant |
| `hmac_secret` | `85eafcd6ac40...` | SDK only — signs click URLs |
| `encryption_key` | `aabbccdd...` (64 hex chars) | SDK only — encrypts click URLs (if using encrypted mode) |
| `tracker_url` | `https://track.juicyapi.com` | Both — the tracker-core server |

Your `key_prefix` is a short string (typically 4 characters) that uniquely identifies your tenant in the multi-tenant tracker system.

---

## Step 2: Add Client-Side Tracking (t.js Beacon)

### Option A: CDN Script Tag (Recommended)

Add this single line to your HTML, just before `</body>` or in `<head>`:

```html
<script src="https://js.juicyapi.com/t.js" data-key="YOUR_KEY_PREFIX" async defer></script>
```

**Replace `YOUR_KEY_PREFIX`** with your tenant key prefix (e.g. `6vct`).

That's it. The script will automatically:
- Send a `pageview` event on page load
- Track outbound link clicks
- Detect SPA navigations (React, Vue, Next.js, etc.)
- Read `ad_click_id` from the URL for attribution

### Option B: Self-Hosted via npm

```bash
npm install @inventhq/tracker-beacon
```

Copy `node_modules/@inventhq/tracker-beacon/t.js` to your static assets directory, then reference it:

```html
<script src="/assets/t.js" data-key="YOUR_KEY_PREFIX" data-host="https://track.juicyapi.com" async defer></script>
```

> **Note:** When self-hosting, you must set `data-host` to your tracker-core URL since the script defaults to `https://track.juicyapi.com`.

### Script Tag Attributes

| Attribute | Required | Default | Description |
|-----------|----------|---------|-------------|
| `data-key` | **Yes** | — | Your tenant `key_prefix` |
| `data-host` | No | `https://track.juicyapi.com` | Tracker-core server URL |
| `data-track` | No | `"outbound"` | `"outbound"` = external links only, `"all"` = all links, `"none"` = pageviews only |
| `data-no-spa` | No | — | If present, disables SPA navigation tracking |

### Framework-Specific Examples

**Next.js (App Router):**
```tsx
// app/layout.tsx
import Script from "next/script";

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html>
      <body>
        {children}
        <Script
          src="https://js.juicyapi.com/t.js"
          data-key="YOUR_KEY_PREFIX"
          strategy="afterInteractive"
        />
      </body>
    </html>
  );
}
```

**React (Vite / CRA):**
```html
<!-- public/index.html -->
<script src="https://js.juicyapi.com/t.js" data-key="YOUR_KEY_PREFIX" async defer></script>
```

**Vue / Nuxt:**
```html
<!-- nuxt.config.ts -->
export default defineNuxtConfig({
  app: {
    head: {
      script: [
        { src: 'https://js.juicyapi.com/t.js', 'data-key': 'YOUR_KEY_PREFIX', async: true, defer: true }
      ]
    }
  }
})
```

**WordPress:**
```php
// functions.php
function add_tracker_beacon() {
    echo '<script src="https://js.juicyapi.com/t.js" data-key="YOUR_KEY_PREFIX" async defer></script>';
}
add_action('wp_footer', 'add_tracker_beacon');
```

**Plain HTML:**
```html
<!DOCTYPE html>
<html>
<head>
  <title>My Site</title>
</head>
<body>
  <h1>Welcome</h1>
  <a href="https://partner.example.com">Visit Partner</a>

  <script src="https://js.juicyapi.com/t.js" data-key="YOUR_KEY_PREFIX" async defer></script>
</body>
</html>
```

---

## Step 3: Add Server-Side Click Tracking (SDK)

The SDK generates tracking URLs that redirect through tracker-core, logging a click event before sending the user to the destination.

### Install

```bash
npm install @inventhq/tracker-sdk
```

### Generate Signed Click URLs

```typescript
import { buildSignedClickUrl } from "@inventhq/tracker-sdk";

const trackingUrl = buildSignedClickUrl(
  "https://track.juicyapi.com",   // tracker-core URL
  "YOUR_HMAC_SECRET",              // your tenant HMAC secret
  "https://offer.example.com/landing", // destination URL
  {
    key_prefix: "YOUR_KEY_PREFIX", // your tenant key prefix
    offer_id: "123",               // custom params (passed through)
    aff_id: "456",
    campaign: "summer_sale",
  },
);

// Use this URL in your ads, emails, or pages:
// https://track.juicyapi.com/t?url=https%3A%2F%2Foffer...&sig=6vct_abc123...&offer_id=123&...
```

### Generate Encrypted Click URLs

If you don't want the destination URL visible in the tracking link:

```typescript
import { buildEncryptedClickUrl } from "@inventhq/tracker-sdk";

const trackingUrl = buildEncryptedClickUrl(
  "https://track.juicyapi.com",
  "YOUR_64_CHAR_HEX_ENCRYPTION_KEY",
  "https://offer.example.com/landing",
  { offer_id: "123" },
);
// => https://track.juicyapi.com/t?d=base64urlblob&offer_id=123
```

### Generate Short Tracked URLs

For pre-registered tracking URLs with server-side destination resolution:

```typescript
import { buildTrackedClickUrl } from "@inventhq/tracker-sdk";

const trackingUrl = buildTrackedClickUrl(
  "https://track.juicyapi.com",
  "YOUR_HMAC_SECRET",
  "tu_019502a1-7b3c-7def-8abc-1234567890ab", // tracking URL ID from Platform API
  { key_prefix: "YOUR_KEY_PREFIX", aff_id: "456" },
);
// => https://track.juicyapi.com/t/tu_019502a1-...?sig=6vct_abc123...&key_prefix=6vct&aff_id=456
```

### Postback URLs (Server-to-Server Conversions)

```typescript
import { buildPostbackUrl } from "@inventhq/tracker-sdk";

const postbackUrl = buildPostbackUrl("https://track.juicyapi.com", {
  click_id: "abc123",
  payout: "2.50",
  status: "approved",
  currency: "USD",
});
// Fire this URL from your server when a conversion happens
```

### Impression Pixels

```typescript
import { buildImpressionUrl } from "@inventhq/tracker-sdk";

const pixelUrl = buildImpressionUrl("https://track.juicyapi.com", {
  campaign_id: "789",
  placement: "header_banner",
  key_prefix: "YOUR_KEY_PREFIX",
});

// Embed in HTML or email:
const pixel = `<img src="${pixelUrl}" width="1" height="1" alt="" style="display:none" />`;
```

### Batch Event Delivery

For high-volume server-side event ingestion:

```typescript
import { TrackerClient } from "@inventhq/tracker-sdk";

const client = new TrackerClient({
  apiUrl: "https://track.juicyapi.com",
  mode: "signed",
  hmacSecret: "YOUR_HMAC_SECRET",
  batchSize: 100,       // flush every 100 events
  flushInterval: 1000,  // or every 1 second
  onError: (err, events) => {
    console.error(`Failed to send ${events.length} events:`, err.message);
  },
});

// Queue events — they auto-flush
client.track({
  event_type: "purchase",
  ip: "203.0.113.1",
  user_agent: "Mozilla/5.0",
  referer: null,
  accept_language: "en-US",
  request_path: "/checkout",
  request_host: "mystore.com",
  params: { order_id: "ORD-789", amount: "49.99", currency: "USD" },
});

// On shutdown — flush remaining events
await client.destroy();
```

---

## Step 4: End-to-End Attribution (Session Stitching)

This is the key feature that connects ad clicks to on-site behavior.

### How It Works

1. **You generate a tracking URL** (server-side SDK) and place it in your ad, email, or page
2. **User clicks the tracking URL** → tracker-core logs a `click` event and redirects to the destination
3. **tracker-core appends `?ad_click_id=<event_id>`** to the destination URL during redirect
4. **t.js beacon loads on the landing page**, reads `ad_click_id` from the URL
5. **Every beacon event includes `ad_click_id`** — pageviews, outbound clicks, everything
6. **Downstream analytics** can join all events by `ad_click_id` to see the full user journey

### Example Flow

**Server-side: Generate the ad link**
```typescript
import { buildSignedClickUrl } from "@inventhq/tracker-sdk";

const adLink = buildSignedClickUrl(
  "https://track.juicyapi.com",
  "YOUR_HMAC_SECRET",
  "https://mystore.com/products/widget",
  { key_prefix: "6vct", campaign: "facebook_q1", ad_set: "lookalike" },
);
// Place this link in your Facebook ad
```

**User clicks → tracker-core redirects to:**
```
https://mystore.com/products/widget?ad_click_id=019502a1-7b3c-7def-8abc-1234567890ab
```

**Client-side: t.js is already on mystore.com**
```html
<!-- Already in your site's HTML -->
<script src="https://js.juicyapi.com/t.js" data-key="6vct" async defer></script>
```

**Events sent by t.js automatically include ad_click_id:**
```json
{
  "event_type": "pageview",
  "key_prefix": "6vct",
  "page": "/products/widget?ad_click_id=019502a1-...",
  "ad_click_id": "019502a1-7b3c-7def-8abc-1234567890ab",
  "session_id": "f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2"
}
```

**Query the full journey:**
```sql
SELECT event_type, page, href, timestamp
FROM events
WHERE params->>'ad_click_id' = '019502a1-7b3c-7def-8abc-1234567890ab'
ORDER BY timestamp;
```

| event_type | page | href | timestamp |
|------------|------|------|-----------|
| click | /t | — | 2026-02-17 10:00:00 |
| pageview | /products/widget | — | 2026-02-17 10:00:01 |
| outbound_click | /products/widget | https://checkout.mystore.com/buy | 2026-02-17 10:00:15 |

---

## Step 5: Verify Your Integration

### Check the Beacon

1. Open your site in a browser
2. Open DevTools → Network tab
3. Filter by `t/auto`
4. You should see a POST request to `https://track.juicyapi.com/t/auto` with a `204 No Content` response
5. Click an outbound link — you should see a second POST with `event_type: "outbound_click"`

### Check Click Tracking

1. Open a tracking URL you generated with the SDK
2. Verify you're redirected to the destination
3. Check the destination URL has `?ad_click_id=...` appended
4. Check DevTools Network tab — the beacon should include `ad_click_id` in its payload

### Health Check

```bash
curl https://track.juicyapi.com/health
# => {"status":"ok","events_processed":12345,"iggy":"connected"}
```

---

## Configuration Reference

### Environment Variables (for self-hosted tracker-core)

| Variable | Description |
|----------|-------------|
| `HMAC_SECRET` | Shared secret for HMAC-signed URLs |
| `ENCRYPTION_KEY` | 64-char hex key for AES-256-GCM encrypted URLs |
| `URL_MODE` | `"signed"` or `"encrypted"` — must match SDK mode |
| `PLATFORM_API_URL` | URL of the Platform API (tenant management) |
| `PLATFORM_API_KEY` | API key for Platform API access |

### Tracker-Core Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/t` | GET | Click redirect (signed or encrypted URL) |
| `/t/:tu_id` | GET | Short URL click redirect |
| `/p` | GET | Postback / conversion tracking |
| `/i` | GET | Impression pixel (1x1 GIF) |
| `/t/auto` | POST | Browser beacon endpoint (used by t.js) |
| `/batch` | POST | Bulk event ingestion (JSON array) |
| `/ingest` | POST | Authenticated external event ingestion |
| `/health` | GET | Health check |

---

## Event Schema Reference

Every event captured by tracker-core follows this schema:

| Field | Type | Description |
|-------|------|-------------|
| `event_id` | string | Unique event ID (auto-generated UUID) |
| `event_type` | string | `"click"`, `"pageview"`, `"outbound_click"`, `"postback"`, `"impression"`, or custom |
| `timestamp` | number | Unix timestamp in milliseconds |
| `ip` | string | Client IP address (captured server-side) |
| `user_agent` | string | Client User-Agent header |
| `referer` | string? | HTTP Referer header |
| `accept_language` | string? | Accept-Language header |
| `request_path` | string | Endpoint path (`/t`, `/t/auto`, `/p`, `/i`, `/batch`) |
| `request_host` | string | Host header value |
| `params` | object | Arbitrary key-value parameters (all your custom data) |

### Common `params` Fields

| Field | Present In | Description |
|-------|-----------|-------------|
| `key_prefix` | All events | Tenant identifier |
| `page` | Beacon events | Page path where the event occurred |
| `href` | `outbound_click` | Clicked link URL |
| `text` | `outbound_click` | Clicked link text (truncated to 150 chars) |
| `session_id` | Beacon events | Per-page-load random ID |
| `ad_click_id` | Beacon events (when present) | Links on-site events to the original ad click |
| `screen_width` | Beacon events | Viewport width in pixels |
| `tu_id` | Tracked URL clicks | Tracking URL ID |
| `offer_id`, `aff_id`, etc. | Click/postback events | Custom tracking parameters you set |

---

## Troubleshooting

### Beacon events not appearing in Network tab
- Verify `data-key` is set on the script tag
- Check browser console for errors
- Ensure tracker-core is reachable (test with `curl https://track.juicyapi.com/health`)

### Click URL returns 403 Forbidden
- Verify your HMAC secret matches the server's `HMAC_SECRET`
- Ensure `key_prefix` is included in params when building signed URLs
- Check that the tenant exists in the Platform API

### `ad_click_id` not appearing in beacon events
- Verify the tracking URL redirects correctly (check with `curl -I <tracking_url>`)
- Check the destination URL has `?ad_click_id=...` appended after redirect
- Ensure t.js is loaded on the landing page

### Batch events rejected (400/500)
- Verify the event schema matches the `TrackingEvent` interface
- Check that `event_type`, `ip`, `user_agent`, `request_path`, `request_host` are all present
- Ensure the JSON payload is a valid array

### Rate limited (429)
- Your tenant has exceeded its rate limit
- Contact your platform administrator to increase limits
- Implement backoff/retry in your `onError` callback

---

## Support

- **GitHub**: [github.com/inventhq/tracker](https://github.com/inventhq/tracker)
- **npm (SDK)**: [@inventhq/tracker-sdk](https://www.npmjs.com/package/@inventhq/tracker-sdk)
- **npm (Beacon)**: [@inventhq/tracker-beacon](https://www.npmjs.com/package/@inventhq/tracker-beacon)
- **CDN**: `https://js.juicyapi.com/t.js`
