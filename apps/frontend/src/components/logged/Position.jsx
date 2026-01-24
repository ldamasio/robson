import React, { useContext, useEffect, useState } from "react";
import { Badge, Card, Alert } from "react-bootstrap";
import axios from "axios";
import { toast } from "react-toastify";
import AuthContext from "../../context/AuthContext";
import LoadingSpinner from "../common/LoadingSpinner";

function Position() {
  const { authTokens } = useContext(AuthContext);
  const [positions, setPositions] = useState([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);

  const baseUrl = import.meta.env.VITE_API_BASE_URL || "";

  const formatCurrency = (value) => {
    const number = Number(value);
    if (Number.isNaN(number)) return value || "N/A";
    return new Intl.NumberFormat("en-US", {
      style: "currency",
      currency: "USD",
      minimumFractionDigits: 2,
    }).format(number);
  };

  const formatPercent = (value) => {
    const number = Number(value);
    if (Number.isNaN(number)) return value || "N/A";
    const sign = number > 0 ? "+" : "";
    return `${sign}${number.toFixed(2)}%`;
  };

  const fetchPositions = async () => {
    try {
      const response = await axios.get(`${baseUrl}/api/portfolio/positions/`, {
        headers: {
          Authorization: `Bearer ${authTokens?.access}`,
        },
      });
      // Sort by margin level (ascending - most risky first)
      const sortedPositions = (response.data.positions || []).sort((a, b) => {
        const aLevel = parseFloat(a.margin_level) || 999;
        const bLevel = parseFloat(b.margin_level) || 999;
        return aLevel - bLevel;
      });
      setPositions(sortedPositions);
      setError(null);
    } catch (err) {
      setError("Failed to load positions.");
      toast.error("Failed to load positions.");
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    let isActive = true;
    const loadPositions = async () => {
      if (!isActive) return;
      await fetchPositions();
    };

    loadPositions();
    return () => {
      isActive = false;
    };
  }, [authTokens?.access]);

  const handleRefresh = () => {
    setLoading(true);
    fetchPositions();
  };

  return (
    <div className="d-grid gap-3">
      <div className="d-flex justify-content-end mb-2">
        <button
          className="btn btn-sm btn-outline-primary"
          onClick={handleRefresh}
          disabled={loading}
        >
          {loading ? "Refreshing..." : "Refresh Positions"}
        </button>
      </div>

      {loading && <LoadingSpinner label="Loading positions..." />}

      {!loading && error && <div className="text-danger">{error}</div>}

      {!loading && !error && positions.length === 0 && (
        <div className="text-muted text-center py-4">No active positions.</div>
      )}

      {!loading &&
        !error &&
        positions.map((position) => {
          const pnl = Number(position.unrealized_pnl);
          const pnlPercent = Number(position.unrealized_pnl_percent);
          const pnlPositive = pnl > 0;
          const pnlBadge = pnlPositive
            ? "success"
            : pnl < 0
              ? "danger"
              : "secondary";
          const isLong = position.side === "BUY" || position.side === "LONG";
          const sideLabel = isLong ? "LONG" : "SHORT";
          const key = position.operation_id || position.id || position.symbol;

          const marginLevel = parseFloat(position.margin_level);
          const isHighRisk = marginLevel < 1.5;
          const isCriticalRisk = marginLevel < 1.2;

          return (
            <Card key={key} className="card-premium mb-3">
              <Card.Body>
                {/* Header: Symbol + Margin Level DESTACADO */}
                <div className="d-flex justify-content-between align-items-start mb-3">
                  <div>
                    <h5 className="mb-1 text-light fw-bold">
                      {position.symbol}
                      <Badge
                        bg={isLong ? "success" : "danger"}
                        className="ms-2"
                      >
                        {sideLabel}
                      </Badge>
                    </h5>
                  </div>
                  <div className="text-end">
                    <small className="text-secondary d-block">
                      Margin Level
                    </small>
                    <h3
                      className={`mb-0 fw-bold ${isCriticalRisk ? "text-danger" : isHighRisk ? "text-warning" : "text-success"}`}
                    >
                      {position.margin_level || "N/A"}
                    </h3>
                    {isCriticalRisk && (
                      <Badge bg="danger" className="mt-1">
                        CRITICAL RISK
                      </Badge>
                    )}
                    {isHighRisk && !isCriticalRisk && (
                      <Badge bg="warning" text="dark" className="mt-1">
                        HIGH RISK
                      </Badge>
                    )}
                  </div>
                </div>

                {/* Alert de risco se margin level baixo */}
                {isCriticalRisk && (
                  <Alert variant="danger" className="py-2 mb-3">
                    ⚠️ Critical margin level! Liquidation risk.
                  </Alert>
                )}

              </Card.Body>
            </Card>
          );
        })}
    </div>
  );
}

export default Position;
