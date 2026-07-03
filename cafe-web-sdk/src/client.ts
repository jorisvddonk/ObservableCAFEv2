let _baseUrl = '';
let _token = '';

export function configure(baseUrl: string, token: string): void {
  _baseUrl = baseUrl.replace(/\/+$/, '');
  _token = token;
}

export function getBaseUrl(): string {
  return _baseUrl;
}

export function setToken(token: string): void {
  _token = token;
  try { localStorage.setItem('cafe_token', token); } catch { /* noop */ }
}

export function getToken(): string {
  return _token;
}

/** Low-level fetch wrapper — injects auth + JSON headers, returns parsed JSON. */
export async function apiFetch<T>(path: string, options: RequestInit = {}): Promise<T> {
  const url = path.startsWith('http') ? path : `${_baseUrl}${path}`;
  const res = await fetch(url, {
    ...options,
    headers: {
      'Content-Type': 'application/json',
      ...(_token ? { Authorization: `Bearer ${_token}` } : {}),
      ...(options.headers as Record<string, string>),
    },
  });
  if (!res.ok) {
    const body = await res.text();
    throw new Error(`${res.status} ${res.statusText}: ${body}`);
  }
  return res.json() as Promise<T>;
}
