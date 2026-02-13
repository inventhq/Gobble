#!/usr/bin/env node
/**
 * Tracker Platform MCP Server
 *
 * Auto-registers all tools from @tracker/tool-definitions.
 * Add a tool to tool-definitions and it appears here automatically.
 *
 * Transport: stdio (for IDE integrations like Windsurf, Cursor, Claude Desktop)
 *
 * Environment variables:
 *   TRACKER_API_URL   — Platform API base URL (default: http://localhost:8787)
 *   TRACKER_API_KEY   — API key (Bearer token) for authentication (required)
 */

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { ALL_TOOLS, buildRequest, type ParamDef } from "@tracker/tool-definitions";

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

const API_URL = (
  process.env.TRACKER_API_URL || "http://localhost:8787"
).replace(/\/$/, "");
const API_KEY = process.env.TRACKER_API_KEY || "";

if (!API_KEY) {
  console.error(
    "ERROR: TRACKER_API_KEY is required. Set it to a valid Platform API key."
  );
  process.exit(1);
}

// ---------------------------------------------------------------------------
// HTTP helper
// ---------------------------------------------------------------------------

async function apiFetch(
  method: string,
  path: string,
  body?: unknown
): Promise<unknown> {
  const url = `${API_URL}${path}`;
  const headers: Record<string, string> = {
    Authorization: `Bearer ${API_KEY}`,
    "Content-Type": "application/json",
  };

  const resp = await fetch(url, {
    method,
    headers,
    body: body ? JSON.stringify(body) : undefined,
  });

  const text = await resp.text();
  try {
    return JSON.parse(text);
  } catch {
    return { status: resp.status, body: text };
  }
}

// ---------------------------------------------------------------------------
// ParamDef → Zod converter
// ---------------------------------------------------------------------------

function paramToZod(param: ParamDef): z.ZodTypeAny {
  let schema: z.ZodTypeAny;

  switch (param.type) {
    case "number":
      schema = z.number().describe(param.description);
      break;
    case "boolean":
      schema = z.boolean().describe(param.description);
      break;
    case "array":
      schema = z
        .array(z.string())
        .describe(param.description);
      break;
    case "string":
    default:
      schema = z.string().describe(param.description);
      break;
  }

  if (!param.required) {
    schema = schema.optional();
  }

  return schema;
}

// ---------------------------------------------------------------------------
// MCP Server — auto-register all tools from @tracker/tool-definitions
// ---------------------------------------------------------------------------

const server = new McpServer({
  name: "tracker-platform",
  version: "0.1.0",
});

for (const def of ALL_TOOLS) {
  const zodShape: Record<string, z.ZodTypeAny> = {};
  for (const [key, param] of Object.entries(def.parameters)) {
    zodShape[key] = paramToZod(param);
  }

  server.tool(
    def.name,
    def.description,
    zodShape,
    async (args: Record<string, unknown>) => {
      const req = buildRequest(def, args);
      const result = await apiFetch(req.method, req.path, req.body);
      return {
        content: [{ type: "text" as const, text: JSON.stringify(result, null, 2) }],
      };
    }
  );
}

// ---------------------------------------------------------------------------
// Start
// ---------------------------------------------------------------------------

async function main() {
  const transport = new StdioServerTransport();
  await server.connect(transport);
  console.error(
    `Tracker MCP server running on stdio — ${ALL_TOOLS.length} tools registered`
  );
}

main().catch((err) => {
  console.error("Fatal error:", err);
  process.exit(1);
});
