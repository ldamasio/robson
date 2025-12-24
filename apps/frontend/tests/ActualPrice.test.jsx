// @vitest-environment jsdom
import React from 'react'
import { render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi, beforeEach } from 'vitest'
import ActualPrice from '../src/components/logged/ActualPrice'

// Mock the useWebSocket hook
const mockUseWebSocket = vi.fn()
vi.mock('../src/hooks/useWebSocket', () => ({
  default: (url) => mockUseWebSocket(url)
}))

vi.mock('../src/context/AuthContext', () => ({
  default: React.createContext({ authTokens: { access: 'token' } })
}))

describe('ActualPrice component', () => {
  beforeEach(() => {
    vi.restoreAllMocks()
  })

  it('renders loading state initially', () => {
    mockUseWebSocket.mockReturnValue({ data: null, isConnected: false })
    render(<ActualPrice />)
    expect(screen.getByLabelText(/connecting/i)).toBeTruthy()
  })

  it('renders current price data from websocket', async () => {
    mockUseWebSocket.mockReturnValue({
      data: {
        symbol: 'BTCUSDC',
        price: 89245.50,
        timestamp: 1700000000
      },
      isConnected: true
    })

    render(<ActualPrice />)

    await waitFor(() => {
      expect(screen.getByText('$89,245.50')).toBeTruthy()
    })
  })
})
