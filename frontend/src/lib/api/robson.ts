// Typed API client for Robson backend.
// Mapped to actual robsond endpoints (v2 API).

import { browser } from "$app/environment";
import { get as getStore } from "svelte/store";
import { authToken } from "$stores/auth";
import { env } from "$env/dynamic/public";

const API_BASE: string = env.PUBLIC_ROBSON_API_BASE ?? "";

// --- Backend response types (match robsond serde output) ---

export type Position = {
  id: string;
  account_id: string | null;
  symbol: string;
  side: "Long" | "Short" | string;
  state: PositionState;
  exchange_sync_state?: string | null;
  entry_mode?: string | null;
  approval_mode?: string | null;
  entry_price: number | null;
  entry_filled_at: string | null;
  tech_stop_distance: number | null;
  quantity: number | null;
  realized_pnl: number | null;
  pnl?: number | null;
  variation_pct?: number | null;
  fees_paid: number | null;
  trailing_stop?: number | null;
  effective_stop?: number | null;
  raw_technical_stop?: number | null;
  invalidation_guard_level?: number | null;
  effective_stop_basis?: string | null;
  current_price?: number | null;
  entry_order_id: string | null;
  exit_order_id: string | null;
  insurance_stop_id: string | null;
  binance_position_id: string | null;
  created_at: string | null;
  updated_at: string | null;
  closed_at: string | null;
};

export type PositionState =
  | "Armed"
  | "Entering"
  | "Active"
  | "Exiting"
  | "Closed"
  | "Error"
  | "Cancelled"
  | "Canceled"
  | {
      Entering: {
        entry_order_id: string;
        expected_entry: number;
        signal_id: string;
      };
    }
  | {
      Active: {
        current_price: number;
        trailing_stop: number;
        favorable_extreme: number;
        extreme_at: string;
        insurance_stop_id: string | null;
        last_emitted_stop: number | null;
      };
    }
  | { Exiting: { exit_order_id: string; exit_reason: string } }
  | {
      Closed: { exit_price: number; realized_pnl: number; exit_reason: string };
    }
  | { Error: { error: string; recoverable: boolean } };

export type ReconciliationBlocker = {
  position_id: string;
  symbol: string;
  side: string;
  reason: string;
};

export type StatusResponse = {
  active_positions: number;
  positions: Position[];
  pending_approvals: PendingApproval[];
  stale_active_count: number;
  reconciliation_blockers: ReconciliationBlocker[];
  new_slots_available: number;
  occupied_slots: number;
  slot_cells_total: number;
  monthly_realized_loss: number;
  monthly_realized_loss_pct: number;
  capital_base: number;
  wallet_balance: number;
};

export type ArmEntryPolicy = {
  mode?: string;
  approval?: string;
};

export type ArmPositionRequest = {
  symbol: string;
  side: string;
  entry_policy?: ArmEntryPolicy;
};

export type MonthlyPositionsResponse = {
  month: string;
  positions: Position[];
};

export type PendingApproval = {
  query_id: string;
  position_id: string | null;
  state: string;
  reason: string;
  expires_at: string;
};

export type HaltState = "active" | "monthly_halt";

export type MonthlyHaltStatus = {
  state: HaltState;
  description: string;
  reason: string | null;
  triggered_at: string | null;
  blocks_new_entries: boolean;
  blocks_signals: boolean;
};

export type PanicResponse = {
  closed_positions: string[];
  count: number;
};

export type SafetyStatusResponse = {
  enabled: boolean;
  symbols: string[];
  poll_interval_secs: number;
  tracked_positions: DetectedPosition[];
  pending_executions: number;
};

export type DetectedPosition = {
  id: string;
  symbol: string;
  side: string;
  entry_price: number;
  quantity: number;
  stop_price: number;
  stop_distance_pct: number;
  detected_at: string;
};

export type SseEvent = {
  event_id: string;
  event_type: string;
  occurred_at: string;
  payload: Record<string, unknown>;
};

export type FundingState =
  | "QUOTED"
  | "CONVERTING"
  | "CONVERTED"
  | "TRANSFERRING"
  | "SETTLED"
  | "REFRESHED"
  | "FAILED";

// Monetary fields are rust_decimal::Decimal, serialized by the backend as JSON
// strings (e.g. "198.79328929...") to preserve precision. Keep them as strings
// and format for display only — never compute money in the browser.
export type FundingItem = {
  asset: string;
  qty: string;
  est_usdt: string;
};

