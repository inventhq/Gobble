import { API_URL } from '$lib/utils/constants';

let _apiKey = '';

export function setApiKey(key: string) {
  _apiKey = key;
}

export function getApiKey(): string {
  return _apiKey;
}

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string
  ) {
    super(message);
    this.name = 'ApiError';
  }
}

export async function api<T = unknown>(
  method: string,
  path: string,
  body?: unknown
): Promise<T> {
  const headers: Record<string, string> = {
    'Content-Type': 'application/json'
  };

  if (_apiKey) {
    headers['Authorization'] = `Bearer ${_apiKey}`;
  }

  const resp = await fetch(`${API_URL}${path}`, {
    method,
    headers,
    body: body ? JSON.stringify(body) : undefined
  });

  const data = await resp.json().catch(() => ({}));

  if (!resp.ok) {
    throw new ApiError(resp.status, (data as { error?: string }).error || `HTTP ${resp.status}`);
  }

  return data as T;
}
