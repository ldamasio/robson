import React, { useState } from 'react';
import Button from 'react-bootstrap/Button';
import Col from 'react-bootstrap/Col';
import Container from 'react-bootstrap/Container';
import Modal from 'react-bootstrap/Modal';
import Row from 'react-bootstrap/Row';
import Form from 'react-bootstrap/Form';

function StartNewOperationModal(props) {
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
              <Form.Select size="md">
                <option>Strategy 0001</option>
              </Form.Select>
              <br />
            </Col>
            <Col xs={6} md={4}>
            <label>Pair</label>
              <Form.Select size="md">
                <option>BTC/USDT</option>
              </Form.Select>
              <br />
            </Col>
          </Row>

          <Row>
            <Col xs={6} md={4}>
              Stop Loss
            </Col>
            <Col xs={6} md={4}>
              Stop Gain
            </Col>
            <Col xs={6} md={4}>
              Position Size
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