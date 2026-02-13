/**
 * Stytch authentication client.
 *
 * Handles login/logout/session management via Stytch's vanilla JS SDK.
 * Configured via environment variables:
 *   VITE_STYTCH_PUBLIC_TOKEN — Stytch public token (from stytch.com dashboard)
 *
 * When VITE_STYTCH_PUBLIC_TOKEN is not set, auth is bypassed (mock mode)
 * and the app uses the VITE_API_KEY directly.
 */

import { StytchUIClient } from '@stytch/vanilla-js';

let stytchClient: StytchUIClient | null = null;

const STYTCH_PUBLIC_TOKEN = import.meta.env.VITE_STYTCH_PUBLIC_TOKEN || '';

export function isStytchEnabled(): boolean {
  return STYTCH_PUBLIC_TOKEN.length > 0;
}

export function getStytchClient(): StytchUIClient | null {
  if (!isStytchEnabled()) return null;

  if (!stytchClient) {
    stytchClient = new StytchUIClient(STYTCH_PUBLIC_TOKEN);
  }
  return stytchClient;
}

export function getSession(): { authenticated: boolean; userId?: string; email?: string } {
  if (!isStytchEnabled()) {
    // Mock mode — always authenticated
    return { authenticated: true, userId: 'mock-admin', email: 'admin@localhost' };
  }

  const client = getStytchClient();
  if (!client) return { authenticated: false };

  const session = client.session.getSync();
  if (!session) return { authenticated: false };

  const user = client.user.getSync();
  return {
    authenticated: true,
    userId: user?.user_id,
    email: user?.emails?.[0]?.email
  };
}

export async function logout(): Promise<void> {
  const client = getStytchClient();
  if (client) {
    await client.session.revoke();
  }
}
