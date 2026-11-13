export interface FieldError {
  field: string;
  code: string;
  message: string;
  /** For query text: the `[start, end)` character range the message is about. */
  span?: [number, number];
}

interface ProblemBody {
  status?: number;
  code?: string;
  title?: string;
  detail?: string;
  request_id?: string;
  errors?: FieldError[];
  current?: unknown;
}

/** A failed API call, parsed from the server's RFC 9457 problem document. */
export class ApiError extends Error {
  readonly status: number;
  readonly code: string;
  readonly title: string;
  readonly detail: string;
  readonly requestId: string | undefined;
  readonly fieldErrors: FieldError[];
  /** On `version_conflict`: the entity as it is now on the server. */
  readonly current: unknown;

  constructor(status: number, body: ProblemBody | null, requestId?: string) {
    const detail = body?.detail ?? fallbackDetail(status);
    super(detail);
    this.name = "ApiError";
    this.status = status;
    this.code = body?.code ?? fallbackCode(status);
    this.title = body?.title ?? "Request failed";
    this.detail = detail;
    this.requestId = body?.request_id ?? requestId;
    this.fieldErrors = body?.errors ?? [];
    this.current = body?.current;
  }

  get isConflict(): boolean {
    return this.code === "version_conflict";
  }
}

/** The request never produced an HTTP response (offline, DNS, CORS, aborted connection). */
export class NetworkError extends Error {
  constructor() {
    super("Can't reach the server. Check your connection and try again.");
    this.name = "NetworkError";
  }
}

function fallbackCode(status: number): string {
  if (status === 401) return "unauthenticated";
  if (status === 403) return "forbidden";
  if (status === 404) return "not_found";
  if (status === 429) return "rate_limited";
  if (status >= 500) return "internal";
  return "request_failed";
}

function fallbackDetail(status: number): string {
  if (status === 401) return "Sign in to continue.";
  if (status === 403) return "You do not have permission to do that.";
  if (status === 404) return "That doesn't exist, or you don't have access to it.";
  if (status === 429) return "Too many requests. Wait a moment and try again.";
  if (status === 504 || status === 503) return "The service is temporarily unavailable. Try again shortly.";
  return "Something went wrong. Try again.";
}

/** Message suitable for showing to a person, whatever was thrown. */
export function errorMessage(error: unknown): string {
  if (error instanceof ApiError || error instanceof NetworkError) return error.message;
  return "Something went wrong. Try again.";
}
