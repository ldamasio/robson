// @vitest-environment jsdom

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  buildAuthorizationUrl,
  handleWebOidcCallback,
  isOidcCallback,
  OIDC_TRANSACTION_STORAGE_KEY,
  resolveOidcConfig,
  type OidcConfig,
} from "#lib/auth/oidc.ts";

const config: OidcConfig = {
  issuer: "https://identity.example",
  clientId: "robson-web",
  redirectUri: "https://robson.example/auth/callback",
  scopes: ["openid", "profile"],
  orgId: "375992756025229511",
  projectId: "123456789012345678",
};

afterEach(() => {
  sessionStorage.clear();
  vi.unstubAllGlobals();
});

describe("RBX Identity OIDC", () => {
  it("requires complete organization-scoped public-client metadata", () => {
    expect(
      resolveOidcConfig({
        PUBLIC_RBX_OIDC_ISSUER: "https://identity.example/",
        PUBLIC_RBX_OIDC_CLIENT_ID: "robson-web",
        PUBLIC_RBX_OIDC_REDIRECT_URI: "https://robson.example/auth/callback",
        PUBLIC_RBX_OIDC_SCOPES: "openid profile openid",
        PUBLIC_RBX_OIDC_ORG_ID: "375992756025229511",
        PUBLIC_RBX_OIDC_PROJECT_ID: "123456789012345678",
      }),
    ).toEqual({
      issuer: "https://identity.example",
      clientId: "robson-web",
      redirectUri: "https://robson.example/auth/callback",
      scopes: ["openid", "profile"],
      orgId: "375992756025229511",
      projectId: "123456789012345678",
      googleIdpId: undefined,
    });

    expect(() =>
      resolveOidcConfig({
        PUBLIC_RBX_OIDC_ISSUER: "https://identity.example",
      }),
    ).toThrow("incomplete");
    expect(() =>
      resolveOidcConfig({
        PUBLIC_RBX_OIDC_ISSUER: "http://identity.example",
        PUBLIC_RBX_OIDC_CLIENT_ID: "robson-web",
        PUBLIC_RBX_OIDC_REDIRECT_URI: "https://robson.example/auth/callback",
        PUBLIC_RBX_OIDC_SCOPES: "openid",
        PUBLIC_RBX_OIDC_ORG_ID: "375992756025229511",
        PUBLIC_RBX_OIDC_PROJECT_ID: "123456789012345678",
      }),
    ).toThrow("HTTPS");
  });

  it("builds Authorization Code plus PKCE with org and audience scopes", () => {
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
    expect(url.searchParams.has("client_secret")).toBe(false);
    const scopes = url.searchParams.get("scope")?.split(" ") ?? [];
    expect(scopes).toContain("urn:zitadel:iam:org:id:375992756025229511");
    expect(scopes).toContain(
      "urn:zitadel:iam:org:project:id:123456789012345678:aud",
    );
    expect(scopes).toContain("urn:zitadel:iam:org:roles:id:375992756025229511");
  });

  it("adds the Google IdP scope only when its real id is configured", () => {
    const discovery = {
      issuer: config.issuer,
      authorization_endpoint: "https://identity.example/oauth/v2/authorize",
      token_endpoint: "https://identity.example/oauth/v2/token",
    };
    const withoutIdp = new URL(
      buildAuthorizationUrl(config, discovery, "state", "challenge"),
    );
    expect(withoutIdp.searchParams.get("scope")).not.toContain(
      "urn:zitadel:iam:org:idp:id:",
    );

    const withIdp = new URL(
      buildAuthorizationUrl(
        { ...config, googleIdpId: "987654321098765432" },
        discovery,
        "state",
        "challenge",
      ),
    );
    expect(withIdp.searchParams.get("scope")).toContain(
      "urn:zitadel:iam:org:idp:id:987654321098765432",
    );
  });

  it("recognizes responses only on the registered callback", () => {
    expect(
      isOidcCallback(
        "https://robson.example/auth/callback?code=code-1",
        config.redirectUri,
      ),
    ).toBe(true);
    expect(
      isOidcCallback(
        "https://robson.example/unrelated?code=code-1",
        config.redirectUri,
      ),
    ).toBe(false);
  });

  it("rejects the wrong state before token exchange", async () => {
    storeTransaction("expected-state");
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);

    await expect(
      handleWebOidcCallback(
        "https://robson.example/auth/callback?code=code-1&state=wrong-state",
        config,
      ),
    ).rejects.toThrow("state validation failed");
    expect(fetchMock).not.toHaveBeenCalled();
    expect(sessionStorage.getItem(OIDC_TRANSACTION_STORAGE_KEY)).toBeNull();
  });

  it("exchanges a valid callback with the original verifier", async () => {
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

    const result = await handleWebOidcCallback(
      "https://robson.example/auth/callback?code=code-1&state=expected-state&iss=https%3A%2F%2Fidentity.example",
      config,
    );

    expect(result).toEqual({
      accessToken: "signed-access-token",
      expiresIn: 900,
      returnPath: "/dashboard",
    });
    const request = fetchMock.mock.calls[0];
    const body = request[1]?.body as URLSearchParams;
    expect(body.get("code_verifier")).toBe("pkce-verifier");
    expect(body.get("client_secret")).toBeNull();
    expect(sessionStorage.getItem(OIDC_TRANSACTION_STORAGE_KEY)).toBeNull();
    expect(Object.values(sessionStorage)).not.toContain("signed-access-token");
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
