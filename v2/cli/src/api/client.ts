import axios, { AxiosInstance } from 'axios';
import type {
  StatusResponse,
  ArmRequest,
  ArmResponse,
  PanicResponse,
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
}
