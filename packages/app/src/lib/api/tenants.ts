import { api } from './client';

export interface Tenant {
  id: string;
  name: string;
  plan: string;
  email: string | null;
  key_prefix: string;
  created_at: number;
}

export interface TenantWithSecrets extends Tenant {
  hmac_secret: string;
  encryption_key: string;
}

export async function getTenants(): Promise<Tenant[]> {
  const data = await api<{ tenants: Tenant[] }>('GET', '/api/tenants');
  return data.tenants;
}

export async function getTenant(id: string): Promise<Tenant> {
  return api<Tenant>('GET', `/api/tenants/${id}`);
}

export async function createTenant(name: string, plan?: string, email?: string): Promise<TenantWithSecrets> {
  const body: Record<string, string> = { name };
  if (plan) body.plan = plan;
  if (email) body.email = email;
  return api<TenantWithSecrets>('POST', '/api/tenants', body);
}

export async function updateTenant(id: string, updates: { name?: string; plan?: string; email?: string }): Promise<{ updated: boolean }> {
  return api('PATCH', `/api/tenants/${id}`, updates);
}

export async function rotateSecrets(id: string): Promise<{ hmac_secret: string; encryption_key: string; message: string }> {
  return api('POST', `/api/tenants/${id}/rotate-secrets`);
}
