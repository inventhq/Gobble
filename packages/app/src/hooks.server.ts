/**
 * SvelteKit server hooks — runs on every request.
 *
 * Validates Stytch session tokens server-side before allowing access
 * to protected /dashboard routes. Unprotected routes (/login, /health)
 * pass through without auth.
 *
 * When STYTCH_PROJECT_ID and STYTCH_SECRET_KEY are not set, auth is
 * bypassed (mock mode) for local development.
 */

import type { Handle } from '@sveltejs/kit';
import { redirect } from '@sveltejs/kit';
import { validateSession, isServerAuthEnabled } from '$lib/auth/stytch-server';

const PUBLIC_PATHS = ['/', '/login', '/authenticate', '/health', '/api/permissions'];

export const handle: Handle = async ({ event, resolve }) => {
  const path = event.url.pathname;

  // Skip auth for public paths and static assets
  if (PUBLIC_PATHS.some((p) => path === p) || path.startsWith('/_app')) {
    return resolve(event);
  }

  // If server-side auth is not configured, allow all (mock mode)
  if (!isServerAuthEnabled()) {
    event.locals.user = { userId: 'mock-admin', email: 'admin@localhost' };
    return resolve(event);
  }

  // Protected route — validate session
  if (path.startsWith('/dashboard')) {
    const sessionToken =
      event.cookies.get('stytch_session') ||
      event.url.searchParams.get('stytch_token_type') === 'multi_tenant_magic_links'
        ? event.url.searchParams.get('token') || ''
        : '';

    if (!sessionToken) {
      throw redirect(303, '/login');
    }

    const user = await validateSession(sessionToken);
    if (!user) {
      // Invalid session — clear cookie and redirect to login
      event.cookies.delete('stytch_session', { path: '/' });
      throw redirect(303, '/login');
    }

    // Attach user to locals for downstream use
    event.locals.user = user;
  }

  return resolve(event);
};
