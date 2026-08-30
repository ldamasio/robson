export type OidcConfig = {
  issuer: string;
  clientId: string;
  redirectUri: string;
  scopes: string[];
};

type OidcDiscovery = {
  issuer: string;
  authorization_endpoint: string;
  token_endpoint: string;
};

type OidcTransaction = {
  state: string;
  codeVerifier: string;
  issuer: string;
  tokenEndpoint: string;
  redirectUri: string;
  returnPath: string;
  createdAt: number;
};

type TokenResponse = {
  access_token?: unknown;
  token_type?: unknown;
  expires_in?: unknown;
};

export type OidcLoginResult = {
  accessToken: string;
  expiresIn: number | null;
  returnPath: string;
};

export const OIDC_TRANSACTION_STORAGE_KEY = "robson_oidc_transaction";
const TRANSACTION_TTL_MS = 10 * 60 * 1000;
const NETWORK_TIMEOUT_MS = 10_000;

export function resolveOidcConfig(
  environment: Record<string, string | undefined>,
): OidcConfig | null {
  const issuer = environment.PUBLIC_RBX_OIDC_ISSUER?.trim();
  const clientId = environment.PUBLIC_RBX_OIDC_CLIENT_ID?.trim();
  const redirectUri = environment.PUBLIC_RBX_OIDC_REDIRECT_URI?.trim();
  const scopes = environment.PUBLIC_RBX_OIDC_SCOPES?.trim();

  if (!issuer && !clientId && !redirectUri && !scopes) return null;
  if (!issuer || !clientId || !redirectUri || !scopes) {
    throw new Error("RBX Identity configuration is incomplete");
  }

  const normalizedIssuer = validateHttpsUrl(issuer, "issuer").replace(
    /\/$/,
    "",
  );
  validateRedirectUri(redirectUri);
  const parsedScopes = [...new Set(scopes.split(/\s+/).filter(Boolean))];
  if (!parsedScopes.includes("openid")) {
    throw new Error("RBX Identity scopes must include openid");
  }

  return {
    issuer: normalizedIssuer,
    clientId,
    redirectUri,
    scopes: parsedScopes,
  };
}

export async function beginOidcLogin(
  config: OidcConfig,
  returnPath: string,
): Promise<void> {
  const discovery = await discover(config);
  const state = randomBase64Url(32);
  const codeVerifier = randomBase64Url(64);
  const codeChallenge = await sha256Base64Url(codeVerifier);
  const transaction: OidcTransaction = {
    state,
    codeVerifier,
    issuer: config.issuer,
    tokenEndpoint: discovery.token_endpoint,
    redirectUri: config.redirectUri,
    returnPath: sanitizeReturnPath(returnPath),
    createdAt: Date.now(),
  };
  sessionStorage.setItem(
    OIDC_TRANSACTION_STORAGE_KEY,
    JSON.stringify(transaction),
  );

  const authorizationUrl = buildAuthorizationUrl(
    config,
    discovery,
    state,
    codeChallenge,
  );
  const { Capacitor } = await import("@capacitor/core");
  if (Capacitor.isNativePlatform()) {
    const { Browser } = await import("@capacitor/browser");
    await Browser.open({ url: authorizationUrl, toolbarColor: "#07080A" });
    return;
  }
  window.location.assign(authorizationUrl);
}

export async function handleOidcCallback(
  callbackUrl: string,
  config: OidcConfig,
): Promise<OidcLoginResult> {
  const callback = new URL(callbackUrl);
  const transaction = readTransaction();
  sessionStorage.removeItem(OIDC_TRANSACTION_STORAGE_KEY);

  if (Date.now() - transaction.createdAt > TRANSACTION_TTL_MS) {
    throw new Error("RBX Identity login expired; start again");
  }
  if (
    transaction.issuer !== config.issuer ||
    transaction.redirectUri !== config.redirectUri ||
    !sameCallbackTarget(callback, new URL(config.redirectUri))
  ) {
    throw new Error("RBX Identity callback does not match this application");
  }
  if (callback.searchParams.get("state") !== transaction.state) {
    throw new Error("RBX Identity state validation failed");
  }
  const responseIssuer = callback.searchParams.get("iss");
  if (responseIssuer && responseIssuer.replace(/\/$/, "") !== config.issuer) {
    throw new Error("RBX Identity issuer validation failed");
  }
  const providerError = callback.searchParams.get("error");
  if (providerError) {
    throw new Error(`RBX Identity rejected the login (${providerError})`);
  }
  const code = callback.searchParams.get("code");
  if (!code) throw new Error("RBX Identity callback is missing the code");

  const body = new URLSearchParams({
    grant_type: "authorization_code",
    client_id: config.clientId,
    code,
    code_verifier: transaction.codeVerifier,
    redirect_uri: config.redirectUri,
  });
  const response = await fetchWithTimeout(transaction.tokenEndpoint, {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body,
  });
  if (!response.ok) {
    throw new Error("RBX Identity token exchange failed");
  }
  const token = (await response.json()) as TokenResponse;
  if (
    typeof token.access_token !== "string" ||
    token.access_token.length === 0 ||
    typeof token.token_type !== "string" ||
    token.token_type.toLowerCase() !== "bearer"
  ) {
    throw new Error("RBX Identity returned an invalid token response");
  }

  return {
    accessToken: token.access_token,
    expiresIn:
      typeof token.expires_in === "number" && Number.isFinite(token.expires_in)
        ? token.expires_in
        : null,
    returnPath: transaction.returnPath,
  };
}

