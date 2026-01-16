// Domain types matching Rust structs
//
// Key changes from v1:
// - Removed stop_gain (no fixed profit target)
// - Added trailing_stop (1x technical stop distance technique)
// - Removed Trade (fill info consolidated in Order)
// - Isolated margin trading (not spot)

export interface Position {
  id: string;
  symbol: string;
  side: 'long' | 'short';
  state: PositionState;
  entry_price?: number;
  quantity: number;
  leverage: number;

  // Technical stop distance (trailing stop anchor)
  tech_stop_distance?: {
    distance: number;           // Absolute distance in quote currency
    distance_pct: number;       // Percentage of entry price
    entry_price: number;
    initial_stop: number;      // Initial technical stop from chart analysis
  };

  // Trailing stop (only populated in Active state)
  trailing_stop?: number;       // Current trailing stop price
  favorable_extreme?: number;   // Peak (long) or lowest (short) price seen
  extreme_at?: string;          // When extreme was reached

  // P&L
  realized_pnl?: number;
  fees_paid?: number;

  // Timestamps
  created_at: string;
  entry_filled_at?: string;
  closed_at?: string;
}

export type PositionState =
  | 'armed'      // Waiting for detector signal
  | 'entering'   // Entry order submitted
  | 'active'     // Position active, monitoring trailing stop
  | 'exiting'    // Exit order submitted
  | 'closed'     // Position closed with PnL realized
  | 'error';     // Error state, manual intervention required

export type ExitReason =
  | 'trailing_stop'      // Normal exit via trailing stop
  | 'insurance_stop'     // Daemon down, exchange stop triggered
  | 'user_panic'         // User manually triggered panic
  | 'degraded_mode'      // Emergency exit
  | 'position_error';    // Position error (margin call, etc.)

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

// Active position details (with trailing stop info)
export interface ActivePosition extends Position {
  state: 'active';
  trailing_stop: number;
  favorable_extreme: number;
  extreme_at: string;
  current_price: number;
  unrealized_pnl: number;
}

// Closed position details
export interface ClosedPosition extends Position {
  state: 'closed';
  exit_price: number;
  realized_pnl: number;
  exit_reason: ExitReason;
}

// Request/Response types
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
  state: PositionState;
}

export interface DisarmRequest {
  position_id: string;
  force?: boolean;
}

export interface PanicRequest {
  symbol?: string;  // If omitted, panic all positions
  confirm?: boolean;
  dry_run?: boolean;
}

export interface ErrorResponse {
  error: {
    code: string;
    message: string;
    details?: Record<string, unknown>;
  };
}
