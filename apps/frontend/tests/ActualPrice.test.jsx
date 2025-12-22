// @vitest-environment jsdom
import React from 'react'
import { render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import ActualPrice from '../src/components/logged/ActualPrice'
import AuthContext from '../src/context/AuthContext'

vi.mock('axios', () => ({
  default: {
    get: vi.fn()
  }
}))

vi.mock('react-toastify', () => ({
  toast: {
    error: vi.fn()
  }
}))

import axios from 'axios'

describe('ActualPrice component', () => {
  it('renders current price data', async () => {
    axios.get.mockResolvedValue({
      data: {
        symbol: 'BTCUSDC',
        bid: '89245.00',
        ask: '89246.00',
        last: '89245.50',
        timestamp: 1700000000
      }
    })

    render(
      <AuthContext.Provider value={{ authTokens: { access: 'token' } }}>
        <ActualPrice />
      </AuthContext.Provider>
    )

    await waitFor(() => {
      expect(screen.getByText('$89,245.50')).toBeTruthy()
    })
  })
})
