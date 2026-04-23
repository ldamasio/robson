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
  account_id: string;
  symbol: string;
  side: 'Long' | 'Short';
  state: PositionState;
  entry_price: number | null;
  entry_filled_at: string | null;
  tech_stop_distance: number | null;
  quantity: number;
  realized_pnl: number;
  fees_paid: number;
  entry_order_id: string | null;
  exit_order_id: string | null;
  insurance_stop_id: string | null;
  binance_position_id: string | null;
  created_at: string;
  updated_at: string;
  closed_at: string | null;
};

export type PositionState =
  | 'Armed'
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

  const source = new EventSource(url.toString());

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

// --- API surface ---

export const robsonApi = {
  health: () => apiFetch<{ status: string }>('/health'),

  getStatus: () => apiFetch<StatusResponse>('/status'),

  getPosition: (id: string) => apiFetch<Position>(`/positions/${id}`),

  armPosition: (body: { symbol: string; side: string }) =>
    apiFetch<Position>('/positions', { method: 'POST', body: JSON.stringify(body) }),

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
