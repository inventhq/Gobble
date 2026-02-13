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

// Public health check (no auth)
app.get("/health", (c) => {
  return c.json({ status: "ok", service: "platform-api" });
});

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
app.route("/api/chat", chat);
app.route("/internal", internal);

// Give the chat route access to app.request() for internal tool execution
setAppRef(app);

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
