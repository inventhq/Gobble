/**
 * Shared types for tool definitions.
 *
 * These types are consumed by both @tracker/vivgrid-tools (OpenAI format)
 * and @tracker/mcp-server (Zod schemas). Zero runtime — pure type definitions.
 */

export interface ParamDef {
  type: "string" | "number" | "boolean" | "array";
  description: string;
  items?: { type: string };
  required?: boolean;
}

export interface ToolDefinition {
  /** Unique tool name (snake_case) */
  name: string;
  /** Human-readable description for the LLM */
  description: string;
  /** Parameter schemas keyed by param name */
  parameters: Record<string, ParamDef>;
  /** HTTP method for the Platform API call */
  method: "GET" | "POST" | "PATCH" | "DELETE";
  /**
   * API path template. Use :param for path params that come from tool args.
   * e.g. "/api/tenants/:tenant_id"
   */
  path: string;
  /** Which args go into the URL query string (GET params) */
  queryArgs?: string[];
  /** Which args go into the JSON request body (POST/PATCH) */
  bodyArgs?: string[];
  /** Which args are path params (substituted into :param in path) */
  pathArgs?: string[];
}
