/**
 * Permit.io RBAC integration.
 *
 * Handles role-based access control via Permit.io's Cloud PDP.
 * Configured via environment variables:
 *   VITE_PERMIT_API_KEY — Permit.io API key (from permit.io dashboard)
 *
 * When VITE_PERMIT_API_KEY is not set, RBAC is bypassed (mock mode)
 * and the current user is treated as admin.
 *
 * User identity: The user's **email** (from Stytch) is used as the
 * Permit.io user key. When adding users in the Permit.io dashboard,
 * set the "Key" field to the user's email address.
 *
 * Roles:
 *   - admin: Full access to all resources (tenants, keys, webhooks, events, stats)
 *   - tenant: Scoped access to own resources only (keys, webhooks, events, stats)
 *
 * Resources:
 *   - tenants: list, create, update, rotate_secrets (admin only)
 *   - keys: list, create, revoke
 *   - webhooks: list, create, update, delete, test
 *   - events: list
 *   - stats: read
 */

const PERMIT_API_KEY = import.meta.env.VITE_PERMIT_API_KEY || '';

export type Role = 'admin' | 'tenant';

export interface UserPermissions {
  role: Role;
  canManageTenants: boolean;
  canRotateSecrets: boolean;
}

export function isPermitEnabled(): boolean {
  return PERMIT_API_KEY.length > 0;
}

/**
 * Get permissions for the current user.
 * In mock mode, returns admin permissions.
 * With Permit.io enabled, queries the PDP for the user's role.
 */
export async function getUserPermissions(userEmail: string): Promise<UserPermissions> {
  if (!isPermitEnabled()) {
    // Mock mode — admin access
    return {
      role: 'admin',
      canManageTenants: true,
      canRotateSecrets: true
    };
  }

  // Permit.io check — proxied through server-side API route (Cloud PDP blocks browser CORS)
  try {
    const resp = await fetch(`/api/permissions?email=${encodeURIComponent(userEmail)}`);
    const data = await resp.json();

    return {
      role: data.role || 'tenant',
      canManageTenants: data.canManageTenants === true,
      canRotateSecrets: data.canRotateSecrets === true
    };
  } catch (err) {
    console.error('[Permit.io] Error checking permissions:', err);
    // Fallback to tenant role on error
    return {
      role: 'tenant',
      canManageTenants: false,
      canRotateSecrets: false
    };
  }
}

/**
 * Check if a user is allowed to perform an action on a resource.
 */
export async function checkPermission(
  userEmail: string,
  action: string,
  resource: string
): Promise<boolean> {
  if (!isPermitEnabled()) return true; // Mock mode — allow all

  try {
    const resp = await fetch('https://cloudpdp.api.permit.io/allowed', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${PERMIT_API_KEY}`
      },
      body: JSON.stringify({
        user: { key: userEmail },
        action,
        resource: { type: resource }
      })
    });

    const data = await resp.json();
    return data.allow === true;
  } catch {
    return false;
  }
}
