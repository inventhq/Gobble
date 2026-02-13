/**
 * Tool definitions for Vivgrid LLM function calling.
 *
 * Auto-derived from @tracker/tool-definitions — the single source of truth.
 * Add a tool to tool-definitions and it appears here automatically.
 */

import { ALL_TOOLS, toOpenAITools, buildRequest } from "@tracker/tool-definitions";

/** OpenAI function calling format — sent to the LLM. */
export const TOOLS = toOpenAITools(ALL_TOOLS);

/** Look up a ToolDefinition by name. */
const TOOL_MAP = new Map(ALL_TOOLS.map((t) => [t.name, t]));

/** Build an HTTP request for a tool call. Returns null if tool not found. */
export function buildToolRequest(
  toolName: string,
  args: Record<string, unknown>
) {
  const def = TOOL_MAP.get(toolName);
  if (!def) return null;
  return buildRequest(def, args);
}
