import { ApiError, NetworkError } from "./errors";

/**
 * The session layer plugs in here. Keeping this as an injected dependency means the
 * HTTP client has no import cycle with session state and is trivially testable.
 */
interface AuthBridge {
  /** Current in-memory access token, if any. */
  getToken: () => string | null;
  /** Obtain a fresh access token (single-flight). Resolves false when the session is over. */
  refresh: () => Promise<boolean>;
}

let bridge: AuthBridge = { getToken: () => null, refresh: async () => false };

export function configureAuth(next: AuthBridge): void {
  bridge = next;
}

export interface RequestOptions {
  body?: unknown;
  query?: Record<string, string | number | undefined | null>;
  /** Version the edit is based on; sent as `If-Match` for optimistic concurrency. */
  ifMatch?: number;
  /** false for endpoints that authenticate by cookie or not at all. */
  auth?: boolean;
  signal?: AbortSignal;
}

export interface ApiResponse<T> {
  data: T;
  /** Entity version from the `ETag` header, when the resource is versioned. */
  version: number | undefined;
}

function buildUrl(path: string, query: RequestOptions["query"]): string {
  if (!query) return path;
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined && value !== null && value !== "") params.set(key, String(value));
  }
  const qs = params.toString();
  return qs ? `${path}?${qs}` : path;
}

async function send(method: string, path: string, options: RequestOptions): Promise<Response> {
  const headers: Record<string, string> = { Accept: "application/json" };
  const isForm = options.body instanceof FormData;
  // For multipart the browser sets Content-Type itself (it carries the boundary).
  if (options.body !== undefined && !isForm) headers["Content-Type"] = "application/json";
  if (options.ifMatch !== undefined) headers["If-Match"] = `"${options.ifMatch}"`;
  const token = options.auth === false ? null : bridge.getToken();
  if (token) headers.Authorization = `Bearer ${token}`;
  try {
    return await fetch(buildUrl(path, options.query), {
      method,
      headers,
      body: options.body === undefined ? undefined : isForm ? (options.body as FormData) : JSON.stringify(options.body),
      // The refresh cookie is scoped to /api/v1/auth; same-origin is enough to send it.
      credentials: "same-origin",
      signal: options.signal,
    });
  } catch (error) {
    if (error instanceof DOMException && error.name === "AbortError") throw error;
    throw new NetworkError();
  }
}

async function parse<T>(response: Response): Promise<ApiResponse<T>> {
  const requestId = response.headers.get("x-request-id") ?? undefined;
  const text = response.status === 204 ? "" : await response.text();
  let body: unknown = null;
  if (text) {
    try {
      body = JSON.parse(text);
    } catch {
      body = null;
    }
  }
  if (!response.ok) throw new ApiError(response.status, body as never, requestId);
  const etag = response.headers.get("etag");
  const version = etag ? Number(etag.replace(/^W\//, "").replace(/"/g, "")) : undefined;
  return { data: body as T, version: Number.isFinite(version) ? version : undefined };
}

/**
 * Perform a request. On 401 the access token is refreshed once (shared between all
 * concurrent callers) and the request is replayed; a second 401 is final.
 */
export async function requestWithMeta<T>(method: string, path: string, options: RequestOptions = {}): Promise<ApiResponse<T>> {
  let response = await send(method, path, options);
  if (response.status === 401 && options.auth !== false) {
    const refreshed = await bridge.refresh();
    if (refreshed) response = await send(method, path, options);
  }
  return parse<T>(response);
}

export async function request<T>(method: string, path: string, options: RequestOptions = {}): Promise<T> {
  return (await requestWithMeta<T>(method, path, options)).data;
}

/** Fetch a binary resource (e.g. an attachment) with the same auth and refresh handling. */
export async function download(path: string): Promise<Blob> {
  let response = await send("GET", path, {});
  if (response.status === 401 && (await bridge.refresh())) response = await send("GET", path, {});
  if (!response.ok) {
    let body: unknown = null;
    try {
      body = await response.json();
    } catch {
      body = null;
    }
    throw new ApiError(response.status, body as never, response.headers.get("x-request-id") ?? undefined);
  }
  return response.blob();
}

/**
 * Open a Server-Sent Events stream with the session's bearer token (EventSource cannot
 * send headers). Refreshes once on 401. The caller reads `response.body`.
 */
export async function openStream(path: string, query: Record<string, string>, lastEventId: string | undefined, signal: AbortSignal): Promise<Response> {
  const open = () => {
    const headers: Record<string, string> = { Accept: "text/event-stream" };
    const token = bridge.getToken();
    if (token) headers.Authorization = `Bearer ${token}`;
    if (lastEventId) headers["Last-Event-ID"] = lastEventId;
    return fetch(buildUrl(path, query), { headers, credentials: "same-origin", signal, cache: "no-store" });
  };
  let response = await open();
  if (response.status === 401 && (await bridge.refresh())) response = await open();
  return response;
}

export const api = {
  get: <T>(path: string, options?: RequestOptions) => request<T>("GET", path, options),
  post: <T>(path: string, body?: unknown, options?: RequestOptions) => request<T>("POST", path, { ...options, body }),
  patch: <T>(path: string, body?: unknown, options?: RequestOptions) => request<T>("PATCH", path, { ...options, body }),
  /** Multipart upload; `form` is sent as-is. */
  upload: <T>(path: string, form: FormData) => request<T>("POST", path, { body: form }),
  put: <T>(path: string, body?: unknown, options?: RequestOptions) => request<T>("PUT", path, { ...options, body }),
  delete: <T = void>(path: string, options?: RequestOptions) => request<T>("DELETE", path, options),
};
