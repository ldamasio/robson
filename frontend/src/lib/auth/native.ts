import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { OidcConfig } from "./oidc";

export type NativeAuthSession = {
  authenticated: boolean;
  expiresAt: number | null;
  returnPath?: string;
};

export type NativeHttpResponse = {
  status: number;
  statusText: string;
  body: string;
};

export type NativeStreamPayload = {
  streamId: string;
  data?: string;
  staleSecs?: number;
};

export type NativeOidcConfig = OidcConfig & { apiBase: string };

export function isNativeRuntime(): boolean {
  return typeof window !== "undefined" && isTauri();
}

export function beginNativeOidcLogin(
  config: NativeOidcConfig,
  returnPath: string,
): Promise<void> {
  return invoke("begin_oidc_login", { config, returnPath });
}

export function getNativeAuthSession(): Promise<NativeAuthSession> {
  return invoke("native_auth_session");
}

export function clearNativeAuthSession(): Promise<void> {
  return invoke("native_logout");
}

export function nativeApiRequest(
  path: string,
  init?: RequestInit,
): Promise<NativeHttpResponse> {
  const headers = Object.fromEntries(new Headers(init?.headers).entries());
  return invoke("native_api_request", {
    request: {
      method: init?.method ?? "GET",
      path,
      headers,
      body: typeof init?.body === "string" ? init.body : null,
    },
  });
}

export function startNativeEventStream(streamId: string): Promise<void> {
  return invoke("native_start_event_stream", { streamId });
}

export function stopNativeEventStream(streamId: string): Promise<void> {
  return invoke("native_stop_event_stream", { streamId });
}

export function onNativeAuthSession(
  handler: (session: NativeAuthSession) => void,
): Promise<UnlistenFn> {
  return listen<NativeAuthSession>("rbx://auth-session", (event) =>
    handler(event.payload),
  );
}

export function onNativeAuthExpired(handler: () => void): Promise<UnlistenFn> {
  return listen("rbx://auth-expired", handler);
}

export function onNativeAuthError(
  handler: (message: string) => void,
): Promise<UnlistenFn> {
  return listen<{ message: string }>("rbx://auth-error", (event) =>
    handler(event.payload.message),
  );
}

export function onNativeStreamEvent(
  eventName: string,
  handler: (payload: NativeStreamPayload) => void,
): Promise<UnlistenFn> {
  return listen<NativeStreamPayload>(eventName, (event) =>
    handler(event.payload),
  );
}
