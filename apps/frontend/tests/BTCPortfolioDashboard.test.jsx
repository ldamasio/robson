/**
 * Frontend Tests for BTC Portfolio Dashboard
 *
 * Tests critical UI behavior to prevent production bugs:
 * - Loading states
 * - Error handling
 * - Data rendering
 * - User interactions
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import '@testing-library/jest-dom';

// Mock axios
vi.mock('axios');

// Mock environment variables
import.meta.env.VITE_API_BASE_URL = 'http://localhost:8000';

describe('BTCPortfolioDashboard', () => {
  const mockPortfolioData = {
    total_btc: '0.52340000',
    spot_btc: '0.50000000',
    margin_btc: '0.02340000',
    margin_debt_btc: '0.00000000',
    breakdown: {
      BTC: '0.50000000',
      ETH: '0.02000000',
    },
  };

  const mockProfitData = {
    profit_btc: '0.02340000',
    profit_percent: '4.67',
    current_balance_btc: '0.52340000',
    total_deposits_btc: '0.50000000',
    total_withdrawals_btc: '0.00000000',
    net_inflows_btc: '0.50000000',
  };

  beforeEach(() => {
    // Clear all mocks before each test
    vi.clearAllMocks();
  });

  describe('Loading State', () => {
    it('should show spinner when loading', async () => {
      const axios = (await import('axios')).default;

      // Mock pending requests
      axios.get.mockImplementation(() => new Promise(() => {}));

      const { BTCPortfolioDashboard } = await import(
        '../BTCPortfolioDashboard'
      );

      render(<BTCPortfolioDashboard />);

      expect(screen.getByText(/Loading portfolio data/i)).toBeInTheDocument();
    });

    it('should hide spinner after data loads', async () => {
      const axios = (await import('axios')).default;

      // Mock successful response
      axios.get.mockResolvedValue({
        data: mockPortfolioData,
      });

      axios.get.mockResolvedValue({
        data: mockProfitData,
      });

      const { BTCPortfolioDashboard } = await import(
        '../BTCPortfolioDashboard'
      );

      render(<BTCPortfolioDashboard />);

      await waitFor(() => {
        expect(screen.queryByText(/Loading portfolio data/i)).not.toBeInTheDocument();
      });
    });
  });

  describe('Error Handling', () => {
    it('should display error message when API fails', async () => {
      const axios = (await import('axios')).default;

      // Mock API error
      axios.get.mockRejectedValue(new Error('Network error'));

      const { BTCPortfolioDashboard } = await import(
        '../BTCPortfolioDashboard'
      );

      render(<BTCPortfolioDashboard />);

      await waitFor(() => {
        expect(screen.getByText(/Error: Network error/i)).toBeInTheDocument();
      });
    });

    it('should show danger styling on error', async () => {
      const axios = (await import('axios')).default;

      axios.get.mockRejectedValue(new Error('API Error'));

      const { BTCPortfolioDashboard } = await import(
        '../BTCPortfolioDashboard'
      );

      render(<BTCPortfolioDashboard />);

      await waitFor(() => {
        const errorCard = screen
          .getByText(/Error: API Error/i)
          .closest('.card');
        expect(errorCard).toHaveClass('border-danger');
      });
    });
  });

  describe('Overview Tab', () => {
    it('should display total portfolio value in BTC', async () => {
      const axios = (await import('axios')).default;

      axios.get.mockImplementation((url) => {
        if (url.includes('total')) {
          return Promise.resolve({ data: mockPortfolioData });
        }
        if (url.includes('profit')) {
          return Promise.resolve({ data: mockProfitData });
        }
        return Promise.reject(new Error('Unknown URL'));
      });

      const { BTCPortfolioDashboard } = await import(
        '../BTCPortfolioDashboard'
      );

      render(<BTCPortfolioDashboard />);

      await waitFor(() => {
        expect(screen.getByText(/0\.52340000 BTC/i)).toBeInTheDocument();
      });
    });

    it('should display profit with green color when positive', async () => {
      const axios = (await import('axios')).default;

      axios.get.mockImplementation((url) => {
        if (url.includes('total')) {
          return Promise.resolve({ data: mockPortfolioData });
        }
        return Promise.resolve({ data: mockProfitData });
      });

      const { BTCPortfolioDashboard } = await import(
        '../BTCPortfolioDashboard'
      );

      render(<BTCPortfolioDashboard />);

      await waitFor(() => {
        const profitBadge = screen.getByText(/↑/i);
        expect(profitBadge).toBeInTheDocument();
        expect(profitBadge.closest('.badge-success')).toBeInTheDocument();
      });
    });

    it('should display loss with red color when negative', async () => {
      const axios = (await import('axios')).default;

      const lossData = {
        ...mockProfitData,
        profit_btc: '-0.05000000',
        profit_percent: '-10.0',
      };

      axios.get.mockImplementation((url) => {
        if (url.includes('total')) {
          return Promise.resolve({ data: mockPortfolioData });
        }
        return Promise.resolve({ data: lossData });
      });

      const { BTCPortfolioDashboard } = await import(
        '../BTCPortfolioDashboard'
      );

      render(<BTCPortfolioDashboard />);

      await waitFor(() => {
        const lossBadge = screen.getByText(/↓/i);
        expect(lossBadge).toBeInTheDocument();
        expect(lossBadge.closest('.badge-danger')).toBeInTheDocument();
      });
    });

    it('should display account breakdown correctly', async () => {
      const axios = (await import('axios')).default;

      axios.get.mockImplementation((url) => {
        if (url.includes('total')) {
          return Promise.resolve({ data: mockPortfolioData });
        }
        return Promise.resolve({ data: mockProfitData });
      });

      const { BTCPortfolioDashboard } = await import(
        '../BTCPortfolioDashboard'
      );

      render(<BTCPortfolioDashboard />);

      await waitFor(() => {
        expect(screen.getByText(/Spot:/i)).toBeInTheDocument();
        expect(screen.getByText(/Margin:/i)).toBeInTheDocument();
        expect(screen.getByText(/Margin Debt:/i)).toBeInTheDocument();
      });
    });
  });

  describe('Tab Navigation', () => {
    it('should switch to history tab when clicked', async () => {
      const axios = (await import('axios')).default;

      axios.get.mockResolvedValue({
        data: mockPortfolioData,
      });

      const { BTCPortfolioDashboard } = await import(
        '../BTCPortfolioDashboard'
      );

      render(<BTCPortfolioDashboard />);

      await waitFor(() => {
        expect(screen.getByText(/📊 Overview/i)).toBeInTheDocument();
      });

      // Click history tab
      fireEvent.click(screen.getByText(/📈 History/i));

      await waitFor(() => {
        expect(screen.getByText(/Time Range/i)).toBeInTheDocument();
      });
    });

    it('should switch to transactions tab when clicked', async () => {
      const axios = (await import('axios')).default;

      axios.get.mockResolvedValue({
        data: mockPortfolioData,
      });

      const { BTCPortfolioDashboard } = await import(
        '../BTCPortfolioDashboard'
      );

      render(<BTCPortfolioDashboard />);

      await waitFor(() => {
        expect(screen.getByText(/📊 Overview/i)).toBeInTheDocument();
      });

      // Click transactions tab
      fireEvent.click(screen.getByText(/💰 Transactions/i));

      await waitFor(() => {
        expect(screen.getByText(/Filter by Type/i)).toBeInTheDocument();
      });
    });
  });

  describe('History Tab', () => {
    it('should display time range selector', async () => {
      const axios = (await import('axios')).default;

      axios.get.mockResolvedValue({ data: mockPortfolioData });

      const { BTCPortfolioDashboard } = await import(
        '../BTCPortfolioDashboard'
      );

      render(<BTCPortfolioDashboard />);

      await waitFor(() => {
        fireEvent.click(screen.getByText(/📈 History/i));
      });

      await waitFor(() => {
        expect(screen.getByLabelText(/Time Range/i)).toBeInTheDocument();
        expect(screen.getByText(/Last 7 days/i)).toBeInTheDocument();
        expect(screen.getByText(/Last 30 days/i)).toBeInTheDocument();
      });
    });

    it('should fetch history when time range changes', async () => {
      const axios = (await import('axios')).default;

      axios.get.mockResolvedValue({ data: mockPortfolioData });

      const { BTCPortfolioDashboard } = await import(
        '../BTCPortfolioDashboard'
      );

      render(<BTCPortfolioDashboard />);

      await waitFor(() => {
        fireEvent.click(screen.getByText(/📈 History/i));
      });

      await waitFor(() => {
        const select = screen.getByLabelText(/Time Range/i);
        fireEvent.change(select, { target: { value: '90' } });
      });

      // Verify API was called with correct date range
      await waitFor(() => {
        expect(axios.get).toHaveBeenCalledWith(
          expect.stringContaining('start_date=')
        );
      });
    });
  });

  describe('Transactions Tab', () => {
    it('should display filter selector', async () => {
      const axios = (await import('axios')).default;

      axios.get.mockResolvedValue({ data: mockPortfolioData });

      const { BTCPortfolioDashboard } = await import(
        '../BTCPortfolioDashboard'
      );

      render(<BTCPortfolioDashboard />);

      await waitFor(() => {
        fireEvent.click(screen.getByText(/💰 Transactions/i));
      });

      await waitFor(() => {
        expect(screen.getByLabelText(/Filter by Type/i)).toBeInTheDocument();
      });
    });

    it('should filter transactions by type', async () => {
      const axios = (await import('axios')).default;

      axios.get.mockResolvedValue({
        data: mockPortfolioData,
      });

      axios.get.mockResolvedValue({
        data: {
          transactions: [
            {
              id: '1',
              type: 'DEPOSIT',
              asset: 'BTC',
              quantity: '1.00000000',
              executed_at: '2025-12-26T10:00:00Z',
              btc_value: '1.00000000',
            },
          ],
        },
      });

      const { BTCPortfolioDashboard } = await import(
        '../BTCPortfolioDashboard'
      );

      render(<BTCPortfolioDashboard />);

      await waitFor(() => {
        fireEvent.click(screen.getByText(/💰 Transactions/i));
      });

      await waitFor(() => {
        const select = screen.getByLabelText(/Filter by Type/i);
        fireEvent.change(select, { target: { value: 'deposit' } });
      });

      await waitFor(() => {
        expect(screen.getByText(/DEPOSIT/i)).toBeInTheDocument();
      });
    });

    it('should display transactions in table', async () => {
      const axios = (await import('axios')).default;

      axios.get.mockResolvedValue({ data: mockPortfolioData });

      axios.get.mockResolvedValue({
        data: {
          transactions: [
            {
              id: '1',
              type: 'DEPOSIT',
              asset: 'BTC',
              quantity: '1.00000000',
              executed_at: '2025-12-26T10:00:00Z',
              btc_value: '1.00000000',
            },
          ],
        },
      });

      const { BTCPortfolioDashboard } = await import(
        '../BTCPortfolioDashboard'
      );

      render(<BTCPortfolioDashboard />);

      await waitFor(() => {
        fireEvent.click(screen.getByText(/💰 Transactions/i));
      });

      await waitFor(() => {
        expect(screen.getByText(/Date/i)).toBeInTheDocument();
        expect(screen.getByText(/Type/i)).toBeInTheDocument();
        expect(screen.getByText(/Asset/i)).toBeInTheDocument();
        expect(screen.getByText(/Amount/i)).toBeInTheDocument();
        expect(screen.getByText(/BTC Value/i)).toBeInTheDocument();
      });
    });
  });

  describe('Auto-refresh', () => {
    it('should refresh data every 60 seconds', async () => {
      vi.useFakeTimers();

      const axios = (await import('axios')).default;

      axios.get.mockResolvedValue({
        data: mockPortfolioData,
      });

      const { BTCPortfolioDashboard } = await import(
        '../BTCPortfolioDashboard'
      );

      render(<BTCPortfolioDashboard />);

      await waitFor(() => {
        expect(axios.get).toHaveBeenCalledTimes(2); // Initial load (total + profit)
      });

      // Fast-forward 60 seconds
      vi.advanceTimersByTime(60000);

      await waitFor(() => {
        expect(axios.get).toHaveBeenCalledTimes(4); // Should refresh
      });

      vi.useRealTimers();
    });
  });
});
