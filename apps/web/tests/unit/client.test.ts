import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { api, ApiError, configureAuth, NetworkError, requestWithMeta } from "@/services/api";

function json(status: number, body: unknown, headers: Record<string, string> = {}): Response {
  return new Response(status === 204 ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json", ...headers } });
}

describe("api client", () => {
  const fetchMock = vi.fn<typeof fetch>();
  let token: string | null;

  beforeEach(() => {
    token = "stale";
    vi.stubGlobal("fetch", fetchMock);
  });
  afterEach(() => {
    fetchMock.mockReset();
    vi.unstubAllGlobals();
  });

  it("attaches the bearer token and parses the body", async () => {
    configureAuth({ getToken: () => token, refresh: async () => false });
    fetchMock.mockResolvedValueOnce(json(200, { ok: true }));
    await expect(api.get("/api/v1/x", { query: { limit: 5, cursor: undefined } })).resolves.toEqual({ ok: true });
    const [url, init] = fetchMock.mock.calls[0]!;
    expect(url).toBe("/api/v1/x?limit=5");
    expect((init!.headers as Record<string, string>).Authorization).toBe("Bearer stale");
  });

  it("refreshes once on 401 and replays with the new token; concurrent callers share one refresh", async () => {
    const refresh = vi.fn(async () => {
      token = "fresh";
      return true;
    });
    let inflight: Promise<boolean> | null = null;
    configureAuth({ getToken: () => token, refresh: () => (inflight ??= refresh().finally(() => (inflight = null))) });
    fetchMock.mockImplementation(async (_url, init) => {
      const auth = (init!.headers as Record<string, string>).Authorization;
      return auth === "Bearer fresh" ? json(200, { ok: true }) : json(401, { code: "unauthenticated", detail: "expired" });
    });
    const results = await Promise.all([api.get("/a"), api.get("/b"), api.get("/c")]);
    expect(results).toEqual([{ ok: true }, { ok: true }, { ok: true }]);
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("gives up after a failed refresh instead of looping", async () => {
    configureAuth({ getToken: () => token, refresh: async () => false });
    fetchMock.mockResolvedValue(json(401, { code: "unauthenticated", detail: "Sign in to continue." }));
    await expect(api.get("/a")).rejects.toMatchObject({ status: 401, code: "unauthenticated" });
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("does not try to refresh for cookie/public endpoints", async () => {
    const refresh = vi.fn(async () => true);
    configureAuth({ getToken: () => token, refresh });
    fetchMock.mockResolvedValue(json(401, {}));
    await expect(api.post("/api/v1/auth/login", {}, { auth: false })).rejects.toBeInstanceOf(ApiError);
    expect(refresh).not.toHaveBeenCalled();
    expect((fetchMock.mock.calls[0]![1]!.headers as Record<string, string>).Authorization).toBeUndefined();
  });

  it("parses problem documents, field errors and the conflicting entity", async () => {
    configureAuth({ getToken: () => null, refresh: async () => false });
    fetchMock.mockResolvedValueOnce(
      json(422, { code: "validation_failed", detail: "One or more fields are invalid.", request_id: "r1", errors: [{ field: "email", code: "x", message: "Bad" }] }),
    );
    const invalid = await api.post("/x", {}).catch((e: unknown) => e);
    expect(invalid).toBeInstanceOf(ApiError);
    expect((invalid as ApiError).fieldErrors[0]).toMatchObject({ field: "email", message: "Bad" });
    expect((invalid as ApiError).requestId).toBe("r1");

    fetchMock.mockResolvedValueOnce(json(409, { code: "version_conflict", current: { name: "Theirs", version: 3 } }));
    const conflict = (await api.patch("/x", {}, { ifMatch: 2 }).catch((e: unknown) => e)) as ApiError;
    expect(conflict.isConflict).toBe(true);
    expect(conflict.current).toEqual({ name: "Theirs", version: 3 });
    expect((fetchMock.mock.calls[1]![1]!.headers as Record<string, string>)["If-Match"]).toBe('"2"');
  });

  it("exposes the ETag version and handles 204 and non-JSON errors", async () => {
    configureAuth({ getToken: () => null, refresh: async () => false });
    fetchMock.mockResolvedValueOnce(json(200, { id: 1 }, { etag: '"7"' }));
    expect((await requestWithMeta("GET", "/x")).version).toBe(7);
    fetchMock.mockResolvedValueOnce(json(204, null));
    await expect(api.delete("/x")).resolves.toBeNull();
    fetchMock.mockResolvedValueOnce(new Response("<html>bad gateway</html>", { status: 502 }));
    await expect(api.get("/x")).rejects.toMatchObject({ status: 502, code: "internal" });
  });

  it("turns transport failures into NetworkError", async () => {
    configureAuth({ getToken: () => null, refresh: async () => false });
    fetchMock.mockRejectedValueOnce(new TypeError("Failed to fetch"));
    await expect(api.get("/x")).rejects.toBeInstanceOf(NetworkError);
  });
});
