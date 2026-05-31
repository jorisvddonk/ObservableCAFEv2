const BASE_URL = '';  // proxied via vite dev server; empty = same origin

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
  const res = await fetch(`${BASE_URL}${path}`, {
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
