// Domain types matching Rust structs

export interface Position {
  id: string;
  symbol: string;
  side: 'long' | 'short';
  state: 'armed' | 'entering' | 'active' | 'exiting' | 'closed' | 'error';
  entry_price?: number;
  stop_loss: number;
  stop_gain: number;
  quantity: number;
  leverage: number;
  unrealized_pnl?: number;
  realized_pnl?: number;
  palma?: {
    distance: number;
    distance_pct: number;
  };
  created_at: string;
  entry_filled_at?: string;
  closed_at?: string;
}

export interface Summary {
  active_count: number;
  armed_count: number;
  closed_today_count: number;
  total_pnl_today: number;
}

export interface StatusResponse {
  positions: Position[];
  summary: Summary;
}

export interface ArmRequest {
  symbol: string;
  strategy: string;
  capital?: number;
  leverage?: number;
  dry_run?: boolean;
}

export interface ArmResponse {
  position_id: string;
  symbol: string;
  state: string;
}

export interface ErrorResponse {
  error: {
    code: string;
    message: string;
    details?: Record<string, unknown>;
  };
}
