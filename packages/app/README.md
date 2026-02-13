# Tracker Dashboard

Unified admin + tenant dashboard for the Tracker event tracking platform. Built with SvelteKit (Svelte 5), TailwindCSS v4, and Lucide icons.

## Quick Start

```sh
npm install
cp .env.example .env   # then fill in values
npm run dev             # http://localhost:5173
```

## Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `VITE_API_URL` | Yes | Platform API URL (default: `http://localhost:8787`) |
| `VITE_API_KEY` | Yes | Platform API bearer token |
| `VITE_STYTCH_PUBLIC_TOKEN` | No | Stytch public token for client-side login UI |
| `STYTCH_PROJECT_ID` | No | Stytch project ID for server-side session validation |
| `STYTCH_SECRET_KEY` | No | Stytch secret key for server-side session validation |
| `VITE_PERMIT_API_KEY` | No | Permit.io API key for RBAC |

When Stytch and Permit.io variables are **not set**, the app runs in **mock mode** — auto-authenticates as admin with full access. This is useful for local development.

## Authentication (Stytch)

Authentication is handled by [Stytch](https://stytch.com) with two layers:

- **Client-side:** `@stytch/vanilla-js` renders the login UI (magic link + Google OAuth). Configured via `VITE_STYTCH_PUBLIC_TOKEN`.
- **Server-side:** The `stytch` Node SDK validates session tokens in `hooks.server.ts` on every `/dashboard` request. Configured via `STYTCH_PROJECT_ID` + `STYTCH_SECRET_KEY`. Invalid or missing sessions are redirected to `/login`.

### Stytch Setup

1. Create a project at [stytch.com](https://stytch.com)
2. Copy your **Public token**, **Project ID**, and **Secret key** from the API Keys page
3. Add your app's URL to the redirect allowlist (e.g. `http://localhost:5173/dashboard`)
4. Paste the values into `.env`

## Authorization (Permit.io)

Role-based access control is handled by [Permit.io](https://permit.io). The user's **email address** (from Stytch) is used as the Permit.io user key.

### Permit.io Setup

#### 1. Create Resources

In the Permit.io dashboard, go to **Policy → Resources** and create:

| Resource | Actions |
|----------|---------|
| `tenants` | `manage`, `list`, `create`, `update`, `rotate_secrets` |
| `keys` | `list`, `create`, `revoke` |
| `webhooks` | `list`, `create`, `update`, `delete`, `test` |
| `events` | `list` |
| `stats` | `read` |

#### 2. Create Roles

Go to **Policy → Roles** and create:

- **`admin`** — grant all actions on all resources
- **`tenant`** — grant all actions on `keys`, `webhooks`, `events`, `stats` only. **No access to `tenants` resource.**

#### 3. Add Users

Go to **Directory → Users** and add users:

- **Key:** The user's email address (must match their Stytch login email)
- **Email:** Same email address
- **Role:** Assign `admin` or `tenant` under the Default Tenant

> **Important:** The "Key" field in Permit.io **must be the user's email address**. This is how the dashboard maps Stytch sessions to Permit.io permissions.

### How Permissions Work

| Feature | `admin` | `tenant` |
|---------|---------|----------|
| Dashboard overview | ✅ | ✅ |
| Events table | ✅ | ✅ |
| Webhooks CRUD | ✅ | ✅ |
| API Keys CRUD | ✅ | ✅ |
| Tenants management | ✅ | ❌ (hidden) |
| Settings | ✅ | ✅ |

The "Tenants" nav item is automatically hidden for non-admin users.

## Pages

| Route | Description |
|-------|-------------|
| `/login` | Stytch login (bypassed in mock mode) |
| `/dashboard` | Stats overview with summary cards + hourly breakdown |
| `/dashboard/events` | Filterable, paginated event table |
| `/dashboard/webhooks` | Webhook CRUD — register, toggle, test, delete |
| `/dashboard/keys` | API key CRUD — create (copy-once), revoke |
| `/dashboard/tenants` | Tenant management — create, edit plan, rotate secrets (admin only) |
| `/dashboard/settings` | Health check, auth info, environment details |

## Project Structure

```
src/
├── hooks.server.ts              # Server-side session validation
├── lib/
│   ├── api/                     # Modular API client (one file per resource)
│   │   ├── client.ts            # Base fetch wrapper with bearer token
│   │   ├── tenants.ts           # Tenant API calls
│   │   ├── keys.ts              # API key API calls
│   │   ├── webhooks.ts          # Webhook API calls
│   │   ├── events.ts            # Events + stats API calls
│   │   └── health.ts            # Health check
│   ├── auth/
│   │   ├── stytch.ts            # Client-side Stytch (login/session)
│   │   ├── stytch-server.ts     # Server-side Stytch (session validation)
│   │   └── permit.ts            # Permit.io RBAC checks
│   ├── stores/
│   │   └── auth.svelte.ts       # Reactive auth state (Svelte 5 runes)
│   ├── components/layout/
│   │   ├── Sidebar.svelte       # Navigation sidebar
│   │   ├── TopBar.svelte        # Top bar with breadcrumbs
│   │   └── StatCard.svelte      # Metric card component
│   └── utils/
│       ├── constants.ts         # API URL, event types, plans, roles
│       ├── format.ts            # Date/number formatting
│       └── cn.ts                # Tailwind class merge
└── routes/
    ├── +layout.svelte           # Root layout (API key init, fonts)
    ├── +page.svelte             # Redirect to /dashboard
    ├── login/+page.svelte       # Login page
    └── dashboard/
        ├── +layout.svelte       # Dashboard layout (auth guard)
        ├── +page.svelte         # Overview
        ├── events/+page.svelte  # Events table
        ├── webhooks/+page.svelte# Webhooks CRUD
        ├── keys/+page.svelte    # API keys CRUD
        ├── tenants/+page.svelte # Tenants admin
        └── settings/+page.svelte# Settings
```

## Building

```sh
npm run build
npm run preview   # preview production build locally
```
