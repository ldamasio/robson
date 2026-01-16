import axios, { AxiosInstance } from 'axios';
import type {
  StatusResponse,
  ArmRequest,
  ArmResponse,
  ErrorResponse,
} from '../types';

export class RobsonClient {
  private client: AxiosInstance;

  constructor(baseURL: string = 'http://localhost:8080') {
    this.client = axios.create({
      baseURL,
      timeout: 5000,
      headers: {
        'Content-Type': 'application/json',
      },
    });
  }

  async status(): Promise<StatusResponse> {
    try {
      const response = await this.client.get<StatusResponse>('/status');
      return response.data;
    } catch (error) {
      this.handleError(error);
      throw error;
    }
  }

  async arm(request: ArmRequest): Promise<ArmResponse> {
    try {
      const response = await this.client.post<ArmResponse>('/arm', request);
      return response.data;
    } catch (error) {
      this.handleError(error);
      throw error;
    }
  }

  async disarm(positionId: string, force: boolean = false): Promise<void> {
    try {
      await this.client.post('/disarm', { position_id: positionId, force });
    } catch (error) {
      this.handleError(error);
      throw error;
    }
  }

  async panic(options: { symbol?: string }): Promise<void> {
    try {
      await this.client.post('/panic', options);
    } catch (error) {
      this.handleError(error);
      throw error;
    }
  }

  private handleError(error: unknown): void {
    if (axios.isAxiosError(error)) {
      if (error.response) {
        const errorResponse = error.response.data as ErrorResponse;
        console.error(`API Error: ${errorResponse.error.message}`);
      } else if (error.request) {
        console.error('Cannot connect to daemon. Is robsond running?');
      } else {
        console.error(`Request error: ${error.message}`);
      }
    } else {
      console.error(`Unexpected error: ${error}`);
    }
  }
}
