// @vitest-environment jsdom
/**
 * Frontend Tests for BTC Portfolio Dashboard
 *
 * Tests critical UI behavior to prevent production bugs:
 * - Loading states
 * - Error handling
 * - Data rendering
 * - User interactions
 */

import React from 'react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import BTCPortfolioDashboard from '../src/components/logged/BTCPortfolioDashboard';

// Mock axios
vi.mock('axios', () => ({
  default: {
    get: vi.fn()
  }
}));

import axios from 'axios';

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
    vi.clearAllMocks();
  });

  describe('Loading State', () => {
    it('should show spinner when loading', async () => {
      // Mock pending requests
      axios.get.mockImplementation(() => new Promise(() => {}));

      render(<BTCPortfolioDashboard />);

      expect(screen.getByText(/Loading portfolio data/i)).toBeInTheDocument();
    });

    it('should hide spinner after data loads', async () => {
      // Mock successful response
      axios.get.mockResolvedValue({
        data: mockPortfolioData,
      });

      render(<BTCPortfolioDashboard />);

      await waitFor(() => {
        expect(screen.queryByText(/Loading portfolio data/i)).not.toBeInTheDocument();
      });
    });
  });

  describe('Error Handling', () => {
    it('should display error message when API fails', async () => {
      // Mock API error
      axios.get.mockRejectedValue(new Error('Network error'));

      render(<BTCPortfolioDashboard />);

      await waitFor(() => {
        expect(screen.getByText(/Error: Network error/i)).toBeInTheDocument();
      });
    });

    it('should show danger styling on error', async () => {
      axios.get.mockRejectedValue(new Error('API Error'));

      render(<BTCPortfolioDashboard />);

      await waitFor(() => {
        const errorCard = screen.getByText(/Error: API Error/i).closest('.card');
        expect(errorCard).toHaveClass('border-danger');
      });
    });
  });

  describe('Overview Tab', () => {
    it('should display total portfolio value in BTC', async () => {
      axios.get.mockImplementation((url) => {
        if (url.includes('total')) {
          return Promise.resolve({ data: mockPortfolioData });
        }
        if (url.includes('profit')) {
          return Promise.resolve({ data: mockProfitData });
        }
        return Promise.reject(new Error('Unknown URL'));
      });

      render(<BTCPortfolioDashboard />);

      await waitFor(() => {
        expect(screen.getByText(/0\.52340000 BTC/i)).toBeInTheDocument();
      });
    });

    it('should display profit with green color when positive', async () => {
      axios.get.mockImplementation((url) => {
        if (url.includes('total')) {
          return Promise.resolve({ data: mockPortfolioData });
        }
        return Promise.resolve({ data: mockProfitData });
      });

      render(<BTCPortfolioDashboard />);

      await waitFor(() => {
        const profitBadge = screen.getByText(/↑/i);
        expect(profitBadge).toBeInTheDocument();
      });
    });

    it('should display account breakdown correctly', async () => {
      axios.get.mockImplementation((url) => {
        if (url.includes('total')) {
          return Promise.resolve({ data: mockPortfolioData });
        }
        return Promise.resolve({ data: mockProfitData });
      });

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
      axios.get.mockResolvedValue({
        data: mockPortfolioData,
      });

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
      axios.get.mockResolvedValue({
        data: mockPortfolioData,
      });

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
      axios.get.mockResolvedValue({ data: mockPortfolioData });

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
  });

  describe('Transactions Tab', () => {
    it('should display filter selector', async () => {
      axios.get.mockResolvedValue({ data: mockPortfolioData });

      render(<BTCPortfolioDashboard />);

      await waitFor(() => {
        fireEvent.click(screen.getByText(/💰 Transactions/i));
      });

      await waitFor(() => {
        expect(screen.getByLabelText(/Filter by Type/i)).toBeInTheDocument();
      });
    });
  });
});
