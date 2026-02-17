/**
 * Cloudflare Worker — serves t.js beacon script from the edge.
 *
 * Serves the minified beacon script with aggressive caching headers.
 * The script is inlined at build time — zero origin fetches.
 *
 * Deploy: cd packages/beacon && npx wrangler deploy
 * Custom domain: js.juicyapi.com
 */

// The beacon script is imported as a string at build time
import BEACON_SCRIPT from "../t.js";

export default {
  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    const path = url.pathname;

    // Health check
    if (path === "/health") {
      return new Response(JSON.stringify({ status: "ok", service: "tracker-beacon" }), {
        headers: { "content-type": "application/json" },
      });
    }

    // Serve beacon script at /t.js or /
    if (path === "/t.js" || path === "/") {
      return new Response(BEACON_SCRIPT, {
        headers: {
          "content-type": "application/javascript; charset=utf-8",
          // Cache at edge for 1 hour, browser for 5 minutes
          // Short browser cache so script updates propagate quickly
          "cache-control": "public, max-age=300, s-maxage=3600",
          // CORS: allow any site to load the script
          "access-control-allow-origin": "*",
          // Security headers
          "x-content-type-options": "nosniff",
        },
      });
    }

    // 404 for anything else
    return new Response("Not found", { status: 404 });
  },
};
