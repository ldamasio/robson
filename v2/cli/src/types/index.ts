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

// Note: StatusResponse is defined below in API types section

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

// Request/Response types matching robsond API

/**
 * Request to arm a new position.
 *
 * Note: leverage is fixed at 10x (not configurable).
 * Position size is calculated via Golden Rule: (capital × risk%) / tech_stop_distance
 */
export interface ArmRequest {
  symbol: string;
  side: 'long' | 'short';
  capital: number;        // Capital to allocate (e.g., 1000 USDT)
  risk_percent: number;   // Risk per trade (e.g., 1 = 1%)
}

/**
 * Response after arming a position.
 */
export interface ArmResponse {
  position_id: string;
  symbol: string;
  side: string;
  state: string;
}

/**
 * Response from status endpoint.
 */
export interface StatusResponse {
  active_positions: number;
  positions: PositionSummary[];
}

/**
 * Summary of a position (from daemon API).
 */
export interface PositionSummary {
  id: string;
  symbol: string;
  side: string;
  state: string;
  entry_price?: number;
  trailing_stop?: number;
  pnl?: number;
}

/**
 * Response from panic endpoint.
 */
export interface PanicResponse {
  closed_positions: string[];
  count: number;
}

/**
 * Error response from daemon API.
 */
export interface ErrorResponse {
  error: string;
}

// =============================================================================
// Safety Net Types
// =============================================================================

/**
 * Safety net status response.
 */
export interface SafetyStatusResponse {
  enabled: boolean;
  symbols: string[];
  poll_interval_secs: number;
  tracked_positions: DetectedPositionSummary[];
  pending_executions: number;
}

/**
 * Summary of a detected rogue position.
 */
export interface DetectedPositionSummary {
  id: string;
  symbol: string;
  side: string;
  entry_price: number;
  quantity: number;
  stop_price: number;
  stop_distance_pct: number;
  detected_at: string;
}

/**
 * Safety net test response.
 */
export interface SafetyTestResponse {
  success: boolean;
  message: string;
  positions?: BinancePositionInfo[];
}

/**
 * Info about a Binance position (for testing).
 */
export interface BinancePositionInfo {
  symbol: string;
  side: string;
  quantity: number;
  entry_price: number;
  calculated_stop: number;
}

// =============================================================================
// Credentials Types
// =============================================================================

/**
 * Request to store credentials.
 */
export interface SetCredentialsRequest {
  tenant_id: string;
  user_id: string;
  profile: string;
  exchange: string;
  api_key: string;
  api_secret: string;
  label?: string;
}

/**
 * Request to list credentials.
 */
export interface ListCredentialsRequest {
  tenant_id?: string;
  user_id?: string;
}

/**
 * Request to revoke credentials.
 */
export interface RevokeCredentialsRequest {
  tenant_id: string;
  user_id: string;
  profile: string;
  exchange: string;
  reason: string;
}

/**
 * Credential metadata (no secrets).
 */
export interface CredentialMetadata {
  tenant_id: string;
  user_id: string;
  profile: string;
  exchange: string;
  status: 'active' | 'revoked' | 'expired';
  label?: string;
  created_at: string;
  last_used_at?: string;
}
