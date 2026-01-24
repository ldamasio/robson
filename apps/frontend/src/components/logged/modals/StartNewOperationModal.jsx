import React, { useState, useContext } from "react";
import Button from "react-bootstrap/Button";
import Col from "react-bootstrap/Col";
import Container from "react-bootstrap/Container";
import Modal from "react-bootstrap/Modal";
import Row from "react-bootstrap/Row";
import Card from "react-bootstrap/Card";
import axios from "axios";
import { toast } from "react-toastify";
import AuthContext from "../../../context/AuthContext";

function StartNewOperationModal(props) {
  const { authTokens } = useContext(AuthContext);
  const [loading, setLoading] = useState(false);

  const createOperation = async (strategyName) => {
    setLoading(true);
    try {
      const BACKEND_URL = import.meta.env.VITE_API_BASE_URL;
      const response = await axios.post(
        `${BACKEND_URL}/api/operations/`,
        {
          strategy_name: strategyName,
          symbol: "BTCUSDC",
          account_type: "ISOLATED_MARGIN",
        },
        {
          headers: {
            Authorization: `Bearer ${authTokens?.access}`,
          },
        },
      );

      toast.success(`${strategyName} operation created successfully!`);
      props.onHide();

      // Refresh the page or trigger a re-fetch of positions
      window.location.reload();
    } catch (error) {
      console.error("Error creating operation:", error);
      const errorMsg =
        error.response?.data?.detail ||
        error.response?.data?.message ||
        "Error creating operation";
      toast.error(errorMsg);
    } finally {
      setLoading(false);
    }
  };

  return (
    <Modal {...props} aria-labelledby="contained-modal-title-vcenter" size="lg">
      <Modal.Header closeButton>
        <Modal.Title id="contained-modal-title-vcenter">
          New BTC/USDC Operation
        </Modal.Title>
      </Modal.Header>
      <Modal.Body>
        <Container>
          <p className="text-center mb-4">
            Do you think BTC price will go <strong>up</strong> or{" "}
            <strong>down</strong>?
          </p>

          <Row className="g-4">
            <Col md={6}>
              <Card
                className="text-center p-4 h-100 cursor-pointer"
                style={{ cursor: "pointer", transition: "transform 0.2s" }}
                onClick={() => !loading && createOperation("BTC Long")}
                onMouseEnter={(e) =>
                  (e.currentTarget.style.transform = "scale(1.05)")
                }
                onMouseLeave={(e) =>
                  (e.currentTarget.style.transform = "scale(1)")
                }
              >
                <Card.Body>
                  <div className="display-3 mb-3">📈</div>
                  <h3 className="text-success">LONG</h3>
                  <p className="text-muted">
                    Bet that it will go <strong>UP</strong>
                  </p>
                  <small className="text-secondary">
                    Buy BTC now to sell later at a higher price
                  </small>
                </Card.Body>
              </Card>
            </Col>
            <Col md={6}>
              <Card
                className="text-center p-4 h-100 cursor-pointer"
                style={{ cursor: "pointer", transition: "transform 0.2s" }}
                onClick={() => !loading && createOperation("BTC Short")}
                onMouseEnter={(e) =>
                  (e.currentTarget.style.transform = "scale(1.05)")
                }
                onMouseLeave={(e) =>
                  (e.currentTarget.style.transform = "scale(1)")
                }
              >
                <Card.Body>
                  <div className="display-3 mb-3">📉</div>
                  <h3 className="text-danger">SHORT</h3>
                  <p className="text-muted">
                    Bet that it will go <strong>DOWN</strong>
                  </p>
                  <small className="text-secondary">
                    Sell BTC now to buy back later at a lower price
                  </small>
                </Card.Body>
              </Card>
            </Col>
          </Row>

          {loading && (
            <div className="text-center mt-4">
              <div className="spinner-border text-primary" role="status">
                <span className="visually-hidden">Loading...</span>
              </div>
              <p className="text-muted mt-2">Creating operation...</p>
            </div>
          )}
        </Container>
      </Modal.Body>
      <Modal.Footer>
        <Button variant="secondary" onClick={props.onHide} disabled={loading}>
          Cancel
        </Button>
      </Modal.Footer>
    </Modal>
  );
}

export default StartNewOperationModal;
