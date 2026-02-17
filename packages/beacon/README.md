# @inventhq/tracker-beacon

Cookieless browser beacon for automatic pageview and outbound click tracking. Drop-in `<script>` tag — zero cookies, zero localStorage, zero fingerprinting, zero dependencies.

Designed for use with [tracker-core](https://github.com/inventhq/tracker). Also available via CDN at `https://js.juicyapi.com/t.js`.

## Install

### CDN (Recommended)

Add a single script tag to your HTML. No build step required:

```html
<script src="https://js.juicyapi.com/t.js" data-key="YOUR_KEY_PREFIX" async defer></script>
```

### npm

```bash
npm install @inventhq/tracker-beacon
```

Then self-host `t.js` from the package:

```javascript
// The package exports the raw script file at:
// node_modules/@inventhq/tracker-beacon/t.js
// Copy it to your static assets directory and serve it yourself.
```

## How It Works

When loaded, `t.js` automatically:

1. **Sends a `pageview` event** with the current page path
2. **Listens for outbound link clicks** and sends `outbound_click` events with the link href and text
3. **Detects SPA navigations** (pushState, replaceState, popstate) and sends additional pageview events
4. **Reads `ad_click_id`** from the URL query string (appended by tracker-core's `/t` redirect) for ad attribution stitching

All events are sent via `navigator.sendBeacon()` to the tracker-core `POST /t/auto` endpoint. The beacon fires asynchronously and survives page unload — no events are lost during navigation.

## Configuration

All configuration is done via `data-*` attributes on the script tag:

| Attribute | Required | Default | Description |
|-----------|----------|---------|-------------|
| `data-key` | **Yes** | — | Your tenant key prefix (e.g. `"6vct"`). Identifies which tenant owns the data. |
| `data-host` | No | `https://track.juicyapi.com` | Tracker-core server URL. Override if self-hosting. |
| `data-track` | No | `"outbound"` | Click tracking mode: `"outbound"` (external links only), `"all"` (all links), or `"none"` (pageviews only). |
| `data-no-spa` | No | — | If present, disables SPA history tracking. Use this on traditional multi-page sites to avoid duplicate pageviews. |

### Examples

**Basic — track pageviews and outbound clicks:**
```html
<script src="https://js.juicyapi.com/t.js" data-key="6vct" async defer></script>
```

**Self-hosted tracker — custom server URL:**
```html
<script src="https://js.juicyapi.com/t.js" data-key="6vct" data-host="https://track.mycompany.com" async defer></script>
```

**Track all link clicks (including internal navigation):**
```html
<script src="https://js.juicyapi.com/t.js" data-key="6vct" data-track="all" async defer></script>
```

**Pageviews only — no click tracking:**
```html
<script src="https://js.juicyapi.com/t.js" data-key="6vct" data-track="none" async defer></script>
```

**Traditional multi-page site — disable SPA detection:**
```html
<script src="https://js.juicyapi.com/t.js" data-key="6vct" data-no-spa async defer></script>
```

## Events Sent

### `pageview`

Sent on page load and on SPA navigations (pushState/replaceState/popstate).

```json
{
  "event_type": "pageview",
  "key_prefix": "6vct",
  "page": "/products/widget?ref=homepage",
  "session_id": "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4",
  "screen_width": 1440,
  "ad_click_id": "019502a1-7b3c-7def-8abc-1234567890ab"
}
```

### `outbound_click`

Sent when a user clicks an external link (or any link, if `data-track="all"`).

```json
{
  "event_type": "outbound_click",
  "key_prefix": "6vct",
  "page": "/products/widget",
  "href": "https://partner.example.com/signup",
  "text": "Sign up for Partner",
  "session_id": "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4",
  "screen_width": 1440,
  "ad_click_id": "019502a1-7b3c-7def-8abc-1234567890ab"
}
```

## Privacy Design

| Concern | How It's Handled |
|---------|-----------------|
| **Cookies** | None. Zero cookies are set or read. |
| **localStorage / sessionStorage** | Not used. |
| **Fingerprinting** | No canvas, WebGL, font, or device fingerprinting. |
| **IP address** | Captured server-side by tracker-core (standard for any HTTP request). Not sent by the script. |
| **Session ID** | Generated per page load via `crypto.getRandomValues()`. Lives in JS memory only — dies on navigation or tab close. Not persisted anywhere. |
| **Ad click ID** | Only present if the user arrived via a tracker-core redirect (`/t` or `/t/:tu_id`). Read from the URL query string, not stored. |
| **GDPR / CCPA** | No personal data is collected or stored client-side. No consent banner required for this script alone (consult your legal team for your specific use case). |

## Ad Click Attribution (Session Stitching)

When a user clicks a tracked ad link (via tracker-core's `/t` or `/t/:tu_id` endpoints), the redirect automatically appends `?ad_click_id=<event_id>` to the destination URL. The beacon script reads this parameter and includes it in every event sent during that page load.

This enables **end-to-end attribution**:

```
Ad Click (Facebook, Google, etc.)
  → tracker-core /t redirect (creates click event, appends ad_click_id)
    → Landing page loads t.js
      → pageview event includes ad_click_id
      → outbound_click events include ad_click_id
        → All events can be joined back to the original ad click
```

**Query example** (downstream analytics):
```sql
-- Find all on-site activity from a specific ad click
SELECT * FROM events
WHERE params->>'ad_click_id' = '019502a1-7b3c-7def-8abc-1234567890ab'
ORDER BY timestamp;
```

## Session ID Behavior

The `session_id` is a random 128-bit hex string generated fresh on every page load. It is **not persisted** — it exists only in JavaScript memory.

- **Multi-page site**: Each page load gets a new session_id. Use `ad_click_id` to stitch across pages.
- **SPA**: The session_id persists across SPA navigations within the same page load.
- **Tab close / refresh**: Session ID is lost. A new one is generated.

This is intentional — it provides per-page-load grouping without any persistent tracking.

## Browser Compatibility

- **Modern browsers**: Chrome 39+, Firefox 31+, Safari 11.1+, Edge 14+ (all support `sendBeacon`)
- **Fallback**: If `sendBeacon` is unavailable, falls back to synchronous `XMLHttpRequest`
- **Script loading**: Use `async defer` attributes for non-blocking load

## Related Packages

- **[@inventhq/tracker-sdk](https://www.npmjs.com/package/@inventhq/tracker-sdk)** — Server-side TypeScript SDK for generating signed/encrypted tracking URLs and batch event delivery.

## License

MIT
