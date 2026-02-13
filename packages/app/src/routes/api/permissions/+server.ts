/**
 * Server-side Permit.io permission check endpoint.
 *
 * Queries the Permit.io Management API to look up the user's role assignments,
 * then derives permissions from the role. The Cloud PDP is not used because
 * it has propagation delays and CORS restrictions.
 *
 * GET /api/permissions?email=user@example.com
 */

import { json } from '@sveltejs/kit';
import type { RequestHandler } from './$types';

const PERMIT_API_KEY = process.env.VITE_PERMIT_API_KEY || import.meta.env.VITE_PERMIT_API_KEY || '';

export const GET: RequestHandler = async ({ url }) => {
  const email = url.searchParams.get('email');

  if (!email) {
    return json({ role: 'tenant', canManageTenants: false, canRotateSecrets: false });
  }

  if (!PERMIT_API_KEY) {
    // Mock mode — admin access
    return json({ role: 'admin', canManageTenants: true, canRotateSecrets: true });
  }

  try {
    // Query user's role assignments from the Permit.io Management API
    const resp = await fetch(
      `https://api.permit.io/v2/facts/default/production/role_assignments?user=${encodeURIComponent(email)}`,
      {
        headers: {
          Authorization: `Bearer ${PERMIT_API_KEY}`
        }
      }
    );

    if (!resp.ok) {
      console.error('[Permit.io Server] Role assignments lookup failed:', resp.status);
      return json({ role: 'tenant', canManageTenants: false, canRotateSecrets: false });
    }

    const assignments = await resp.json();

    // Check if any assignment has the "admin" role
    const isAdmin = Array.isArray(assignments)
      && assignments.some((a: { role: string }) => a.role === 'admin');

    return json({
      role: isAdmin ? 'admin' : 'tenant',
      canManageTenants: isAdmin,
      canRotateSecrets: isAdmin
    });
  } catch (err) {
    console.error('[Permit.io Server] Error:', err);
    return json({ role: 'tenant', canManageTenants: false, canRotateSecrets: false });
  }
};
