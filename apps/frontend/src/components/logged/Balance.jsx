import React, { useState, useEffect, useContext } from "react";
import { Card, Row, Col, Badge } from "react-bootstrap";
import { Wallet2 } from "react-bootstrap-icons";
import axios from "axios";
import AuthContext from "../../context/AuthContext";
import LoadingSpinner from "../common/LoadingSpinner";

function Balance() {
  const { authTokens } = useContext(AuthContext);
  const [loading, setLoading] = useState(true);
  const [marginData, setMarginData] = useState(null);
  const [error, setError] = useState(null);

  useEffect(() => {
    const fetchMarginAccount = async () => {
      setLoading(true);
      try {
        const BACKEND_URL = import.meta.env.VITE_API_BASE_URL;
        const symbol = "BTCUSDC";
        const { data } = await axios.get(
          `${BACKEND_URL}/api/margin/account/${symbol}/`,
          {
            headers: {
              Authorization: `Bearer ${authTokens?.access}`,
            },
          },
        );
        setMarginData(data);
        setError(null);
      } catch (error) {
        console.error("Error fetching margin account:", error);
        setError("Error loading margin account");
        setMarginData(null);
      } finally {
        setLoading(false);
      }
    };

    if (authTokens?.access) {
      fetchMarginAccount();
    }
  }, [authTokens?.access]);

  if (loading) {
    return <LoadingSpinner label="Loading balance..." />;
  }

  if (error) {
    return (
      <Card className="card-premium">
        <Card.Body>
          <h5 className="text-light mb-3">Isolated Margin BTC/USDC</h5>
          <div className="text-danger">{error}</div>
        </Card.Body>
      </Card>
    );
  }

  return (
    <Card className="card-premium border-0 shadow-lg overflow-hidden">
      {/* Decorative top bar */}
      <div className="bg-primary" style={{ height: '4px', opacity: 0.6 }}></div>

      <Card.Body className="p-4">
        <div className="d-flex align-items-center justify-content-between mb-4">
          <div className="d-flex align-items-center">
            <div className="bg-primary bg-opacity-10 p-2 rounded-circle me-3">
              <Wallet2 className="text-primary" size={24} />
            </div>
            <h5 className="text-light mb-0 fw-bold">Investment Wallet</h5>
          </div>
          <Badge bg="primary" className="bg-opacity-10 text-primary border border-primary border-opacity-25 py-2 px-3 fw-normal">
            Isolated Margin
          </Badge>
        </div>

        {marginData ? (
          <>
            <div className="mb-4 bg-glass p-3 rounded-3 border border-white border-opacity-5">
              <small className="text-secondary d-block mb-1 text-uppercase letter-spacing-1">Estimated Net Equity</small>
              <div className="d-flex align-items-baseline">
                <span className="h2 fw-bold text-success mb-0 font-monospace">
                  {marginData.net_equity_btc || "0.00000000"}
                </span>
                <span className="ms-2 text-secondary fw-bold">BTC</span>
              </div>
              <small className="text-muted d-block mt-2">
                <i className="bi bi-info-circle me-1"></i>
                Simulation of closing all positions at market price
              </small>
            </div>
          </>
        ) : (
          <div className="text-muted text-center py-4">
            <LoadingSpinner label="Syncing with Binance..." />
          </div>
        )}
      </Card.Body>
    </Card>
  );
}

export default Balance;
