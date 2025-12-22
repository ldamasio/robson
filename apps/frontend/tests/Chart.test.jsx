// @vitest-environment jsdom
import React from 'react'
import { render, waitFor } from '@testing-library/react'
import { vi } from 'vitest'
import Chart from '../src/components/logged/Chart'
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

describe('Chart component', () => {
  it('renders candlestick chart with data', async () => {
    const candlePayload = JSON.stringify([
      {
        Date: '2024-01-01T00:00:00.000Z',
        Open: '100',
        High: '110',
        Low: '90',
        Close: '105',
        Volume: '1'
      },
      {
        Date: '2024-01-01T00:15:00.000Z',
        Open: '105',
        High: '112',
        Low: '101',
        Close: '108',
        Volume: '1'
      }
    ])

    axios.get.mockImplementation((url) => {
      if (url.includes('/api/historical-data/')) {
        return Promise.resolve({ data: { data: candlePayload } })
      }
      if (url.includes('/api/portfolio/positions/')) {
        return Promise.resolve({
          data: {
            positions: [
              {
                entry_price: '100',
                stop_loss: '95',
                take_profit: '120'
              }
            ]
          }
        })
      }
      return Promise.reject(new Error('Unexpected URL'))
    })

    const { container } = render(
      <AuthContext.Provider value={{ authTokens: { access: 'token' } }}>
        <Chart />
      </AuthContext.Provider>
    )

    await waitFor(() => {
      expect(container.querySelector('svg')).toBeTruthy()
    })
  })
})
