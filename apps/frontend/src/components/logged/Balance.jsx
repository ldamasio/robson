import React, { useState, useEffect, useContext } from "react";
import { Card, Row, Col } from "react-bootstrap";
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
        <div className="d-flex align-items-center mb-4">
          <div className="bg-primary bg-opacity-10 p-2 rounded-3 me-3">
            <Wallet2 className="text-primary" size={24} />
          </div>
          <h5 className="text-light mb-0 fw-bold">Carteira de Investimento</h5>
        </div>

        {marginData ? (
          <>
            <div className="mb-4">
              <small className="text-secondary d-block mb-1">Total Estimado (BTC)</small>
              <div className="h3 fw-bold text-success mb-0 font-monospace">
                {marginData.net_equity_btc || "N/A"}
              </div>
              <small className="text-muted">Valor se todas as dívidas fossem pagas agora</small>
            </div>

            <hr className="border-secondary opacity-25 my-4" />

            <Row className="g-3">
              <Col xs={6}>
                <small className="text-secondary d-block">USDC Total</small>
                <div className="fw-bold text-light">
                  {marginData.totalUSDC || "N/A"}
                </div>
              </Col>
              <Col xs={6}>
                <small className="text-secondary d-block">Margem (ML)</small>
                <div className={`fw-bold ${parseFloat(marginData.marginLevel) < 1.3 ? 'text-danger' : 'text-warning'}`}>
                  {marginData.marginLevel || "N/A"}
                </div>
              </Col>
            </Row>
          </>
        ) : (
          <div className="text-muted">Nenhuma conta margin encontrada</div>
        )}
      </Card.Body>
    </Card>
  );
}

export default Balance;