export type FundingQuote = {
  quote_id: string;
  items: FundingItem[];
  estimated_usdt: string;
  fees: string;
  slippage_bps: number;
  expires_at: string;
};

export type FundingEvent = {
  type: string;
  at: string;
  [key: string]: unknown;
};

export type FundingSaga = {
  saga_id: string;
  state: FundingState;
  items: FundingItem[];
  events: FundingEvent[];
  updated_at: string;
};

export type FundingSagaSummary = {
  saga_id: string;
  state: FundingState;
  estimated_usdt: string;
  created_at: string;
};

export type FundingRecoverSpotUsdtRequest = {
  asset?: "USDT";
  amount?: string;
  dry_run?: boolean;
  execute?: boolean;
  confirm?: string;
  correlation_id?: string;
};

export type FundingRecoverSpotUsdtResponse = {
  correlation_id: string;
  asset: string;
  amount: string;
  from: string;
  to: string;
  transfer_type: string;
  spot_usdt_before: string;
  futures_usdt_wallet_before: string;
  futures_usdt_available_before: string;
  spot_usdt_after_expected: string;
  futures_usdt_wallet_after_expected: string;
  spot_usdt_after_actual?: string | null;
  futures_usdt_wallet_after_actual?: string | null;
  futures_usdt_available_after_actual?: string | null;
  transfer_id?: string | null;
  dry_run: boolean;
  idempotent_skip: boolean;
};

type EventSourceLike = {
  onmessage: ((this: EventSource, ev: MessageEvent) => unknown) | null;
  onerror: ((this: EventSource, ev: Event) => unknown) | null;
  close: () => void;
};

// --- Helpers ---

function getToken(): string | null {
  if (!browser) return null;
  return getStore(authToken);
}

async function apiFetch<T>(path: string, init?: RequestInit): Promise<T> {
  const token = getToken();
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    ...(init?.headers as Record<string, string>),
  };
  if (token) {
    headers["Authorization"] = `Bearer ${token}`;
  }

  const res = await fetch(`${API_BASE}${path}`, { ...init, headers });
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw new ApiError(path, res.status, res.statusText, body);
  }
  if (res.status === 204) return undefined as T;
  return res.json() as Promise<T>;
}

function toNumber(value: unknown): number | null {
  if (value === null || value === undefined || value === "") return null;
  const n = Number(value);
  return Number.isFinite(n) ? n : null;
}

function requireNumber(value: unknown, field: string): number {
  const n = Number(value);
  if (!Number.isFinite(n)) {
    throw new Error(`Invalid /status response: missing numeric ${field}`);
  }
  return n;
}

function normalizePosition(raw: unknown): Position {
  const p = raw as Record<string, unknown>;
  return {
    id: String(p.id ?? ""),
    account_id: p.account_id == null ? null : String(p.account_id),
    symbol: String(p.symbol ?? ""),
    side: String(p.side ?? ""),
    state: (p.state ?? "Error") as PositionState,
    exchange_sync_state:
      p.exchange_sync_state == null ? null : String(p.exchange_sync_state),
    entry_mode: p.entry_mode == null ? null : String(p.entry_mode),
    approval_mode: p.approval_mode == null ? null : String(p.approval_mode),
    entry_price: toNumber(p.entry_price),
    entry_filled_at:
      p.entry_filled_at == null ? null : String(p.entry_filled_at),
    tech_stop_distance: toNumber(p.tech_stop_distance),
    quantity: toNumber(p.quantity),
    realized_pnl: toNumber(p.realized_pnl),
    pnl: toNumber(p.pnl),
    variation_pct: toNumber(p.variation_pct),
    fees_paid: toNumber(p.fees_paid),
    trailing_stop: toNumber(p.trailing_stop),
    effective_stop: toNumber(p.effective_stop),
    raw_technical_stop: toNumber(p.raw_technical_stop),
    invalidation_guard_level: toNumber(p.invalidation_guard_level),
    effective_stop_basis:
      p.effective_stop_basis == null ? null : String(p.effective_stop_basis),
    current_price: toNumber(p.current_price),
    entry_order_id: p.entry_order_id == null ? null : String(p.entry_order_id),
    exit_order_id: p.exit_order_id == null ? null : String(p.exit_order_id),
    insurance_stop_id:
      p.insurance_stop_id == null ? null : String(p.insurance_stop_id),
    binance_position_id:
      p.binance_position_id == null ? null : String(p.binance_position_id),
    created_at: p.created_at == null ? null : String(p.created_at),
    updated_at: p.updated_at == null ? null : String(p.updated_at),
    closed_at: p.closed_at == null ? null : String(p.closed_at),
  };
}

