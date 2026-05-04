export type AdminAuth = {
  username: string;
  password: string;
};

export const API_BASE_URL = (
  import.meta.env.VITE_API_BASE_URL || 'http://localhost:3000'
).replace(/\/$/, '');

export class ApiError extends Error {
  status: number;
  body: unknown;

  constructor(status: number, message: string, body?: unknown) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.body = body;
  }
}

function basicAuthHeader(auth: AdminAuth): string {
  const bytes = new TextEncoder().encode(`${auth.username}:${auth.password}`);
  let binary = '';
  bytes.forEach((byte) => {
    binary += String.fromCharCode(byte);
  });
  return `Basic ${btoa(binary)}`;
}

function endpoint(path: string): string {
  return `${API_BASE_URL}${path.startsWith('/') ? path : `/${path}`}`;
}

async function parseBody(response: Response): Promise<unknown> {
  const text = await response.text();
  if (!text) return undefined;

  const contentType = response.headers.get('content-type') || '';
  if (contentType.includes('application/json')) {
    try {
      return JSON.parse(text);
    } catch {
      return text;
    }
  }

  return text;
}

function errorMessage(status: number, body: unknown): string {
  if (body && typeof body === 'object') {
    const maybe = body as Record<string, unknown>;
    if (typeof maybe.errorMessage === 'string') return maybe.errorMessage;
    if (typeof maybe.error_message === 'string') return maybe.error_message;
    if (typeof maybe.message === 'string') return maybe.message;
  }

  if (typeof body === 'string' && body.trim().length > 0) return body;
  return `Request failed with HTTP ${status}`;
}

export async function request<T>(
  path: string,
  options: RequestInit & { auth: AdminAuth },
): Promise<T> {
  const { auth, headers, ...init } = options;
  const response = await fetch(endpoint(path), {
    ...init,
    headers: {
      Accept: 'application/json',
      Authorization: basicAuthHeader(auth),
      ...(init.body ? { 'Content-Type': 'application/json' } : {}),
      ...headers,
    },
  });

  const body = await parseBody(response);
  if (!response.ok) {
    throw new ApiError(response.status, errorMessage(response.status, body), body);
  }

  return body as T;
}

export function toQueryString(params: Record<string, string | boolean | undefined>): string {
  const search = new URLSearchParams();
  Object.entries(params).forEach(([key, value]) => {
    if (value !== undefined) search.set(key, String(value));
  });
  const text = search.toString();
  return text ? `?${text}` : '';
}
