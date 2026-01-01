import React, { useState, useEffect, useContext } from 'react';
import Button from 'react-bootstrap/Button';
import Col from 'react-bootstrap/Col';
import Container from 'react-bootstrap/Container';
import Modal from 'react-bootstrap/Modal';
import Row from 'react-bootstrap/Row';
import Form from 'react-bootstrap/Form';
import AuthContext from '../../../context/AuthContext';

function StartNewOperationModal(props) {
  const { authTokens } = useContext(AuthContext);
  const [strategies, setStrategies] = useState([]);
  const [selectedStrategy, setSelectedStrategy] = useState('');

  // Fetch strategies from backend
  useEffect(() => {
    const fetchStrategies = async () => {
      try {
        const response = await fetch(`${import.meta.env.VITE_API_BASE_URL}/api/strategies/`, {
          headers: {
            'Content-Type': 'application/json',
            'Authorization': `Bearer ${authTokens.access}`,
          },
        });
        if (response.ok) {
          const data = await response.json();
          setStrategies(data.results || data);
        }
      } catch (err) {
        console.error('Failed to fetch strategies:', err);
      }
    };
    if (authTokens?.access) {
      fetchStrategies();
    }
  }, [authTokens]);

  return (
    <Modal {...props} aria-labelledby="contained-modal-title-vcenter">
      <Modal.Header closeButton>
        <Modal.Title id="contained-modal-title-vcenter">
          Start New Operation
        </Modal.Title>
      </Modal.Header>
      <Modal.Body className="grid-example">
        <Container>
          <Row>
            <Col xs={12} md={8}>
              <label>Select Strategy</label>
              <Form.Select
                size="md"
                value={selectedStrategy}
                onChange={(e) => setSelectedStrategy(e.target.value)}
              >
                <option value="">Select a strategy...</option>
                {strategies.map((strategy) => (
                  <option key={strategy.id} value={strategy.id}>
                    {strategy.name}
                  </option>
                ))}
              </Form.Select>
            </Col>
            <Col xs={6} md={4}>
              <label>Trading Pair</label>
              <Form.Select size="md">
                <option>BTC/USDT</option>
              </Form.Select>
            </Col>
          </Row>

          <Row>
            <Col xs={6} md={4}>
              <Form.Check // prettier-ignore
                type="switch"
                id="custom-switch"
                label="Stop Loss 1% Capital"
              />
              <Form.Check // prettier-ignore
                type="switch"
                id="custom-switch"
                label="Stop Gain 15% Proft"
              />
            </Col>
            <Col xs={6} md={4}>
              <label>Time Frame</label>
              <Form.Select size="md">
                <option>1h</option>
              </Form.Select>
            </Col>
            <Col xs={6} md={4}>
              <label>Position Size</label>
              <Form.Select size="md">
                <option>10%</option>
              </Form.Select>
            </Col>
          </Row>
        </Container>
      </Modal.Body>
      <Modal.Footer>
        <Button onClick={props.onHide}>Start New Operation</Button>
      </Modal.Footer>
    </Modal>
  );
}

export default StartNewOperationModal