function normalizeStatus(raw: StatusResponse): StatusResponse {
  return {
    ...raw,
    active_positions: Number(raw.active_positions ?? 0),
    positions: Array.isArray(raw.positions)
      ? raw.positions.map(normalizePosition)
      : [],
    pending_approvals: Array.isArray(raw.pending_approvals)
      ? raw.pending_approvals
      : [],
    stale_active_count: Number(raw.stale_active_count ?? 0),
    reconciliation_blockers: Array.isArray(raw.reconciliation_blockers)
      ? raw.reconciliation_blockers.map((blocker) => ({
          position_id: String(blocker.position_id),
          symbol: String(blocker.symbol),
          side: String(blocker.side),
          reason: String(blocker.reason),
        }))
      : [],
    new_slots_available: requireNumber(
      raw.new_slots_available,
      "new_slots_available",
    ),
    occupied_slots: requireNumber(raw.occupied_slots, "occupied_slots"),
    slot_cells_total: requireNumber(raw.slot_cells_total, "slot_cells_total"),
    monthly_realized_loss: requireNumber(
      raw.monthly_realized_loss,
      "monthly_realized_loss",
    ),
    monthly_realized_loss_pct: requireNumber(
      raw.monthly_realized_loss_pct,
      "monthly_realized_loss_pct",
    ),
    capital_base: requireNumber(raw.capital_base, "capital_base"),
    wallet_balance: requireNumber(raw.wallet_balance, "wallet_balance"),
  };
}

export class ApiError extends Error {
  constructor(
    public readonly path: string,
    public readonly status: number,
    public readonly statusText: string,
    public readonly body: string,
  ) {
    super(`API ${path} failed: ${status} ${statusText}`);
    this.name = "ApiError";
  }
}

// --- Event stream (SSE) ---

export function connectEventStream(
  onEvent: (event: SseEvent) => void,
  onError?: (err: Event) => void,
  onReconnect?: () => void,
): () => void {
  if (!browser) return () => {};

  const token = getToken();
  const url = `${API_BASE}/events`;

  // Test mock factory takes precedence (no real fetch in tests)
  const factory = (
    window as unknown as {
      __RBX_EVENT_SOURCE_FACTORY__?: (targetUrl: string) => EventSourceLike;
    }
  ).__RBX_EVENT_SOURCE_FACTORY__;

  const source = factory ? factory(url) : new FetchEventSource(url, token, onReconnect);

  source.onmessage = (msg) => {
    try {
      const data = JSON.parse(msg.data) as SseEvent;
      onEvent(data);
    } catch {
      // ignore malformed events (heartbeats, etc.)
    }
  };

  if (onError) source.onerror = onError;

  return () => source.close();
}

/** Fetch-based SSE client — sends Bearer token via header, not query param.
 *  Reconnects automatically with exponential backoff on any disconnect. */
export class FetchEventSource implements EventSourceLike {
  onmessage: ((this: EventSource, ev: MessageEvent) => unknown) | null = null;
  onerror: ((this: EventSource, ev: Event) => unknown) | null = null;

  private controller = new AbortController();
  private retries = 0;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private static readonly MAX_RECONNECT_MS = 30_000;

  constructor(
    url: string,
    token: string | null,
    private readonly onReconnect?: () => void,
  ) {
    this.connect(url, token);
  }

