function resolveBaseUrl(): string {
  if (typeof window === 'undefined') return '';
  const explicit = (window as any).__CAFE_API_URL__;
  if (typeof explicit === 'string' && explicit.length > 0) {
    return explicit;
  }
  const origin = window.location.origin;
  const m = origin.match(/^(https?:\/\/[^:]+):(\d+)$/);
  if (m) {
    const port = parseInt(m[2], 10);
    if (port === 8081 || port === 8080) {
      return `${m[1]}:4000`;
    }
    return origin;
  }
  return `${origin}:4000`;
}

const BASE_URL = resolveBaseUrl();

let _token = localStorage.getItem('cafe_token') ?? '';

export function setToken(token: string) {
  _token = token;
  localStorage.setItem('cafe_token', token);
}

export function getToken(): string {
  return _token;
}

export async function apiFetch<T>(
  path: string,
  options: RequestInit = {},
): Promise<T> {
  const url = path.startsWith('http') ? path : `${BASE_URL}${path}`;
  console.log('[apiFetch]', options.method || 'GET', url);
  const res = await fetch(url, {
    ...options,
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${_token}`,
      ...options.headers,
    },
  });

  if (!res.ok) {
    const body = await res.text();
    throw new Error(`${res.status} ${res.statusText}: ${body}`);
  }

  return res.json() as Promise<T>;
}

export function getApiBaseUrl(): string {
  return BASE_URL;
}

export function clearApiOverride(): void {
  try {
    sessionStorage.removeItem('cafe_api_url');
  } catch {
    // ignore
  }
}
