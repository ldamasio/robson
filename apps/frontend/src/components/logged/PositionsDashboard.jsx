import React, { useState } from 'react'
import { Tab, Tabs } from 'react-bootstrap'
import Position from './Position'
import TradeHistory from './TradeHistory'

function PositionsDashboard() {
    const [key, setKey] = useState('active')

    return (
        <div className="positions-dashboard mb-5">
            <div className="d-flex align-items-center justify-content-between mb-4">
                <h2 className="text-gradient fw-bold mb-0">Portfolio Tracker</h2>
            </div>

            <Tabs
                id="positions-dashboard-tabs"
                activeKey={key}
                onSelect={(k) => setKey(k)}
                className="mb-4 custom-tabs"
            >
                <Tab eventKey="active" title="Active Positions">
                    {/* Wrapping Position in a container style if needed, 
               but Position.jsx already renders a list of cards. 
               We might want to add a header or summary here later. */}
                    <div className="row g-4">
                        <div className="col-12">
                            <Position />
                        </div>
                    </div>
                </Tab>
                <Tab eventKey="history" title="Trade History">
                    <TradeHistory />
                </Tab>
            </Tabs>

            <style jsx="true">{`
        .custom-tabs .nav-link {
          color: var(--bs-gray-400);
          border: none;
          font-weight: 500;
          padding: 1rem 1.5rem;
          transition: all 0.2s ease;
        }
        .custom-tabs .nav-link:hover {
          color: var(--bs-light);
          background: rgba(255, 255, 255, 0.05);
          border-radius: 0.5rem 0.5rem 0 0;
        }
        .custom-tabs .nav-link.active {
          color: var(--bs-white);
          background: transparent;
          border-bottom: 2px solid var(--primary-color);
        }
      `}</style>
        </div>
    )
}

export default PositionsDashboard
