import { api } from './client';

export interface HealthResponse {
  status: string;
  service: string;
}

export async function healthCheck(): Promise<HealthResponse> {
  return api<HealthResponse>('GET', '/health');
}
