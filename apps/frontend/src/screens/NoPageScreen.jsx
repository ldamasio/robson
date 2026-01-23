import React from 'react'
import { Container, Row, Col, Button } from 'react-bootstrap'
import { LinkContainer } from 'react-router-bootstrap'
import Header from "../components/common/Header"
import Footer from "../components/common/Footer"

function NoPageScreen() {
  return (
    <div className="d-flex flex-column min-vh-100">
      <Header />
      <main className="py-5 my-auto">
        <Container>
          <Row className="justify-content-center text-center">
            <Col md={8} lg={6}>
              <h1 className="display-1 fw-bold text-gradient mb-4">404</h1>
              <h2 className="mb-4 text-light">Page Not Found</h2>
              <p className="text-secondary mb-5">
                The page you are looking for doesn't exist or has been moved as part of our platform simplification.
              </p>
              <LinkContainer to="/">
                <Button variant="primary" size="lg" className="rounded-pill px-5">
                  Back to Home
                </Button>
              </LinkContainer>
            </Col>
          </Row>
        </Container>
      </main>
      <Footer />
    </div>
  )
}

export default NoPageScreen
