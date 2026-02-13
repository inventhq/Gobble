import { api } from './client';

export interface ApiKey {
  id: string;
  key_prefix: string;
  name: string;
  tenant_id?: string;
  last_used_at: number | null;
  created_at: number;
}

export interface ApiKeyCreated {
  id: string;
  key: string;
  key_prefix: string;
  name: string;
  tenant_id: string;
  message: string;
}

export async function getKeys(): Promise<ApiKey[]> {
  const data = await api<{ keys: ApiKey[] }>('GET', '/api/keys');
  return data.keys;
}

export async function createKey(tenantId: string, name?: string): Promise<ApiKeyCreated> {
  const body: Record<string, string> = { tenant_id: tenantId };
  if (name) body.name = name;
  return api<ApiKeyCreated>('POST', '/api/keys', body);
}

export async function revokeKey(id: string): Promise<{ deleted: boolean }> {
  return api('DELETE', `/api/keys/${id}`);
}
