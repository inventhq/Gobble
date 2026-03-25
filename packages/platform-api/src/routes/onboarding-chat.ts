/**
 * Public onboarding chat endpoint.
 *
 * Unauthenticated — new customers have no API key yet.
 * Uses the admin bootstrap key internally to execute tools on behalf
 * of the customer during onboarding (create tenant, issue tokens, etc.).
 *
 * Uses the onboarding-specific system prompt instead of the analytics prompt.
 */

import { Hono } from "hono";
import { TOOLS, buildToolRequest } from "@tracker/vivgrid-tools";
import { ONBOARDING_PROMPT } from "@tracker/vivgrid-tools";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type OnboardingEnv = {
  Bindings: {
    VIVGRID_API_KEY?: string;
    ADMIN_BOOTSTRAP_KEY: string;
    [key: string]: unknown;
  };
  Variables: Record<string, unknown>;
};

// ---------------------------------------------------------------------------
// App reference for internal tool execution
// ---------------------------------------------------------------------------

// eslint-disable-next-line @typescript-eslint/no-explicit-any
let _app: any = null;

/** Called by the host app after mounting routes. */
export function setOnboardingAppRef(app: unknown) {
  _app = app;
}

// ---------------------------------------------------------------------------
// Internal tool executor (uses admin key)
// ---------------------------------------------------------------------------

async function executeTool(
  toolName: string,
  args: Record<string, unknown>,
  adminKey: string,
  env: unknown
): Promise<string> {
  const req = buildToolRequest(toolName, args);
  if (!req) return JSON.stringify({ error: `Unknown tool: ${toolName}` });
  if (!_app) return JSON.stringify({ error: "App reference not set" });

  try {
    const init: Record<string, unknown> = {
      method: req.method,
      headers: {
        Authorization: `Bearer ${adminKey}`,
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
// Onboarding chat route
// ---------------------------------------------------------------------------

const onboardingChat = new Hono<OnboardingEnv>();

onboardingChat.post("/", async (c) => {
  const vivgridKey = c.env.VIVGRID_API_KEY;
  if (!vivgridKey) {
    return c.json({ error: "VIVGRID_API_KEY not configured" }, 500);
  }

  const adminKey = c.env.ADMIN_BOOTSTRAP_KEY;
  if (!adminKey) {
    return c.json({ error: "ADMIN_BOOTSTRAP_KEY not configured" }, 500);
  }

  const body = await c.req.json<{
    message: string;
    history?: Array<{ role: string; content: string }>;
  }>();

  if (!body.message) {
    return c.json({ error: "message is required" }, 400);
  }

  const env = c.env;

  // Build messages array with onboarding prompt
  const messages: Array<Record<string, unknown>> = [
    { role: "system", content: ONBOARDING_PROMPT },
  ];

  if (body.history) {
    for (const msg of body.history) {
      messages.push({ role: msg.role, content: msg.content });
    }
  }

  messages.push({ role: "user", content: body.message });

  // Tool call loop (max 5 rounds — onboarding needs more tool calls than analytics)
  const MAX_ROUNDS = 5;
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

    // Execute each tool call using admin key
    for (const toolCall of assistantMsg.tool_calls) {
      const args = JSON.parse(toolCall.function.arguments);
      const result = await executeTool(
        toolCall.function.name,
        args,
        adminKey,
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

export default onboardingChat;
