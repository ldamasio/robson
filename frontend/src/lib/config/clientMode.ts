export type ClientMode = "operator" | "mobile-readonly";

export class MobileReadOnlyError extends Error {
  constructor(
    public readonly method: string,
    public readonly path: string,
  ) {
    super(`Mobile read-only mode blocks ${method} ${path}`);
    this.name = "MobileReadOnlyError";
  }
}

export function resolveClientMode(value: string | undefined): ClientMode {
  return value === "mobile-readonly" ? "mobile-readonly" : "operator";
}

export function methodAllowed(mode: ClientMode, method = "GET"): boolean {
  const normalized = method.toUpperCase();
  return mode === "operator" || normalized === "GET" || normalized === "HEAD";
}

export function assertMethodAllowed(
  mode: ClientMode,
  method: string,
  path: string,
): void {
  if (!methodAllowed(mode, method)) {
    throw new MobileReadOnlyError(method.toUpperCase(), path);
  }
}
