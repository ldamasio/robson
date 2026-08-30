import { describe, expect, it } from "vitest";
import { invalidAndroidIdentityVariables } from "../../scripts/validate-android-identity-config.mjs";

const completeConfiguration = {
  PUBLIC_RBX_OIDC_ISSUER: "https://identity.example",
  PUBLIC_RBX_OIDC_CLIENT_ID: "robson-native",
  PUBLIC_RBX_OIDC_REDIRECT_URI: "br.ia.rbx.robson://oauth/callback",
  PUBLIC_RBX_OIDC_SCOPES:
    "openid profile email urn:example:audience urn:example:role",
};

describe("Android RBX Identity build configuration", () => {
  it("accepts complete public-client metadata", () => {
    expect(invalidAndroidIdentityVariables(completeConfiguration)).toEqual([]);
  });

  it("rejects the repository's intentionally incomplete profile", () => {
    expect(
      invalidAndroidIdentityVariables({
        ...completeConfiguration,
        PUBLIC_RBX_OIDC_CLIENT_ID: "",
        PUBLIC_RBX_OIDC_SCOPES: "openid profile email",
      }),
    ).toEqual(["PUBLIC_RBX_OIDC_CLIENT_ID", "PUBLIC_RBX_OIDC_SCOPES"]);
  });

  it("rejects insecure issuers and the wrong native callback", () => {
    expect(
      invalidAndroidIdentityVariables({
        ...completeConfiguration,
        PUBLIC_RBX_OIDC_ISSUER: "http://identity.example",
        PUBLIC_RBX_OIDC_REDIRECT_URI: "https://example.test/callback",
      }),
    ).toEqual(["PUBLIC_RBX_OIDC_ISSUER", "PUBLIC_RBX_OIDC_REDIRECT_URI"]);
  });
});
