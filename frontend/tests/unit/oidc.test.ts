// @vitest-environment jsdom

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  buildAuthorizationUrl,
  handleOidcCallback,
  isOidcCallback,
  OIDC_TRANSACTION_STORAGE_KEY,
  resolveOidcConfig,
  type OidcConfig,
} from "$lib/auth/oidc";

const config: OidcConfig = {
  issuer: "https://identity.example",
  clientId: "robson-android",
  redirectUri: "br.ia.rbx.robson://oauth/callback",
  scopes: ["openid", "profile", "rbx:robson:observer"],
};

afterEach(() => {
  sessionStorage.clear();
  vi.unstubAllGlobals();
});

describe("RBX Identity OIDC", () => {
  it("requires a complete public-client configuration", () => {
    expect(
      resolveOidcConfig({
        PUBLIC_RBX_OIDC_ISSUER: "https://identity.example/",
        PUBLIC_RBX_OIDC_CLIENT_ID: "robson-android",
        PUBLIC_RBX_OIDC_REDIRECT_URI: "br.ia.rbx.robson://oauth/callback",
        PUBLIC_RBX_OIDC_SCOPES: "openid profile openid",
      }),
    ).toEqual({
      issuer: "https://identity.example",
      clientId: "robson-android",
      redirectUri: "br.ia.rbx.robson://oauth/callback",
      scopes: ["openid", "profile"],
    });

    expect(() =>
      resolveOidcConfig({
        PUBLIC_RBX_OIDC_ISSUER: "https://identity.example",
      }),
    ).toThrow("incomplete");
    expect(() =>
      resolveOidcConfig({
        PUBLIC_RBX_OIDC_ISSUER: "http://identity.example",
        PUBLIC_RBX_OIDC_CLIENT_ID: "robson-android",
        PUBLIC_RBX_OIDC_REDIRECT_URI: "br.ia.rbx.robson://oauth/callback",
        PUBLIC_RBX_OIDC_SCOPES: "openid",
      }),
    ).toThrow("HTTPS");
  });

  it("builds Authorization Code plus PKCE without a client secret", () => {
    const url = new URL(
      buildAuthorizationUrl(
        config,
        {
          issuer: config.issuer,
          authorization_endpoint: "https://identity.example/oauth/v2/authorize",
          token_endpoint: "https://identity.example/oauth/v2/token",
        },
        "state-value",
        "challenge-value",
      ),
    );

    expect(url.searchParams.get("response_type")).toBe("code");
    expect(url.searchParams.get("code_challenge_method")).toBe("S256");
    expect(url.searchParams.get("code_challenge")).toBe("challenge-value");
    expect(url.searchParams.get("state")).toBe("state-value");
    expect(url.searchParams.has("client_secret")).toBe(false);
  });

  it("recognizes responses only on the registered callback", () => {
    expect(
      isOidcCallback(
        "br.ia.rbx.robson://oauth/callback?code=code-1",
        config.redirectUri,
      ),
    ).toBe(true);
    expect(
      isOidcCallback(
        "br.ia.rbx.robson://unrelated/path?code=code-1",
        config.redirectUri,
      ),
    ).toBe(false);
  });

  it("rejects a callback with the wrong state before token exchange", async () => {
    storeTransaction("expected-state");
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);

    await expect(
      handleOidcCallback(
        "br.ia.rbx.robson://oauth/callback?code=code-1&state=wrong-state",
        config,
      ),
    ).rejects.toThrow("state validation failed");
    expect(fetchMock).not.toHaveBeenCalled();
    expect(sessionStorage.getItem(OIDC_TRANSACTION_STORAGE_KEY)).toBeNull();
  });

  it("exchanges a valid callback with the original PKCE verifier", async () => {
    storeTransaction("expected-state");
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          access_token: "signed-access-token",
          token_type: "Bearer",
          expires_in: 900,
        }),
        { status: 200, headers: { "Content-Type": "application/json" } },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    const result = await handleOidcCallback(
      "br.ia.rbx.robson://oauth/callback?code=code-1&state=expected-state&iss=https%3A%2F%2Fidentity.example",
      config,
    );

    expect(result).toEqual({
      accessToken: "signed-access-token",
      expiresIn: 900,
      returnPath: "/dashboard",
    });
    const request = fetchMock.mock.calls[0];
    expect(request[0]).toBe("https://identity.example/oauth/v2/token");
    const body = request[1]?.body as URLSearchParams;
    expect(body.get("code_verifier")).toBe("pkce-verifier");
    expect(body.get("client_secret")).toBeNull();
  });
});

function storeTransaction(state: string): void {
  sessionStorage.setItem(
    OIDC_TRANSACTION_STORAGE_KEY,
    JSON.stringify({
      state,
      codeVerifier: "pkce-verifier",
      issuer: config.issuer,
      tokenEndpoint: "https://identity.example/oauth/v2/token",
      redirectUri: config.redirectUri,
      returnPath: "/dashboard",
      createdAt: Date.now(),
    }),
  );
}