export function isOidcCallback(url: string, redirectUri?: string): boolean {
  try {
    const parsed = new URL(url);
    const hasResponse =
      parsed.searchParams.has("code") || parsed.searchParams.has("error");
    if (!hasResponse) return false;
    return redirectUri
      ? sameCallbackTarget(parsed, new URL(redirectUri))
      : true;
  } catch {
    return false;
  }
}

export function buildAuthorizationUrl(
  config: OidcConfig,
  discovery: OidcDiscovery,
  state: string,
  codeChallenge: string,
): string {
  const url = new URL(discovery.authorization_endpoint);
  url.searchParams.set("client_id", config.clientId);
  url.searchParams.set("redirect_uri", config.redirectUri);
  url.searchParams.set("response_type", "code");
  url.searchParams.set("scope", config.scopes.join(" "));
  url.searchParams.set("state", state);
  url.searchParams.set("code_challenge", codeChallenge);
  url.searchParams.set("code_challenge_method", "S256");
  return url.toString();
}

async function discover(config: OidcConfig): Promise<OidcDiscovery> {
  const response = await fetchWithTimeout(
    `${config.issuer}/.well-known/openid-configuration`,
  );
  if (!response.ok) throw new Error("RBX Identity discovery failed");
  const discovery = (await response.json()) as Partial<OidcDiscovery>;
  if (
    discovery.issuer?.replace(/\/$/, "") !== config.issuer ||
    !discovery.authorization_endpoint ||
    !discovery.token_endpoint
  ) {
    throw new Error("RBX Identity discovery response is invalid");
  }
  validateHttpsUrl(discovery.authorization_endpoint, "authorization endpoint");
  validateHttpsUrl(discovery.token_endpoint, "token endpoint");
  return discovery as OidcDiscovery;
}

function readTransaction(): OidcTransaction {
  const raw = sessionStorage.getItem(OIDC_TRANSACTION_STORAGE_KEY);
  if (!raw) throw new Error("RBX Identity login transaction was not found");
  try {
    const parsed = JSON.parse(raw) as Partial<OidcTransaction>;
    if (
      typeof parsed.state !== "string" ||
      typeof parsed.codeVerifier !== "string" ||
      typeof parsed.issuer !== "string" ||
      typeof parsed.tokenEndpoint !== "string" ||
      typeof parsed.redirectUri !== "string" ||
      typeof parsed.returnPath !== "string" ||
      typeof parsed.createdAt !== "number"
    ) {
      throw new Error("invalid transaction");
    }
    return parsed as OidcTransaction;
  } catch {
    throw new Error("RBX Identity login transaction is invalid");
  }
}

async function fetchWithTimeout(
  input: RequestInfo | URL,
  init?: RequestInit,
): Promise<Response> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), NETWORK_TIMEOUT_MS);
  try {
    return await fetch(input, { ...init, signal: controller.signal });
  } finally {
    clearTimeout(timer);
  }
}

function validateHttpsUrl(value: string, label: string): string {
  const url = new URL(value);
  if (
    url.protocol !== "https:" ||
    url.username ||
    url.password ||
    url.search ||
    url.hash
  ) {
    throw new Error(`${label} must be a credential-free HTTPS URL`);
  }
  return url.toString();
}

function validateRedirectUri(value: string): void {
  const url = new URL(value);
  const isHttps = url.protocol === "https:";
  const isRobsonNative =
    url.protocol === "br.ia.rbx.robson:" &&
    url.hostname === "oauth" &&
    url.pathname === "/callback";
  if (
    (!isHttps && !isRobsonNative) ||
    url.username ||
    url.password ||
    url.search ||
    url.hash
  ) {
    throw new Error("RBX Identity redirect URI is not allowed");
  }
}

function sameCallbackTarget(left: URL, right: URL): boolean {
  return (
    left.protocol === right.protocol &&
    left.hostname === right.hostname &&
    left.port === right.port &&
    left.pathname === right.pathname
  );
}

function sanitizeReturnPath(value: string): string {
  return value.startsWith("/") && !value.startsWith("//")
    ? value
    : "/dashboard";
}

function randomBase64Url(size: number): string {
  const bytes = new Uint8Array(size);
  crypto.getRandomValues(bytes);
  return base64Url(bytes);
}

async function sha256Base64Url(value: string): Promise<string> {
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(value),
  );
  return base64Url(new Uint8Array(digest));
}

function base64Url(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary)
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
}
