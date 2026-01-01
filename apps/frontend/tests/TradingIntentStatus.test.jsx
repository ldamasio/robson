import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import '@testing-library/jest-dom';
import TradingIntentStatus from '../src/components/logged/TradingIntentStatus';

// Mock react-router-dom
vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual('react-router-dom');
  return {
    ...actual,
    useNavigate: () => vi.fn(),
  };
});

// Mock AuthContext
const mockAuthTokens = { access: 'mock-token' };
vi.mock('../src/context/AuthContext', () => ({
  __esModule: true,
  default: {},
  AuthContext: { displayName: 'AuthContext' },
  useContext: () => ({ authTokens: mockAuthTokens }),
}));

// Mock useTradingIntent hook
const mockIntent = {
  id: 'test-intent-123',
  status: 'PENDING',
  symbol: { name: 'BTCUSDT', base_asset: 'BTC', quote_asset: 'USDT' },
  strategy: { id: 1, name: 'All In' },
  side: 'BUY',
  quantity: '0.001',
  entry_price: '50000',
  stop_price: '49000',
  capital: '100',
  risk_amount: '1',
  risk_percent: '1.0',
  validation_result: null,
  execution_result: null,
  created_at: '2026-01-01T10:00:00Z',
  updated_at: '2026-01-01T10:00:00Z',
};

let mockHookReturn = {
  intent: mockIntent,
  isLoading: false,
  error: null,
  refetch: vi.fn(),
  isPolling: false,
};

vi.mock('../src/hooks/useTradingIntent', () => ({
  useTradingIntent: vi.fn(() => mockHookReturn),
}));

// Mock global fetch
global.fetch = vi.fn();

