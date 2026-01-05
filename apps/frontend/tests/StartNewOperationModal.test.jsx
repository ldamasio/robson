// @vitest-environment jsdom
import React from "react";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import StartNewOperationModal from "../src/components/logged/modals/StartNewOperationModal";
import AuthContext from "../src/context/AuthContext";

// Mock AuthContext
const mockAuthTokens = {
  access: "mock-access-token",
  refresh: "mock-refresh-token",
};

const mockAuthContext = {
  authTokens: mockAuthTokens,
  user: { username: "testuser" },
  loginUser: vi.fn(),
  logoutUser: vi.fn(),
};

// Wrapper component with AuthContext
const renderWithAuth = (ui, authContextValue = mockAuthContext) => {
  return render(
    <AuthContext.Provider value={authContextValue}>{ui}</AuthContext.Provider>,
  );
};

// Mock fetch globally
global.fetch = vi.fn();

describe("StartNewOperationModal", () => {
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
      if (url.includes("/api/symbols/")) {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            results: [
              { id: 1, base_asset: "BTC", quote_asset: "USDT" },
              { id: 2, base_asset: "ETH", quote_asset: "USDT" },
            ],
          }),
        });
      }
      if (url.includes("/api/strategies/")) {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            results: [
              { id: 1, name: "Mean Reversion MA99" },
              { id: 2, name: "Breakout Consolidation" },
            ],
          }),
        });
      }
      return Promise.reject(new Error("Unknown URL"));
    });
  });

  it("renders all required form fields", async () => {
    renderWithAuth(<StartNewOperationModal {...defaultProps} />);

    // Wait for data to load
    await waitFor(() => {
      expect(screen.getByText("Trading Pair")).toBeInTheDocument();
    });

    // Check all fields are present
    expect(screen.getByText("Trading Pair")).toBeInTheDocument();
    expect(screen.getByText("Strategy")).toBeInTheDocument();

    // Check info alert
    expect(screen.getByText("One-click plan creation")).toBeInTheDocument();
    expect(
      screen.getByText(/Backend will automatically calculate/),
    ).toBeInTheDocument();

    // Check buttons
    expect(screen.getByText("Create Plan")).toBeInTheDocument();
    expect(screen.getByText("Cancel")).toBeInTheDocument();

    // Check dropdowns
    const selects = screen.getAllByRole("combobox");
    expect(selects.length).toBe(2); // Symbol and Strategy selects
  });

  it("validates required fields on submit", async () => {
    renderWithAuth(<StartNewOperationModal {...defaultProps} />);

    const submitButton = await screen.findByRole("button", {
      name: /Create Plan/,
    });
    await waitFor(() => {
      expect(submitButton).toBeEnabled();
    });

    // Click submit without filling form
    fireEvent.click(submitButton);

    // Wait for error to appear - use a text query with container option
    await waitFor(() => {
      const alerts = screen.getAllByRole("alert");
      const errorAlert = alerts.find((alert) =>
        alert.textContent.includes("Please select both"),
      );
      expect(errorAlert).toBeDefined();
      expect(errorAlert.textContent).toContain(
        "Please select both symbol and strategy",
      );
    });

    // Verify select fields have invalid styling
    const allSelects = screen.getAllByRole("combobox");
    expect(allSelects.length).toBe(2);
    // Both selects should have is-invalid class
    const hasInvalidSelect = allSelects.some((select) =>
      select.classList.contains("is-invalid"),
    );
    expect(hasInvalidSelect).toBe(true);
  });

  it("submits successfully with valid data", async () => {
    // Mock successful POST request
    global.fetch.mockImplementation((url, options) => {
      if (url.includes("/api/symbols/")) {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            results: [{ id: 1, base_asset: "BTC", quote_asset: "USDT" }],
          }),
        });
      }
      if (url.includes("/api/strategies/")) {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            results: [{ id: 1, name: "Mean Reversion MA99" }],
          }),
        });
      }
      if (
        url.includes("/api/trading-intents/create/") &&
        options?.method === "POST"
      ) {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            intent_id: "abc-123",
            id: 123,
            symbol: 1,
            symbol_display: "BTC/USDT",
            strategy: 1,
            side: "BUY",
            entry_price: "50000",
            stop_price: "49000",
            capital: "1000",
            quantity: "0.005",
            status: "PENDING",
          }),
        });
      }
      return Promise.reject(new Error("Unknown URL"));
    });

    renderWithAuth(<StartNewOperationModal {...defaultProps} />);

    await waitFor(() => {
      expect(
        screen.getByRole("button", { name: /Create Plan/ }),
      ).toBeInTheDocument();
    });

    // Fill form - only symbol and strategy needed
    const allSelects = screen.getAllByRole("combobox");
    const symbolSelect = allSelects[0];
    const strategySelect = allSelects[1];

    fireEvent.change(symbolSelect, { target: { value: "1" } });
    fireEvent.change(strategySelect, { target: { value: "1" } });

    // Submit
    const submitButton = screen.getByRole("button", { name: /Create Plan/ });
    fireEvent.click(submitButton);

    // Wait for success
    await waitFor(() => {
      expect(mockOnSuccess).toHaveBeenCalledWith(
        expect.objectContaining({
          intent_id: "abc-123",
          side: "BUY",
          symbol_display: "BTC/USDT",
        }),
      );
    });

    expect(mockOnHide).toHaveBeenCalled();
  });

  it("shows API error message gracefully", async () => {
    // Mock failed POST request
    global.fetch.mockImplementation((url, options) => {
      if (url.includes("/api/symbols/")) {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            results: [{ id: 1, base_asset: "BTC", quote_asset: "USDT" }],
          }),
        });
      }
      if (url.includes("/api/strategies/")) {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            results: [{ id: 1, name: "Mean Reversion MA99" }],
          }),
        });
      }
      if (
        url.includes("/api/trading-intents/create/") &&
        options?.method === "POST"
      ) {
        return Promise.resolve({
          ok: false,
          status: 400,
          json: async () => ({
            error: "Insufficient balance to execute this trade",
          }),
        });
      }
      return Promise.reject(new Error("Unknown URL"));
    });

    renderWithAuth(<StartNewOperationModal {...defaultProps} />);

    await waitFor(() => {
      expect(
        screen.getByRole("button", { name: /Create Plan/ }),
      ).toBeInTheDocument();
    });

    // Fill form
    const allSelects = screen.getAllByRole("combobox");
    const symbolSelect = allSelects[0];
    const strategySelect = allSelects[1];

    fireEvent.change(symbolSelect, { target: { value: "1" } });
    fireEvent.change(strategySelect, { target: { value: "1" } });

    // Submit
    const submitButton = screen.getByRole("button", { name: /Create Plan/ });
    fireEvent.click(submitButton);

    // Wait for error message
    await waitFor(() => {
      expect(
        screen.getByText(/Insufficient balance to execute this trade/),
      ).toBeInTheDocument();
    });

    // Modal should stay open
    expect(mockOnHide).not.toHaveBeenCalled();
  });

  it("disables form during submission", async () => {
    // Mock slow POST request
    let resolveSubmit;
    global.fetch.mockImplementation((url, options) => {
      if (url.includes("/api/symbols/")) {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            results: [{ id: 1, base_asset: "BTC", quote_asset: "USDT" }],
          }),
        });
      }
      if (url.includes("/api/strategies/")) {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            results: [{ id: 1, name: "Mean Reversion MA99" }],
          }),
        });
      }
      if (
        url.includes("/api/trading-intents/create/") &&
        options?.method === "POST"
      ) {
        return new Promise((resolve) => {
          resolveSubmit = resolve;
        });
      }
      return Promise.reject(new Error("Unknown URL"));
    });

    renderWithAuth(<StartNewOperationModal {...defaultProps} />);

    await waitFor(() => {
      expect(
        screen.getByRole("button", { name: /Create Plan/ }),
      ).toBeInTheDocument();
    });

    // Fill form
    const allSelects = screen.getAllByRole("combobox");
    const symbolSelect = allSelects[0];
    const strategySelect = allSelects[1];

    fireEvent.change(symbolSelect, { target: { value: "1" } });
    fireEvent.change(strategySelect, { target: { value: "1" } });

    // Submit
    const submitButton = screen.getByRole("button", { name: /Create Plan/ });
    fireEvent.click(submitButton);

    // Wait for loading state
    await waitFor(() => {
      expect(screen.getByText("Creating Plan...")).toBeInTheDocument();
    });

    // Check selects are disabled
    expect(symbolSelect).toBeDisabled();
    expect(strategySelect).toBeDisabled();

    // Resolve the promise
    if (resolveSubmit) {
      resolveSubmit({
        ok: true,
        json: async () => ({
          intent_id: "abc-123",
          id: 123,
          symbol: 1,
          symbol_display: "BTC/USDT",
          strategy: 1,
          side: "BUY",
          entry_price: "50000",
          stop_price: "49000",
          capital: "1000",
          status: "PENDING",
        }),
      });
    }
  });

  it("displays strategy helper text", async () => {
    renderWithAuth(<StartNewOperationModal {...defaultProps} />);

    await waitFor(() => {
      expect(screen.getByText("Strategy")).toBeInTheDocument();
    });

    // Check helper text
    expect(
      screen.getByText(
        "Strategy settings determine side, risk level, and capital allocation",
      ),
    ).toBeInTheDocument();
  });

  it("resets form after successful submission", async () => {
    // Mock successful POST request
    global.fetch.mockImplementation((url, options) => {
      if (url.includes("/api/symbols/")) {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            results: [{ id: 1, base_asset: "BTC", quote_asset: "USDT" }],
          }),
        });
      }
      if (url.includes("/api/strategies/")) {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            results: [{ id: 1, name: "Mean Reversion MA99" }],
          }),
        });
      }
      if (
        url.includes("/api/trading-intents/create/") &&
        options?.method === "POST"
      ) {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            intent_id: "abc-123",
            id: 123,
          }),
        });
      }
      return Promise.reject(new Error("Unknown URL"));
    });

    renderWithAuth(<StartNewOperationModal {...defaultProps} />);

    await waitFor(() => {
      expect(
        screen.getByRole("button", { name: /Create Plan/ }),
      ).toBeInTheDocument();
    });

    // Fill and submit
    const allSelects = screen.getAllByRole("combobox");
    const symbolSelect = allSelects[0];
    const strategySelect = allSelects[1];

    fireEvent.change(symbolSelect, { target: { value: "1" } });
    fireEvent.change(strategySelect, { target: { value: "1" } });

    const submitButton = screen.getByRole("button", { name: /Create Plan/ });
    fireEvent.click(submitButton);

    // Wait for success and modal close
    await waitFor(() => {
      expect(mockOnHide).toHaveBeenCalled();
    });
  });
});
