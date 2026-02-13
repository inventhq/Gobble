/**
 * @tracker/vivgrid-tools — Vivgrid AI chat integration.
 *
 * Usage in the host Hono app:
 *
 *   import { chatRoute, setAppRef } from "@tracker/vivgrid-tools";
 *   app.route("/api/chat", chatRoute);
 *   setAppRef(app);
 */

export { chatRoute } from "./route.js";
export { setAppRef } from "./route.js";
export { TOOLS, buildToolRequest } from "./tools.js";
export { SYSTEM_PROMPT } from "./prompt.js";