  private scheduleReconnect(url: string, token: string | null): void {
    if (this.controller.signal.aborted) return;
    const delay = Math.min(1_000 * 2 ** this.retries, FetchEventSource.MAX_RECONNECT_MS);
    this.retries++;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      if (!this.controller.signal.aborted) this.connect(url, token);
    }, delay);
  }

  private async connect(url: string, token: string | null): Promise<void> {
    const headers: Record<string, string> = { Accept: `text/event-stream` };
    if (token) headers[`Authorization`] = `Bearer ${token}`;

    try {
      const res = await fetch(url, {
        headers,
        signal: this.controller.signal,
      });
      if (!res.ok || !res.body) {
        this.onerror?.call({} as EventSource, new Event(`error`));
        this.scheduleReconnect(url, token);
        return;
      }

      const reader = res.body.getReader();
      if (this.retries > 0) this.onReconnect?.();
      this.retries = 0; // reset backoff on successful stream start
      const decoder = new TextDecoder();
      let buf = ``;

      while (true) {
        const { done, value } = await reader.read();
        if (done) {
          this.scheduleReconnect(url, token);
          break;
        }

        buf += decoder.decode(value, { stream: true });

        // SSE events are separated by blank lines (\n\n)
        let boundary: number;
        while ((boundary = buf.indexOf(`\n\n`)) !== -1) {
          const raw = buf.slice(0, boundary);
          buf = buf.slice(boundary + 2);
          this.dispatchSseEvent(raw);
        }
      }
    } catch (err) {
      if ((err as DOMException)?.name === "AbortError") return;
      this.onerror?.call({} as EventSource, new Event(`error`));
      this.scheduleReconnect(url, token);
    }
  }

  /** Parse a single SSE text block and emit onmessage for data lines. */
  private dispatchSseEvent(text: string): void {
    let data = ``;
    for (const line of text.split(`
`)) {
      if (line.startsWith(`：`)) continue; // comment / heartbeat
      if (line.startsWith(`data:`)) {
        data += line.slice(5);
      }
    }
    if (!data) return;
    this.onmessage?.call(
      {} as EventSource,
      new MessageEvent(`message`, { data }),
    );
  }

  close(): void {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.controller.abort();
  }
}

// --- API surface ---

export const robsonApi = {
  health: () => apiFetch<{ status: string }>("/health"),

  getStatus: async () =>
    normalizeStatus(await apiFetch<StatusResponse>("/status")),

  getMonthlyPositions: async (month?: string) => {
    const query = month ? `?month=${encodeURIComponent(month)}` : "";
    const response = await apiFetch<MonthlyPositionsResponse>(
      `/positions${query}`,
    );
    return {
      ...response,
      positions: Array.isArray(response.positions)
        ? response.positions.map(normalizePosition)
        : [],
    };
  },

  getPosition: async (id: string) =>
    normalizePosition(await apiFetch<Position>(`/positions/${id}`)),

  armPosition: (body: ArmPositionRequest) =>
    apiFetch<Position>("/positions", {
      method: "POST",
      body: JSON.stringify(body),
    }),

  injectSignal: (
    id: string,
    body: { entry_price: number; stop_loss: number },
  ) =>
    apiFetch<void>(`/positions/${id}/signal`, {
      method: "POST",
      body: JSON.stringify(body),
    }),

  closePosition: (id: string) =>
    apiFetch<void>(`/positions/${id}`, { method: "DELETE" }),

  approveQuery: (id: string) =>
    apiFetch<void>(`/queries/${id}/approve`, { method: "POST" }),

  getHaltStatus: () => apiFetch<MonthlyHaltStatus>("/monthly-halt"),

  triggerHalt: (reason: string) =>
    apiFetch<MonthlyHaltStatus>("/monthly-halt", {
      method: "POST",
      body: JSON.stringify({ reason }),
    }),

  panic: () => apiFetch<PanicResponse>("/panic", { method: "POST" }),

  getSafetyStatus: () => apiFetch<SafetyStatusResponse>("/safety/status"),

  getFundingQuote: () =>
    apiFetch<FundingQuote>("/funding/quote", { method: "POST" }),

  executeFunding: (quoteId: string, idempotencyKey: string) =>
    apiFetch<{ saga_id: string; state: FundingState }>("/funding/execute", {
      method: "POST",
      headers: { "Idempotency-Key": idempotencyKey },
      body: JSON.stringify({ quote_id: quoteId }),
    }),

  getFundingSaga: (id: string) => apiFetch<FundingSaga>(`/funding/${id}`),

  listFunding: () => apiFetch<FundingSagaSummary[]>("/funding"),

  recoverSpotUsdtToFutures: (body: FundingRecoverSpotUsdtRequest) =>
    apiFetch<FundingRecoverSpotUsdtResponse>(
      "/funding/recover-spot-usdt-to-futures",
      {
        method: "POST",
        body: JSON.stringify(body),
      },
    ),
};
