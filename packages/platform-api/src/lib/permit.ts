/**
 * Permit.io auto-provisioning helper.
 *
 * When a new tenant is created with an email, this module automatically
 * creates the user in Permit.io and assigns them the "tenant" role.
 *
 * The user's email is used as the Permit.io user key, matching the
 * dashboard's auth flow (Stytch email → Permit.io key).
 *
 * Requires PERMIT_API_KEY environment variable. When not set, provisioning
 * is silently skipped (useful for local development without Permit.io).
 */

const PERMIT_API_BASE = "https://api.permit.io/v2";

/**
 * Sync a user to Permit.io with the given role.
 * Creates the user if they don't exist, then assigns the role.
 *
 * @param apiKey - Permit.io API key
 * @param email - User's email (used as the Permit.io user key)
 * @param role - Role to assign (e.g. "tenant" or "admin")
 */
export async function syncUserToPermit(
  apiKey: string,
  email: string,
  role: string = "tenant"
): Promise<{ success: boolean; error?: string }> {
  if (!apiKey || !email) {
    return { success: false, error: "Missing API key or email" };
  }

  const headers = {
    "Content-Type": "application/json",
    Authorization: `Bearer ${apiKey}`,
  };

  try {
    // Step 1: Create or update the user
    const userResp = await fetch(`${PERMIT_API_BASE}/facts/default/default/users`, {
      method: "POST",
      headers,
      body: JSON.stringify({
        key: email,
        email: email,
      }),
    });

    if (!userResp.ok && userResp.status !== 409) {
      const err = await userResp.text();
      console.error("Permit.io user creation failed:", userResp.status, err);
      return { success: false, error: `User creation failed: ${userResp.status}` };
    }

    // Step 2: Assign the role
    const roleResp = await fetch(
      `${PERMIT_API_BASE}/facts/default/default/role_assignments`,
      {
        method: "POST",
        headers,
        body: JSON.stringify({
          user: email,
          role: role,
          tenant: "default",
        }),
      }
    );

    if (!roleResp.ok && roleResp.status !== 409) {
      const err = await roleResp.text();
      console.error("Permit.io role assignment failed:", roleResp.status, err);
      return {
        success: false,
        error: `Role assignment failed: ${roleResp.status}`,
      };
    }

    return { success: true };
  } catch (err) {
    console.error("Permit.io sync error:", err);
    return {
      success: false,
      error: err instanceof Error ? err.message : "Unknown error",
    };
  }
}
