import { api } from './client';

export interface Webhook {
  id: string;
  url: string;
  event_types: string[];
  active: boolean;
  created_at: number;
}

export interface WebhookCreated extends Webhook {
  secret: string;
  message: string;
}

export async function getWebhooks(): Promise<Webhook[]> {
  const data = await api<{ webhooks: Webhook[] }>('GET', '/api/webhooks');
  return data.webhooks;
}

export async function registerWebhook(url: string, eventTypes?: string[]): Promise<WebhookCreated> {
  const body: Record<string, unknown> = { url };
  if (eventTypes) body.event_types = eventTypes;
  return api<WebhookCreated>('POST', '/api/webhooks', body);
}

export async function updateWebhook(
  id: string,
  updates: { url?: string; event_types?: string[]; active?: boolean }
): Promise<{ updated: boolean }> {
  return api('PATCH', `/api/webhooks/${id}`, updates);
}

export async function deleteWebhook(id: string): Promise<{ deleted: boolean }> {
  return api('DELETE', `/api/webhooks/${id}`);
}

export async function testWebhook(id: string): Promise<{ delivered: boolean; status_code?: number; error?: string }> {
  return api('POST', `/api/webhooks/${id}/test`);
}
