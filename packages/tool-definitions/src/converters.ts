/**
 * Converter utilities — transform ToolDefinitions into consumer-specific formats.
 *
 * - toOpenAITools()  → OpenAI function calling format (for Vivgrid)
 * - buildRequest()   → HTTP method, path, query, body (for internal execution)
 */

import type { ToolDefinition } from "./types.js";

// ---------------------------------------------------------------------------
// OpenAI function calling format (used by Vivgrid)
// ---------------------------------------------------------------------------

export interface OpenAITool {
  type: "function";
  function: {
    name: string;
    description: string;
    parameters: {
      type: "object";
      properties: Record<string, unknown>;
      required?: string[];
    };
  };
}

/** Convert a ToolDefinition to OpenAI function calling format. */
export function toOpenAITool(def: ToolDefinition): OpenAITool {
  const properties: Record<string, unknown> = {};
  const required: string[] = [];

  for (const [key, param] of Object.entries(def.parameters)) {
    const prop: Record<string, unknown> = {
      type: param.type,
      description: param.description,
    };
    if (param.type === "array" && param.items) {
      prop.items = param.items;
    }
    properties[key] = prop;
    if (param.required) {
      required.push(key);
    }
  }

  return {
    type: "function",
    function: {
      name: def.name,
      description: def.description,
      parameters: {
        type: "object",
        properties,
        ...(required.length > 0 ? { required } : {}),
      },
    },
  };
}

/** Convert all ToolDefinitions to OpenAI function calling format. */
export function toOpenAITools(defs: ToolDefinition[]): OpenAITool[] {
  return defs.map(toOpenAITool);
}

// ---------------------------------------------------------------------------
// HTTP request builder (used by executor in vivgrid-tools and mcp-server)
// ---------------------------------------------------------------------------

export interface ToolRequest {
  method: "GET" | "POST" | "PATCH" | "DELETE";
  path: string;
  body?: Record<string, unknown>;
}

/** Build an HTTP request from a ToolDefinition and the LLM-provided args. */
export function buildRequest(
  def: ToolDefinition,
  args: Record<string, unknown>
): ToolRequest {
  // 1. Substitute path params (e.g. /api/tenants/:tenant_id)
  let path = def.path;
  if (def.pathArgs) {
    for (const key of def.pathArgs) {
      if (args[key] != null) {
        path = path.replace(`:${key}`, String(args[key]));
      }
    }
  }

  // 2. Special handling for get_breakdown: prepend "param:" to group_by
  const queryParams = new URLSearchParams();
  if (def.name === "get_breakdown" && args.group_by) {
    queryParams.set("group_by", `param:${args.group_by}`);
  }

  // 3. Build query string from queryArgs
  if (def.queryArgs) {
    for (const key of def.queryArgs) {
      if (args[key] != null) {
        queryParams.set(key, String(args[key]));
      }
    }
  }

  const qs = queryParams.toString();
  if (qs) {
    path = `${path}?${qs}`;
  }

  // 4. Build body from bodyArgs
  let body: Record<string, unknown> | undefined;
  if (def.bodyArgs) {
    body = {};
    for (const key of def.bodyArgs) {
      if (args[key] != null) {
        body[key] = args[key];
      }
    }
    if (Object.keys(body).length === 0) body = undefined;
  }

  return { method: def.method, path, body };
}
