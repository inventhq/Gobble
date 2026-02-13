/**
 * Auth store — reactive state for current user session and permissions.
 *
 * Uses Svelte 5 runes ($state) for reactivity.
 * In mock mode (no Stytch token), auto-authenticates as admin.
 */

import { getSession, isStytchEnabled } from '$lib/auth/stytch';
import { getUserPermissions, type UserPermissions } from '$lib/auth/permit';

interface AuthState {
  initialized: boolean;
  authenticated: boolean;
  userId: string;
  email: string;
  permissions: UserPermissions;
}

const defaultPermissions: UserPermissions = {
  role: 'admin',
  canManageTenants: true,
  canRotateSecrets: true
};

export const auth: AuthState = $state({
  initialized: false,
  authenticated: false,
  userId: '',
  email: '',
  permissions: defaultPermissions
});

export async function initAuth(): Promise<void> {
  const session = getSession();

  if (session.authenticated) {
    auth.authenticated = true;
    auth.userId = session.userId || '';
    auth.email = session.email || '';

    // Fetch permissions (use email as Permit.io user key for stable identity)
    auth.permissions = await getUserPermissions(auth.email);
  } else {
    auth.authenticated = false;
  }

  auth.initialized = true;
}

export function isMockAuth(): boolean {
  return !isStytchEnabled();
}
