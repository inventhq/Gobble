/**
 * Platform API — Multi-tenant management layer for tracker-core.
 *
 * Hono app running on Cloudflare Workers with Turso/libSQL for storage.
 * Provides tenant management, API key auth, webhook CRUD, and an internal
 * secrets endpoint for tracker-core's multi-tenant signature verification.
 */

import { Hono } from "hono";
import { cors } from "hono/cors";
import { createDb } from "./db/client.js";
import { type AppType } from "./types.js";
import { authMiddleware } from "./middleware/auth.js";
import tenants from "./routes/tenants.js";
import keys from "./routes/keys.js";
import webhooks from "./routes/webhooks.js";
import internal from "./routes/internal.js";
import events from "./routes/events.js";
import trackingUrls from "./routes/tracking-urls.js";
import { filterRules } from "./routes/filter-rules.js";
import ingestTokens from "./routes/ingest-tokens.js";
import onboarding from "./routes/onboarding.js";
import onboardingChat, { setOnboardingAppRef } from "./routes/onboarding-chat.js";
import chat, { setAppRef } from "./routes/chat.js";

const app = new Hono<AppType>();

// CORS for frontend dashboard (when built)
app.use("/*", cors());

// Inject DB client into every request context
app.use("/*", async (c, next) => {
  const db = createDb(c.env);
  c.set("db", db);
  return next();
});

// Root landing page (no auth)
app.get("/", (c) => {
  return c.json({
    service: "platform-api",
    description: "Tracker Platform API — tenant management, analytics, webhooks",
    endpoints: {
      "GET /health": "Health check",
      "GET /api/tenants": "List tenants",
      "POST /api/tenants": "Create tenant",
      "GET /api/keys": "List API keys",
      "GET /api/events": "Query events",
      "GET /api/tracking-urls": "List tracking URLs",
      "GET /api/webhooks": "List webhooks",
      "GET /api/filter-rules": "List filter rules",
      "GET /api/ingest-tokens": "List ingest tokens",
      "POST /api/chat": "AI-powered analytics chat",
    },
    auth: "All /api/* routes require Authorization: Bearer <api_key>",
    docs: "https://github.com/inventhq/tracker",
  });
});

// Public health check (no auth)
app.get("/health", (c) => {
  return c.json({ status: "ok", service: "platform-api" });
});

// Public onboarding chat (no auth — new customers have no API key yet)
app.route("/onboard", onboardingChat);

// All /api/* and /internal/* routes require auth
app.use("/api/*", authMiddleware());
app.use("/internal/*", authMiddleware());

// Mount route groups
app.route("/api/tenants", tenants);
app.route("/api/keys", keys);
app.route("/api/webhooks", webhooks);
app.route("/api/events", events);
app.route("/api/tracking-urls", trackingUrls);
app.route("/api/filter-rules", filterRules);
app.route("/api/ingest-tokens", ingestTokens);
app.route("/api", onboarding);
app.route("/api/chat", chat);
app.route("/internal", internal);

// Give chat routes access to app.request() for internal tool execution
setAppRef(app);
setOnboardingAppRef(app);

// 404 fallback
app.notFound((c) => {
  return c.json({ error: "Not found" }, 404);
});

// Global error handler
app.onError((err, c) => {
  console.error("Unhandled error:", err);
  return c.json({ error: "Internal server error" }, 500);
});

export default app;
