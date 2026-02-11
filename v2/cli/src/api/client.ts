import axios, { AxiosInstance } from 'axios';
import type {
  StatusResponse,
  ArmRequest,
  ArmResponse,
  PanicResponse,
  SafetyStatusResponse,
  SafetyTestResponse,
  SetCredentialsRequest,
  ListCredentialsRequest,
  RevokeCredentialsRequest,
  CredentialMetadata,
} from '../types';

/**
 * HTTP client for robsond daemon API.
 *
 * Endpoints:
 * - GET  /health           → Health check
 * - GET  /status           → List positions
 * - POST /positions        → Arm new position
 * - GET  /positions/:id    → Get position
 * - DELETE /positions/:id  → Disarm position
 * - POST /positions/:id/signal → Inject signal (testing)
 * - POST /panic            → Emergency close all
 * - POST /credentials      → Store credentials
 * - GET  /credentials      → List credentials
 * - DELETE /credentials    → Revoke credentials
 */
export class RobsonClient {
  private client: AxiosInstance;

  constructor(baseURL: string = 'http://localhost:8080') {
    this.client = axios.create({
      baseURL,
      timeout: 10000,
      headers: {
        'Content-Type': 'application/json',
      },
    });
  }

  /**
   * Health check - verify daemon is running.
   */
  async health(): Promise<{ status: string; version: string }> {
    const response = await this.client.get('/health');
    return response.data;
  }

  /**
   * Get status of all positions.
   */
  async status(): Promise<StatusResponse> {
    const response = await this.client.get<StatusResponse>('/status');
    return response.data;
  }

  /**
   * Arm a new position.
   *
   * Creates position in Armed state and spawns detector task.
   */
  async arm(request: ArmRequest): Promise<ArmResponse> {
    const response = await this.client.post<ArmResponse>('/positions', {
      symbol: request.symbol,
      side: request.side,
      capital: request.capital,
      risk_percent: request.risk_percent,
    });
    return response.data;
  }

  /**
   * Disarm (cancel) an armed position.
   *
   * Only works for positions in Armed state.
   */
  async disarm(positionId: string): Promise<void> {
    await this.client.delete(`/positions/${positionId}`);
  }

  /**
   * Inject a detector signal (for testing).
   *
   * Triggers entry flow: Armed → Entering → Active.
   */
  async injectSignal(
    positionId: string,
    entryPrice: number,
    stopLoss: number
  ): Promise<void> {
    await this.client.post(`/positions/${positionId}/signal`, {
      position_id: positionId,
      entry_price: entryPrice,
      stop_loss: stopLoss,
    });
  }

  /**
   * Emergency close all positions.
   */
  async panic(): Promise<PanicResponse> {
    const response = await this.client.post<PanicResponse>('/panic');
    return response.data;
  }

  // ==========================================================================
  // Safety Net Methods
  // ==========================================================================

  /**
   * Get safety net status.
   *
   * Shows detected rogue positions and pending executions.
   * This is an observability endpoint - no credentials required.
   */
  async safetyStatus(): Promise<SafetyStatusResponse> {
    const response = await this.client.get<SafetyStatusResponse>('/safety/status');
    return response.data;
  }

  /**
   * Test safety net connection.
   *
   * Tests Binance API connection and shows what positions would be monitored.
   *
   * @param scope - Identity scope (tenant_id, user_id, profile)
   */
  async safetyTest(scope?: { tenant_id: string; user_id: string; profile: string }): Promise<SafetyTestResponse> {
    const params: Record<string, string> = {};
    if (scope) {
      params.tenant_id = scope.tenant_id;
      params.user_id = scope.user_id;
      params.profile = scope.profile;
    }

    const response = await this.client.get<SafetyTestResponse>('/safety/test', { params });
    return response.data;
  }

  // ==========================================================================
  // Credentials Methods
  // ==========================================================================

  /**
   * Store encrypted credentials.
   *
   * Credentials are encrypted server-side with AES-256-GCM.
   * The API never returns secrets in responses.
   */
  async setCredentials(request: SetCredentialsRequest): Promise<void> {
    await this.client.post('/credentials', {
      tenant_id: request.tenant_id,
      user_id: request.user_id,
      profile: request.profile,
      exchange: request.exchange,
      api_key: request.api_key,
      api_secret: request.api_secret,
      label: request.label,
    });
  }

  /**
   * List stored credentials (metadata only, no secrets).
   */
  async listCredentials(request: ListCredentialsRequest): Promise<CredentialMetadata[]> {
    const params: Record<string, string> = {};
    if (request.tenant_id) params.tenant_id = request.tenant_id;
    if (request.user_id) params.user_id = request.user_id;

    const response = await this.client.get<CredentialMetadata[]>('/credentials', { params });
    return response.data;
  }

  /**
   * Revoke credentials.
   */
  async revokeCredentials(request: RevokeCredentialsRequest): Promise<void> {
    await this.client.delete('/credentials', {
      data: {
        tenant_id: request.tenant_id,
        user_id: request.user_id,
        profile: request.profile,
        exchange: request.exchange,
        reason: request.reason,
      },
    });
  }
}
