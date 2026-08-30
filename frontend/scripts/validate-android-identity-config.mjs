import { pathToFileURL } from "node:url";
import { loadEnv } from "vite";

const STANDARD_SCOPES = new Set(["openid", "profile", "email"]);

/**
 * @param {Record<string, string | undefined>} environment
 * @returns {string[]}
 */
export function invalidAndroidIdentityVariables(environment) {
  const issuer = environment.PUBLIC_RBX_OIDC_ISSUER?.trim() ?? "";
  const clientId = environment.PUBLIC_RBX_OIDC_CLIENT_ID?.trim() ?? "";
  const redirectUri = environment.PUBLIC_RBX_OIDC_REDIRECT_URI?.trim() ?? "";
  const scopesValue = environment.PUBLIC_RBX_OIDC_SCOPES?.trim() ?? "";
  const invalid = [];

  if (!isValidHttpsIssuer(issuer)) {
    invalid.push("PUBLIC_RBX_OIDC_ISSUER");
  }
  if (!clientId || clientId.includes("REPLACE_WITH_")) {
    invalid.push("PUBLIC_RBX_OIDC_CLIENT_ID");
  }
  if (redirectUri !== "br.ia.rbx.robson://oauth/callback") {
    invalid.push("PUBLIC_RBX_OIDC_REDIRECT_URI");
  }

  const scopes = [...new Set(scopesValue.split(/\s+/).filter(Boolean))];
  const customScopes = scopes.filter((scope) => !STANDARD_SCOPES.has(scope));
  if (
    !scopes.includes("openid") ||
    customScopes.length < 2 ||
    scopes.some((scope) => scope.includes("REPLACE_WITH_"))
  ) {
    invalid.push("PUBLIC_RBX_OIDC_SCOPES");
  }

  return invalid;
}

/** @param {string} value */
function isValidHttpsIssuer(value) {
  if (!value || value.includes("REPLACE_WITH_")) return false;
  try {
    const issuer = new URL(value);
    return (
      issuer.protocol === "https:" &&
      issuer.username === "" &&
      issuer.password === "" &&
      issuer.search === "" &&
      issuer.hash === ""
    );
  } catch {
    return false;
  }
}

function validateConfiguredEnvironment() {
  const environment = loadEnv("android", process.cwd(), "PUBLIC_");
  const invalid = invalidAndroidIdentityVariables(environment);
  if (invalid.length > 0) {
    console.error(
      `Android RBX Identity metadata is incomplete: ${invalid.join(", ")}`,
    );
    process.exitCode = 1;
    return;
  }
  console.log("Android RBX Identity public metadata is structurally complete.");
}

const entryUrl = process.argv[1] ? pathToFileURL(process.argv[1]).href : "";
if (import.meta.url === entryUrl) {
  validateConfiguredEnvironment();
}
