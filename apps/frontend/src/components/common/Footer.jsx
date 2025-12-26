import React from 'react'
import { Container, Row, Col } from 'react-bootstrap'
import { LinkContainer } from 'react-router-bootstrap'

function Footer() {
  return (
    <footer className="py-5 bg-dark border-top border-secondary mt-auto">
      <Container>
        <Row className="gy-4">
          <Col md={4}>
            <h5 className="text-gradient fw-bold mb-3">Robson</h5>
            <p className="text-secondary">
              Just another crypto robot for intelligent trading strategies and risk management.
            </p>
          </Col>
          <Col md={2}>
            <h6 className="text-light mb-3">Product</h6>
            <ul className="list-unstyled">
              <li className="mb-2"><LinkContainer to="/features"><a href="#" className="text-secondary text-decoration-none">Features</a></LinkContainer></li>
              <li className="mb-2"><LinkContainer to="/pricing"><a href="#" className="text-secondary text-decoration-none">Pricing</a></LinkContainer></li>
              <li className="mb-2"><LinkContainer to="/demo"><a href="#" className="text-secondary text-decoration-none">Demo</a></LinkContainer></li>
            </ul>
          </Col>
          <Col md={2}>
            <h6 className="text-light mb-3">Company</h6>
            <ul className="list-unstyled">
              <li className="mb-2"><LinkContainer to="/about-us"><a href="#" className="text-secondary text-decoration-none">About Us</a></LinkContainer></li>
              <li className="mb-2"><LinkContainer to="/careers"><a href="#" className="text-secondary text-decoration-none">Careers</a></LinkContainer></li>
              <li className="mb-2"><LinkContainer to="/contact"><a href="#" className="text-secondary text-decoration-none">Contact</a></LinkContainer></li>
            </ul>
          </Col>
          <Col md={4}>
            <h6 className="text-light mb-3">Legal</h6>
            <div className="d-flex flex-column text-secondary small">
              <span>&copy; {new Date().getFullYear()} RBX Robótica. All rights reserved.</span>
              <span className="mt-1">Designed by RBX Robótica</span>
              {import.meta.env.VITE_APP_VERSION && (
                <span className="mt-2 opacity-50 x-small" style={{ fontSize: '0.65rem' }}>
                  Build: {import.meta.env.VITE_APP_VERSION}
                </span>
              )}
            </div>
          </Col>
        </Row>
      </Container>
    </footer>
  )
}

export default Footer