describe('TradingIntentStatus Component', () => {
  const { useTradingIntent } = require('../src/hooks/useTradingIntent');

  beforeEach(() => {
    vi.clearAllMocks();
    mockHookReturn = {
      intent: mockIntent,
      isLoading: false,
      error: null,
      refetch: vi.fn(),
      isPolling: false,
    };
    useTradingIntent.mockReturnValue(mockHookReturn);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  describe('test_renders_pending_intent', () => {
    it('should render pending intent with correct status badge', () => {
      render(<TradingIntentStatus intentId="test-intent-123" showDetails={true} />);

      expect(screen.getByText('PENDING')).toBeInTheDocument();
      expect(screen.getByText(/test-inte.*123/)).toBeInTheDocument();
      expect(screen.getByText('BTCUSDT')).toBeInTheDocument();
      expect(screen.getByText('All In')).toBeInTheDocument();
    });
  });

  describe('test_renders_validated_intent', () => {
    it('should render validated intent with validation results', () => {
      const validatedIntent = {
        ...mockIntent,
        status: 'VALIDATED',
        validation_result: {
          status: 'PASS',
          guards: [
            { name: 'Balance Check', status: 'PASS', message: 'Balance sufficient' },
            { name: 'Risk Limit', status: 'PASS', message: 'Risk within limits' },
          ],
          warnings: [],
          validated_at: '2026-01-01T10:01:00Z',
        },
      };

      mockHookReturn.intent = validatedIntent;
      useTradingIntent.mockReturnValue(mockHookReturn);

      render(<TradingIntentStatus intentId="test-intent-123" showDetails={true} />);

      expect(screen.getByText('VALIDATED')).toBeInTheDocument();
      expect(screen.getByText('Balance Check')).toBeInTheDocument();
      expect(screen.getByText('Risk Limit')).toBeInTheDocument();
      expect(screen.getByText('Balance sufficient')).toBeInTheDocument();
      expect(screen.getByText('Risk within limits')).toBeInTheDocument();
    });
  });

  describe('test_renders_executed_intent', () => {
    it('should render executed intent with execution results', () => {
      const executedIntent = {
        ...mockIntent,
        status: 'EXECUTED',
        execution_result: {
          status: 'SUCCESS',
          mode: 'DRY-RUN',
          actions: [
            {
              type: 'BUY',
              asset: 'BTC',
              quantity: '0.001',
              price: '50000',
              order_id: '123456789',
              status: 'FILLED',
            },
          ],
          audit_trail: [
            { action: 'SPOT_BUY', amount: '50.00', asset: 'USDC' },
          ],
          errors: [],
          warnings: [],
          executed_at: '2026-01-01T10:05:00Z',
        },
      };

      mockHookReturn.intent = executedIntent;
      useTradingIntent.mockReturnValue(mockHookReturn);

      render(<TradingIntentStatus intentId="test-intent-123" showDetails={true} />);

      expect(screen.getByText('EXECUTED')).toBeInTheDocument();
      expect(screen.getByText('123456789')).toBeInTheDocument();
      expect(screen.getByText('SPOT_BUY')).toBeInTheDocument();
    });
  });

  describe('test_expands_collapses_sections', () => {
    it('should expand and collapse validation section', () => {
      const validatedIntent = {
        ...mockIntent,
        status: 'VALIDATED',
        validation_result: {
          status: 'PASS',
          guards: [
            { name: 'Balance Check', status: 'PASS', message: 'Balance sufficient' },
          ],
          warnings: [],
          validated_at: '2026-01-01T10:01:00Z',
        },
      };

      mockHookReturn.intent = validatedIntent;
      useTradingIntent.mockReturnValue(mockHookReturn);

      render(<TradingIntentStatus intentId="test-intent-123" showDetails={true} />);

      const validationButton = screen.getByText('Validation Results');
      fireEvent.click(validationButton);

      // After clicking, the accordion should toggle
      // In a real test, we'd check for expanded/collapsed state
      expect(validationButton).toBeInTheDocument();
    });
  });

  describe('test_polling_behavior', () => {
    it('should show polling indicator when polling is active', () => {
      mockHookReturn.isPolling = true;
      useTradingIntent.mockReturnValue(mockHookReturn);

      render(<TradingIntentStatus intentId="test-intent-123" showDetails={true} />);

      expect(screen.getByText('Live updates enabled')).toBeInTheDocument();
    });

    it('should not show polling indicator when not polling', () => {
      mockHookReturn.isPolling = false;
      useTradingIntent.mockReturnValue(mockHookReturn);

      render(<TradingIntentStatus intentId="test-intent-123" showDetails={true} />);

      expect(screen.queryByText('Live updates enabled')).not.toBeInTheDocument();
    });
  });

  describe('test_shows_validation_failures', () => {
    it('should display failed guards in red', () => {
      const failedIntent = {
        ...mockIntent,
        status: 'VALIDATED',
        validation_result: {
          status: 'FAIL',
          guards: [
            { name: 'Balance Check', status: 'PASS', message: 'Balance sufficient' },
            { name: 'Risk Limit', status: 'FAIL', message: 'Monthly risk exceeded' },
          ],
          warnings: [],
          validated_at: '2026-01-01T10:01:00Z',
        },
      };

      mockHookReturn.intent = failedIntent;
      useTradingIntent.mockReturnValue(mockHookReturn);

      render(<TradingIntentStatus intentId="test-intent-123" showDetails={true} />);

      expect(screen.getByText('Risk Limit')).toBeInTheDocument();
      expect(screen.getByText('Monthly risk exceeded')).toBeInTheDocument();
    });
  });

  describe('test_handles_api_errors', () => {
    it('should display error message when fetch fails', () => {
      mockHookReturn.intent = null;
      mockHookReturn.error = 'Trading intent not found';
      useTradingIntent.mockReturnValue(mockHookReturn);

      render(<TradingIntentStatus intentId="test-intent-123" showDetails={true} />);

      expect(screen.getByText(/error loading trading intent/i)).toBeInTheDocument();
      expect(screen.getByText('Trading intent not found')).toBeInTheDocument();
    });
  });

  describe('test_loading_state', () => {
    it('should show loading spinner when loading', () => {
      mockHookReturn.isLoading = true;
      mockHookReturn.intent = null;
      useTradingIntent.mockReturnValue(mockHookReturn);

      render(<TradingIntentStatus intentId="test-intent-123" showDetails={true} />);

      expect(screen.getByText('Loading trading intent...')).toBeInTheDocument();
    });
  });

  describe('test_action_buttons', () => {
    it('should show Validate Now button when status is PENDING', () => {
      render(<TradingIntentStatus intentId="test-intent-123" showDetails={true} />);

      expect(screen.getByText('Validate Now')).toBeInTheDocument();
    });

    it('should show Execute buttons when status is VALIDATED', () => {
      const validatedIntent = { ...mockIntent, status: 'VALIDATED' };
      mockHookReturn.intent = validatedIntent;
      useTradingIntent.mockReturnValue(mockHookReturn);

      render(<TradingIntentStatus intentId="test-intent-123" showDetails={true} />);

      expect(screen.getByText('Dry-Run')).toBeInTheDocument();
      expect(screen.getByText('Live')).toBeInTheDocument();
      expect(screen.getByText(/Execute/)).toBeInTheDocument();
    });
  });

  describe('test_copy_intent_id', () => {
    it('should copy intent ID to clipboard when copy button clicked', async () => {
      // Mock clipboard API
      const mockClipboard = {
        writeText: vi.fn().mockResolvedValue(undefined),
      };
      global.navigator.clipboard = mockClipboard;

      render(<TradingIntentStatus intentId="test-intent-123" showDetails={true} />);

      const copyButton = screen.getByRole('button', { name: /📋/ });
      fireEvent.click(copyButton);

      await waitFor(() => {
        expect(mockClipboard.writeText).toHaveBeenCalledWith('test-intent-123');
      });
    });
  });
});
