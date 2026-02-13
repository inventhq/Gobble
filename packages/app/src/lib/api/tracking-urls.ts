import { api } from './client';

export interface TrackingUrl {
  id: string;
  destination: string;
  clicks: number;
  postbacks: number;
  impressions: number;
  created_at: number;
}

export interface TrackingUrlsResponse {
  tracking_urls: TrackingUrl[];
  count: number;
  limit: number;
  offset: number;
}

export async function getTrackingUrls(opts?: {
  limit?: number;
  offset?: number;
}): Promise<TrackingUrlsResponse> {
  const params = new URLSearchParams();
  if (opts?.limit) params.set('limit', String(opts.limit));
  if (opts?.offset) params.set('offset', String(opts.offset));
  const qs = params.toString();
  return api<TrackingUrlsResponse>('GET', `/api/tracking-urls${qs ? `?${qs}` : ''}`);
}

export async function getTrackingUrl(id: string): Promise<TrackingUrl> {
  return api<TrackingUrl>('GET', `/api/tracking-urls/${id}`);
}

export async function createTrackingUrl(
  destination: string,
  tenantId?: string
): Promise<{ id: string; destination: string }> {
  const body: Record<string, string> = { destination };
  if (tenantId) body.tenant_id = tenantId;
  return api('POST', '/api/tracking-urls', body);
}

export async function updateTrackingUrl(
  id: string,
  destination: string
): Promise<{ id: string; destination: string; updated: boolean }> {
  return api('PATCH', `/api/tracking-urls/${id}`, { destination });
}

export async function deleteTrackingUrl(id: string): Promise<{ deleted: boolean }> {
  return api('DELETE', `/api/tracking-urls/${id}`);
}
