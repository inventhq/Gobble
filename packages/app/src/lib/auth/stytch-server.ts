/**
 * Server-side Stytch session validation.
 *
 * Uses the Stytch Node SDK to verify session tokens on the server.
 * This ensures that session tokens cannot be forged client-side.
 *
 * Environment variables (server-side only, not VITE_ prefixed):
 *   STYTCH_PROJECT_ID  — Stytch project ID
 *   STYTCH_SECRET_KEY  — Stytch secret key
 */

import * as stytch from 'stytch';

let client: stytch.Client | null = null;

function getClient(): stytch.Client | null {
  const projectId = process.env.STYTCH_PROJECT_ID || '';
  const secret = process.env.STYTCH_SECRET_KEY || '';

  if (!projectId || !secret) return null;

  if (!client) {
    client = new stytch.Client({
      project_id: projectId,
      secret: secret,
      env: projectId.startsWith('project-test-')
        ? stytch.envs.test
        : stytch.envs.live
    });
  }

  return client;
}

export function isServerAuthEnabled(): boolean {
  return !!(process.env.STYTCH_PROJECT_ID && process.env.STYTCH_SECRET_KEY);
}

export interface SessionUser {
  userId: string;
  email: string;
}

/**
 * Validate a Stytch session token server-side.
 * Returns the authenticated user or null if invalid.
 */
export async function validateSession(sessionToken: string): Promise<SessionUser | null> {
  const stytchClient = getClient();
  if (!stytchClient) return null;

  try {
    const resp = await stytchClient.sessions.authenticate({
      session_token: sessionToken
    });

    const email = resp.user.emails?.[0]?.email || '';

    return {
      userId: resp.user.user_id,
      email
    };
  } catch {
    return null;
  }
}
