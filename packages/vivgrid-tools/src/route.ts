/**
 * Vivgrid AI chat route — Hono sub-app for natural language queries.
 *
 * Handles the full tool-call loop:
 *   1. User sends a natural language message
 *   2. LLM (via Vivgrid, OpenAI-compatible) decides which tools to call
 *   3. Tools execute internally via app.request() — no network round-trip
 *   4. Results sent back to LLM for a natural language response
 */

import { Hono } from "hono";
import { TOOLS, buildToolRequest } from "./tools.js";
import { SYSTEM_PROMPT } from "./prompt.js";

// ---------------------------------------------------------------------------
// App reference — set by host app to enable internal tool execution
// ---------------------------------------------------------------------------

// eslint-disable-next-line @typescript-eslint/no-explicit-any
let _app: any = null;

/** Called by the host app after mounting routes. */
export function setAppRef(app: unknown) {
  _app = app;
}

// ---------------------------------------------------------------------------
// Internal tool executor
// ---------------------------------------------------------------------------

async function executeTool(
  toolName: string,
  args: Record<string, unknown>,
  authHeader: string,
  env: unknown
): Promise<string> {
  const req = buildToolRequest(toolName, args);
  if (!req) return JSON.stringify({ error: `Unknown tool: ${toolName}` });
  if (!_app) return JSON.stringify({ error: "App reference not set" });

  try {
    const init: Record<string, unknown> = {
      method: req.method,
      headers: {
        Authorization: authHeader,
        "Content-Type": "application/json",
      },
    };
    if (req.body) {
      init.body = JSON.stringify(req.body);
    }
    const resp = await _app.request(
      `http://internal${req.path}`,
      init,
      env
    );
    const data = await resp.json();
    return JSON.stringify(data);
  } catch (err) {
    return JSON.stringify({ error: `Tool execution failed: ${err}` });
  }
}

// ---------------------------------------------------------------------------
// Hono sub-app
// ---------------------------------------------------------------------------

type ChatEnv = {
  Bindings: { VIVGRID_API_KEY?: string; [key: string]: unknown };
  Variables: Record<string, unknown>;
};

const chatRoute = new Hono<ChatEnv>();

chatRoute.post("/", async (c) => {
  const vivgridKey = c.env.VIVGRID_API_KEY;
  if (!vivgridKey) {
    return c.json({ error: "VIVGRID_API_KEY not configured" }, 500);
  }

  const body = await c.req.json<{
    message: string;
    history?: Array<{ role: string; content: string }>;
  }>();

  if (!body.message) {
    return c.json({ error: "message is required" }, 400);
  }

  const authHeader = c.req.header("Authorization") || "";
  const env = c.env;

  // Build messages array
  const messages: Array<Record<string, unknown>> = [
    { role: "system", content: SYSTEM_PROMPT },
  ];

  if (body.history) {
    for (const msg of body.history) {
      messages.push({ role: msg.role, content: msg.content });
    }
  }

  messages.push({ role: "user", content: body.message });

  // Tool call loop (max 3 rounds to prevent infinite loops)
  const MAX_ROUNDS = 3;
  for (let round = 0; round < MAX_ROUNDS; round++) {
    const llmResp = await fetch(
      "https://api.vivgrid.com/v1/chat/completions",
      {
        method: "POST",
        headers: {
          Authorization: `Bearer ${vivgridKey}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          messages,
          tools: TOOLS,
          stream: false,
        }),
      }
    );

    if (!llmResp.ok) {
      const errText = await llmResp.text();
      console.error("Vivgrid API error:", errText);
      return c.json({ error: "LLM request failed", details: errText }, 502);
    }

    const llmData = (await llmResp.json()) as {
      choices: Array<{
        message: {
          role: string;
          content: string | null;
          tool_calls?: Array<{
            id: string;
            function: { name: string; arguments: string };
          }>;
        };
        finish_reason: string;
      }>;
      usage?: {
        prompt_tokens: number;
        completion_tokens: number;
        total_tokens: number;
      };
    };

    const choice = llmData.choices[0];
    if (!choice) {
      return c.json({ error: "No response from LLM" }, 502);
    }

    const assistantMsg = choice.message;

    // If no tool calls, return the final text response
    if (
      choice.finish_reason !== "tool_calls" ||
      !assistantMsg.tool_calls?.length
    ) {
      return c.json({
        response: assistantMsg.content || "",
        usage: llmData.usage,
      });
    }

    // Add assistant message with tool calls to history
    messages.push(assistantMsg);

    // Execute each tool call and add results
    for (const toolCall of assistantMsg.tool_calls) {
      const args = JSON.parse(toolCall.function.arguments);
      const result = await executeTool(
        toolCall.function.name,
        args,
        authHeader,
        env
      );

      messages.push({
        role: "tool",
        tool_call_id: toolCall.id,
        content: result,
      });
    }
  }

  return c.json({ error: "Max tool call rounds exceeded" }, 500);
});

export { chatRoute };
