// @vitest-environment jsdom
import React from "react";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import StartNewOperationModal from "../src/components/logged/modals/StartNewOperationModal";
import AuthContext from "../src/context/AuthContext";
import axios from "axios";

// Mock axios
vi.mock("axios");

// Mock react-toastify
vi.mock("react-toastify", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
  },
}));

// Mock AuthContext
const mockAuthTokens = {
  access: "mock-access-token",
};

const mockAuthContext = {
  authTokens: mockAuthTokens,
};

describe("StartNewOperationModal", () => {
  const mockOnHide = vi.fn();

  const defaultProps = {
    show: true,
    onHide: mockOnHide,
  };

  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders the simplified LONG/SHORT options", () => {
    render(
      <AuthContext.Provider value={mockAuthContext}>
        <StartNewOperationModal {...defaultProps} />
      </AuthContext.Provider>
    );

    expect(screen.getByText("Nova Operação BTC/USDC")).toBeInTheDocument();
    expect(screen.getByText("LONG")).toBeInTheDocument();
    expect(screen.getByText("SHORT")).toBeInTheDocument();
    expect(screen.getByText("Cancelar")).toBeInTheDocument();
  });

  it("calls the API with correct parameters when LONG is clicked", async () => {
    axios.post.mockResolvedValueOnce({ data: { success: true } });

    // Mock window.location.reload
    const originalLocation = window.location;
    delete window.location;
    window.location = { ...originalLocation, reload: vi.fn() };

    render(
      <AuthContext.Provider value={mockAuthContext}>
        <StartNewOperationModal {...defaultProps} />
      </AuthContext.Provider>
    );

    const longCard = screen.getByText("LONG").closest(".card");
    fireEvent.click(longCard);

    await waitFor(() => {
      expect(axios.post).toHaveBeenCalledWith(
        expect.stringContaining("/api/operations/"),
        {
          strategy_name: "BTC Long",
          symbol: "BTCUSDC",
          account_type: "ISOLATED_MARGIN",
        },
        expect.any(Object)
      );
    });

    // Cleanup mock
    window.location = originalLocation;
  });

  it("calls the API with correct parameters when SHORT is clicked", async () => {
    axios.post.mockResolvedValueOnce({ data: { success: true } });

    // Mock window.location.reload
    const originalLocation = window.location;
    delete window.location;
    window.location = { ...originalLocation, reload: vi.fn() };

    render(
      <AuthContext.Provider value={mockAuthContext}>
        <StartNewOperationModal {...defaultProps} />
      </AuthContext.Provider>
    );

    const shortCard = screen.getByText("SHORT").closest(".card");
    fireEvent.click(shortCard);

    await waitFor(() => {
      expect(axios.post).toHaveBeenCalledWith(
        expect.stringContaining("/api/operations/"),
        {
          strategy_name: "BTC Short",
          symbol: "BTCUSDC",
          account_type: "ISOLATED_MARGIN",
        },
        expect.any(Object)
      );
    });

    // Cleanup mock
    window.location = originalLocation;
  });
});
