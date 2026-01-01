// @vitest-environment jsdom
import React from 'react';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import StartNewOperationModal from '../src/components/logged/modals/StartNewOperationModal';
import AuthContext from '../src/context/AuthContext';

// Mock AuthContext
const mockAuthTokens = {
  access: 'mock-access-token',
  refresh: 'mock-refresh-token',
};

const mockAuthContext = {
  authTokens: mockAuthTokens,
  user: { username: 'testuser' },
  loginUser: vi.fn(),
  logoutUser: vi.fn(),
};

// Wrapper component with AuthContext
const renderWithAuth = (ui, authContextValue = mockAuthContext) => {
  return render(
    <AuthContext.Provider value={authContextValue}>
      {ui}
    </AuthContext.Provider>
  );
};

// Mock fetch globally
global.fetch = vi.fn();

describe('StartNewOperationModal', () => {
  const mockOnHide = vi.fn();
  const mockOnSuccess = vi.fn();

  const defaultProps = {
    show: true,
    onHide: mockOnHide,
    onSuccess: mockOnSuccess,
  };

  beforeEach(() => {
    // Reset mocks before each test
    vi.clearAllMocks();
    global.fetch.mockClear();

    // Mock successful API responses for symbols and strategies
    global.fetch.mockImplementation((url) => {
      if (url.includes('/api/symbols/')) {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            results: [
              { id: 1, base_asset: 'BTC', quote_asset: 'USDT' },
              { id: 2, base_asset: 'ETH', quote_asset: 'USDT' },
            ],
          }),
        });
      }
      if (url.includes('/api/strategies/')) {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            results: [
              { id: 1, name: 'Mean Reversion MA99' },
              { id: 2, name: 'Breakout Consolidation' },
            ],
          }),
        });
      }
      return Promise.reject(new Error('Unknown URL'));
    });
  });

  it('renders all required form fields', async () => {
    renderWithAuth(<StartNewOperationModal {...defaultProps} />);

    // Wait for data to load
    await waitFor(() => {
      expect(screen.getByText('Trading Pair')).toBeInTheDocument();
    });

    // Check all fields are present
    expect(screen.getByText('Trading Pair')).toBeInTheDocument();
    expect(screen.getByText('Strategy')).toBeInTheDocument();
    expect(screen.getByText('Side')).toBeInTheDocument();
    expect(screen.getByText('Entry Price')).toBeInTheDocument();
    expect(screen.getByText('Stop Price')).toBeInTheDocument();
    expect(screen.getByText('Capital')).toBeInTheDocument();

    // Check radio buttons
    expect(screen.getByLabelText('BUY (Long)')).toBeInTheDocument();
    expect(screen.getByLabelText('SELL (Short)')).toBeInTheDocument();

    // Check buttons
    expect(screen.getByText('Create Plan')).toBeInTheDocument();
    expect(screen.getByText('Cancel')).toBeInTheDocument();
  });

  it('validates required fields on submit', async () => {
    renderWithAuth(<StartNewOperationModal {...defaultProps} />);

    await waitFor(() => {
      expect(screen.getByRole('button', { name: /Create Plan/ })).toBeInTheDocument();
    });

    // Click submit without filling form
    const submitButton = screen.getByRole('button', { name: /Create Plan/ });
    fireEvent.click(submitButton);

    // Wait for validation errors to appear
    await waitFor(() => {
      expect(screen.getByText('Symbol is required')).toBeInTheDocument();
    }, { timeout: 3000 });

    expect(screen.getByText('Strategy is required')).toBeInTheDocument();
    expect(screen.getByText('Entry price is required')).toBeInTheDocument();
    expect(screen.getByText('Stop price is required')).toBeInTheDocument();
    expect(screen.getByText('Capital is required')).toBeInTheDocument();
  });

  it('validates entry price must not equal stop price', async () => {
    renderWithAuth(<StartNewOperationModal {...defaultProps} />);

    await waitFor(() => {
      expect(screen.getByRole('button', { name: /Create Plan/ })).toBeInTheDocument();
    });

    // Fill form with same entry and stop price
    // Use document.querySelectorAll since modal renders in portal
    const allSelects = screen.getAllByRole('combobox');
    const symbolSelect = allSelects[0];
    const strategySelect = allSelects[1];
    const entryPriceInput = screen.getAllByPlaceholderText('0.00')[0];
    const stopPriceInput = screen.getAllByPlaceholderText('0.00')[1];
    const capitalInput = screen.getByPlaceholderText('1000.00');

    fireEvent.change(symbolSelect, { target: { value: '1' } });
    fireEvent.change(strategySelect, { target: { value: '1' } });
    fireEvent.change(entryPriceInput, { target: { value: '50000' } });
    fireEvent.change(stopPriceInput, { target: { value: '50000' } });
    fireEvent.change(capitalInput, { target: { value: '10000' } });

    // Submit
    const submitButton = screen.getByRole('button', { name: /Create Plan/ });
    fireEvent.click(submitButton);

    // Wait for validation error
    await waitFor(() => {
      expect(screen.getByText('Stop price must be different from entry price')).toBeInTheDocument();
    });
  });

  it('submits successfully with valid data', async () => {
    // Mock successful POST request
    global.fetch.mockImplementation((url, options) => {
      if (url.includes('/api/symbols/')) {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            results: [{ id: 1, base_asset: 'BTC', quote_asset: 'USDT' }],
          }),
        });
      }
      if (url.includes('/api/strategies/')) {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            results: [{ id: 1, name: 'Mean Reversion MA99' }],
          }),
        });
      }
      if (url.includes('/api/trading-intents/create/') && options?.method === 'POST') {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            id: 123,
            symbol: 1,
            symbol_display: 'BTC/USDT',
            strategy: 1,
            side: 'BUY',
            entry_price: '50000',
            stop_price: '48000',
            capital: '10000',
            status: 'PENDING',
          }),
        });
      }
      return Promise.reject(new Error('Unknown URL'));
    });

    renderWithAuth(<StartNewOperationModal {...defaultProps} />);

    await waitFor(() => {
      expect(screen.getByRole('button', { name: /Create Plan/ })).toBeInTheDocument();
    });

    // Fill form
    const allSelects = screen.getAllByRole('combobox');
    const symbolSelect = allSelects[0];
    const strategySelect = allSelects[1];
    const entryPriceInput = screen.getAllByPlaceholderText('0.00')[0];
    const stopPriceInput = screen.getAllByPlaceholderText('0.00')[1];
    const capitalInput = screen.getByPlaceholderText('1000.00');

    fireEvent.change(symbolSelect, { target: { value: '1' } });
    fireEvent.change(strategySelect, { target: { value: '1' } });
    fireEvent.change(entryPriceInput, { target: { value: '50000' } });
    fireEvent.change(stopPriceInput, { target: { value: '48000' } });
    fireEvent.change(capitalInput, { target: { value: '10000' } });

    // Submit
    const submitButton = screen.getByRole('button', { name: /Create Plan/ });
    fireEvent.click(submitButton);

    // Wait for success
    await waitFor(() => {
      expect(mockOnSuccess).toHaveBeenCalledWith(
        expect.objectContaining({
          id: 123,
          side: 'BUY',
          symbol_display: 'BTC/USDT',
        })
      );
    });

    expect(mockOnHide).toHaveBeenCalled();
  });

  it('shows API error message gracefully', async () => {
    // Mock failed POST request
    global.fetch.mockImplementation((url, options) => {
      if (url.includes('/api/symbols/')) {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            results: [{ id: 1, base_asset: 'BTC', quote_asset: 'USDT' }],
          }),
        });
      }
      if (url.includes('/api/strategies/')) {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            results: [{ id: 1, name: 'Mean Reversion MA99' }],
          }),
        });
      }
      if (url.includes('/api/trading-intents/create/') && options?.method === 'POST') {
        return Promise.resolve({
          ok: false,
          status: 400,
          json: async () => ({
            detail: 'Insufficient balance to execute this trade',
          }),
        });
      }
      return Promise.reject(new Error('Unknown URL'));
    });

    renderWithAuth(<StartNewOperationModal {...defaultProps} />);

    await waitFor(() => {
      expect(screen.getByRole('button', { name: /Create Plan/ })).toBeInTheDocument();
    });

    // Fill form
    const allSelects = screen.getAllByRole('combobox');
    const symbolSelect = allSelects[0];
    const strategySelect = allSelects[1];
    const entryPriceInput = screen.getAllByPlaceholderText('0.00')[0];
    const stopPriceInput = screen.getAllByPlaceholderText('0.00')[1];
    const capitalInput = screen.getByPlaceholderText('1000.00');

    fireEvent.change(symbolSelect, { target: { value: '1' } });
    fireEvent.change(strategySelect, { target: { value: '1' } });
    fireEvent.change(entryPriceInput, { target: { value: '50000' } });
    fireEvent.change(stopPriceInput, { target: { value: '48000' } });
    fireEvent.change(capitalInput, { target: { value: '10000' } });

    // Submit
    const submitButton = screen.getByRole('button', { name: /Create Plan/ });
    fireEvent.click(submitButton);

    // Wait for error message
    await waitFor(() => {
      expect(screen.getByText(/Insufficient balance to execute this trade/)).toBeInTheDocument();
    });

    // Modal should stay open
    expect(mockOnHide).not.toHaveBeenCalled();
  });

  it('disables form during submission', async () => {
    // Mock slow POST request
    let resolveSubmit;
    global.fetch.mockImplementation((url, options) => {
      if (url.includes('/api/symbols/')) {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            results: [{ id: 1, base_asset: 'BTC', quote_asset: 'USDT' }],
          }),
        });
      }
      if (url.includes('/api/strategies/')) {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            results: [{ id: 1, name: 'Mean Reversion MA99' }],
          }),
        });
      }
      if (url.includes('/api/trading-intents/create/') && options?.method === 'POST') {
        return new Promise((resolve) => {
          resolveSubmit = resolve;
        });
      }
      return Promise.reject(new Error('Unknown URL'));
    });

    renderWithAuth(<StartNewOperationModal {...defaultProps} />);

    await waitFor(() => {
      expect(screen.getByRole('button', { name: /Create Plan/ })).toBeInTheDocument();
    });

    // Fill form
    const allSelects = screen.getAllByRole('combobox');
    const symbolSelect = allSelects[0];
    const strategySelect = allSelects[1];
    const entryPriceInput = screen.getAllByPlaceholderText('0.00')[0];
    const stopPriceInput = screen.getAllByPlaceholderText('0.00')[1];
    const capitalInput = screen.getByPlaceholderText('1000.00');

    fireEvent.change(symbolSelect, { target: { value: '1' } });
    fireEvent.change(strategySelect, { target: { value: '1' } });
    fireEvent.change(entryPriceInput, { target: { value: '50000' } });
    fireEvent.change(stopPriceInput, { target: { value: '48000' } });
    fireEvent.change(capitalInput, { target: { value: '10000' } });

    // Submit
    const submitButton = screen.getByRole('button', { name: /Create Plan/ });
    fireEvent.click(submitButton);

    // Wait for loading state
    await waitFor(() => {
      expect(screen.getByText('Creating Plan...')).toBeInTheDocument();
    });

    // Check inputs are disabled
    expect(entryPriceInput).toBeDisabled();
    expect(stopPriceInput).toBeDisabled();
    expect(capitalInput).toBeDisabled();

    // Resolve the promise
    if (resolveSubmit) {
      resolveSubmit({
        ok: true,
        json: async () => ({
          id: 123,
          symbol: 1,
          symbol_display: 'BTC/USDT',
          strategy: 1,
          side: 'BUY',
          entry_price: '50000',
          stop_price: '48000',
          capital: '10000',
          status: 'PENDING',
        }),
      });
    }
  });

  it('displays calculated position size preview', async () => {
    renderWithAuth(<StartNewOperationModal {...defaultProps} />);

    await waitFor(() => {
      expect(screen.getByRole('button', { name: /Create Plan/ })).toBeInTheDocument();
    });

    // Fill form
    const allSelects = screen.getAllByRole('combobox');
    const symbolSelect = allSelects[0];
    const entryPriceInput = screen.getAllByPlaceholderText('0.00')[0];
    const stopPriceInput = screen.getAllByPlaceholderText('0.00')[1];
    const capitalInput = screen.getByPlaceholderText('1000.00');

    fireEvent.change(symbolSelect, { target: { value: '1' } });
    fireEvent.change(entryPriceInput, { target: { value: '50000' } });
    fireEvent.change(stopPriceInput, { target: { value: '48000' } });
    fireEvent.change(capitalInput, { target: { value: '10000' } });

    // Wait for calculation
    await waitFor(() => {
      expect(screen.getByText(/Calculated Position Size:/)).toBeInTheDocument();
    });

    // Check calculation is displayed - use more specific selector
    // Position Size = (10000 × 0.01) / |50000 - 48000| = 100 / 2000 = 0.05 BTC
    expect(screen.getByText(/0\.05/)).toBeInTheDocument();
    // Use getAllByText since BTC appears in multiple places
    expect(screen.getAllByText(/BTC/).length).toBeGreaterThan(0);
  });
});
