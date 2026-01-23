import React, { useState, useEffect, useContext } from "react";
import { Card } from "react-bootstrap";
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
        setError("Erro ao carregar conta margin");
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
    return <LoadingSpinner label="Carregando saldo..." />;
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
    <Card className="card-premium">
      <Card.Body>
        <h5 className="text-light mb-3">Isolated Margin BTC/USDC</h5>
        {marginData ? (
          <>
            <div className="mb-2">
              <small className="text-secondary">Total USDC:</small>
              <div className="h6 text-light">
                {marginData.totalUSDC || "N/A"}
              </div>
            </div>
            <div className="mb-2">
              <small className="text-secondary">USDC Disponível:</small>
              <div className="h6 text-light">
                {marginData.freeUSDC || "N/A"}
              </div>
            </div>
            <div className="mb-2">
              <small className="text-secondary">Margin Level:</small>
              <div className="h5 fw-bold text-warning">
                {marginData.marginLevel || "N/A"}
              </div>
            </div>
          </>
        ) : (
          <div className="text-muted">Nenhuma conta margin encontrada</div>
        )}
      </Card.Body>
    </Card>
  );
}

export default Balance;
