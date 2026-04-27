// Typed API client for Robson backend.
// Mapped to actual robsond endpoints (v2 API).

import { browser } from '$app/environment';
import { get as getStore } from 'svelte/store';
import { authToken } from '$stores/auth';
import { env } from '$env/dynamic/public';

const API_BASE: string = env.PUBLIC_ROBSON_API_BASE ?? '';

// --- Backend response types (match robsond serde output) ---

export type Position = {
  id: string;
  account_id: string | null;
  symbol: string;
  side: 'Long' | 'Short' | string;
  state: PositionState;
  entry_price: number | null;
  entry_filled_at: string | null;
  tech_stop_distance: number | null;
  quantity: number | null;
  realized_pnl: number | null;
  pnl?: number | null;
  fees_paid: number | null;
  trailing_stop?: number | null;
  entry_order_id: string | null;
  exit_order_id: string | null;
  insurance_stop_id: string | null;
  binance_position_id: string | null;
  created_at: string | null;
  updated_at: string | null;
  closed_at: string | null;
};

export type PositionState =
  | 'Armed'
  | 'Entering'
  | 'Active'
  | 'Exiting'
  | 'Closed'
  | 'Error'
  | { Entering: { entry_order_id: string; expected_entry: number; signal_id: string } }
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
  | { Closed: { exit_price: number; realized_pnl: number; exit_reason: string } }
  | { Error: { error: string; recoverable: boolean } };

export type StatusResponse = {
  active_positions: number;
  positions: Position[];
  pending_approvals: PendingApproval[];
};

export type PendingApproval = {
  query_id: string;
  position_id: string | null;
  state: string;
  reason: string;
  expires_at: string;
};

export type HaltState = 'active' | 'monthly_halt';

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
    'Content-Type': 'application/json',
    ...(init?.headers as Record<string, string>)
  };
  if (token) {
    headers['Authorization'] = `Bearer ${token}`;
  }

  const res = await fetch(`${API_BASE}${path}`, { ...init, headers });
  if (!res.ok) {
    const body = await res.text().catch(() => '');
    throw new ApiError(path, res.status, res.statusText, body);
  }
  if (res.status === 204) return undefined as T;
  return res.json() as Promise<T>;
}

function toNumber(value: unknown): number | null {
  if (value === null || value === undefined || value === '') return null;
  const n = Number(value);
  return Number.isFinite(n) ? n : null;
}

function normalizePosition(raw: unknown): Position {
  const p = raw as Record<string, unknown>;
  return {
    id: String(p.id ?? ''),
    account_id: p.account_id == null ? null : String(p.account_id),
    symbol: String(p.symbol ?? ''),
    side: String(p.side ?? ''),
    state: (p.state ?? 'Error') as PositionState,
    entry_price: toNumber(p.entry_price),
    entry_filled_at: p.entry_filled_at == null ? null : String(p.entry_filled_at),
    tech_stop_distance: toNumber(p.tech_stop_distance),
    quantity: toNumber(p.quantity),
    realized_pnl: toNumber(p.realized_pnl),
    pnl: toNumber(p.pnl),
    fees_paid: toNumber(p.fees_paid),
    trailing_stop: toNumber(p.trailing_stop),
    entry_order_id: p.entry_order_id == null ? null : String(p.entry_order_id),
    exit_order_id: p.exit_order_id == null ? null : String(p.exit_order_id),
    insurance_stop_id: p.insurance_stop_id == null ? null : String(p.insurance_stop_id),
    binance_position_id: p.binance_position_id == null ? null : String(p.binance_position_id),
    created_at: p.created_at == null ? null : String(p.created_at),
    updated_at: p.updated_at == null ? null : String(p.updated_at),
    closed_at: p.closed_at == null ? null : String(p.closed_at)
  };
}

function normalizeStatus(raw: StatusResponse): StatusResponse {
  return {
    ...raw,
    active_positions: Number(raw.active_positions ?? 0),
    positions: Array.isArray(raw.positions) ? raw.positions.map(normalizePosition) : [],
    pending_approvals: Array.isArray(raw.pending_approvals) ? raw.pending_approvals : []
  };
}

export class ApiError extends Error {
  constructor(
    public readonly path: string,
    public readonly status: number,
    public readonly statusText: string,
    public readonly body: string
  ) {
    super(`API ${path} failed: ${status} ${statusText}`);
    this.name = 'ApiError';
  }
}

// --- Event stream (SSE) ---

export function connectEventStream(
  onEvent: (event: SseEvent) => void,
  onError?: (err: Event) => void
): () => void {
  if (!browser) return () => {};

  const token = getToken();
  const url = new URL(`${API_BASE}/events`, window.location.origin);
  // Bearer token via query param for SSE (EventSource doesn't support headers).
  if (token) url.searchParams.set('token', token);

  const source = createEventSource(url.toString());

  source.onmessage = (msg) => {
    try {
      const data = JSON.parse(msg.data) as SseEvent;
      onEvent(data);
    } catch {
      // ignore malformed events
    }
  };

  if (onError) source.onerror = onError;

  return () => source.close();
}

function createEventSource(url: string): EventSourceLike {
  const factory = (
    window as unknown as {
      __RBX_EVENT_SOURCE_FACTORY__?: (targetUrl: string) => EventSourceLike;
    }
  ).__RBX_EVENT_SOURCE_FACTORY__;

  return factory ? factory(url) : new EventSource(url);
}

// --- API surface ---

export const robsonApi = {
  health: () => apiFetch<{ status: string }>('/health'),

  getStatus: async () => normalizeStatus(await apiFetch<StatusResponse>('/status')),

  getPosition: async (id: string) => normalizePosition(await apiFetch<Position>(`/positions/${id}`)),

  armPosition: (body: { symbol: string; side: string }) =>
    apiFetch<Position>('/positions', { method: 'POST', body: JSON.stringify(body) }),

  injectSignal: (id: string, body: { entry_price: number; stop_loss: number }) =>
    apiFetch<void>(`/positions/${id}/signal`, { method: 'POST', body: JSON.stringify(body) }),

  closePosition: (id: string) =>
    apiFetch<void>(`/positions/${id}`, { method: 'DELETE' }),

  approveQuery: (id: string) =>
    apiFetch<void>(`/queries/${id}/approve`, { method: 'POST' }),

  getHaltStatus: () => apiFetch<MonthlyHaltStatus>('/monthly-halt'),

  triggerHalt: (reason: string) =>
    apiFetch<MonthlyHaltStatus>('/monthly-halt', {
      method: 'POST',
      body: JSON.stringify({ reason })
    }),

  panic: () =>
    apiFetch<PanicResponse>('/panic', { method: 'POST' }),

  getSafetyStatus: () => apiFetch<SafetyStatusResponse>('/safety/status')
};
