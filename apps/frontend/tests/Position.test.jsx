// @vitest-environment jsdom
import React from 'react'
import { render, screen, waitFor } from '@testing-library/react'
import { vi } from 'vitest'
import Position from '../src/components/logged/Position'
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

describe('Position component', () => {
  it('renders active positions from the API', async () => {
    axios.get.mockResolvedValue({
      data: {
        positions: [
          {
            operation_id: 1,
            symbol: 'BTCUSDC',
            side: 'BUY',
            quantity: '0.00033',
            entry_price: '88837.92',
            current_price: '89245.50',
            unrealized_pnl: '134.50',
            unrealized_pnl_percent: '0.46',
            stop_loss: '87061.16',
            take_profit: '92391.44',
            distance_to_stop_percent: '-15.3',
            distance_to_target_percent: '3.5'
          }
        ]
      }
    })

    render(
      <AuthContext.Provider value={{ authTokens: { access: 'token' } }}>
        <Position />
      </AuthContext.Provider>
    )

    await waitFor(() => {
      expect(screen.getByText('BTCUSDC')).toBeTruthy()
    })
  })
})
