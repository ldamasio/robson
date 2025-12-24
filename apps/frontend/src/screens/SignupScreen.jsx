import React from 'react'
import { Container, Row, Col, Form, Button, Card } from 'react-bootstrap'
import { Link } from 'react-router-dom'
import Header from "../components/common/Header"
import Footer from "../components/common/Footer"

const SignupScreen = () => {
  return (
    <div className="d-flex flex-column min-vh-100">
      <Header />
      <main className="d-flex align-items-center py-5">
        <Container>
          <Row className="justify-content-center">
            <Col xs={12} md={6} lg={5} xl={4}>
              <Card className="card-premium shadow-lg border-0">
                <Card.Body className="p-5">
                  <div className="text-center mb-4">
                    <h2 className="fw-bold mb-2">Create Account</h2>
                    <p className="text-secondary">Join Robson and start trading smarter</p>
                  </div>
                  <Form>
                    <Form.Group className="mb-3" controlId="formName">
                      <Form.Label className="text-secondary">Full Name</Form.Label>
                      <Form.Control
                        type="text"
                        placeholder="John Doe"
                        required
                        className="bg-dark text-light border-secondary"
                      />
                    </Form.Group>

                    <Form.Group className="mb-3" controlId="formEmail">
                      <Form.Label className="text-secondary">Email address</Form.Label>
                      <Form.Control
                        type="email"
                        placeholder="john@example.com"
                        required
                        className="bg-dark text-light border-secondary"
                      />
                    </Form.Group>

                    <Form.Group className="mb-3" controlId="formUsername">
                      <Form.Label className="text-secondary">Username</Form.Label>
                      <Form.Control
                        type="text"
                        placeholder="Choose a username"
                        required
                        className="bg-dark text-light border-secondary"
                      />
                    </Form.Group>

                    <Form.Group className="mb-4" controlId="formPassword">
                      <Form.Label className="text-secondary">Password</Form.Label>
                      <Form.Control
                        type="password"
                        placeholder="Create a strong password"
                        required
                        className="bg-dark text-light border-secondary"
                      />
                    </Form.Group>

                    <div className="d-grid gap-2 mb-4">
                      <Button variant="primary" type="submit" size="lg">
                        Sign Up
                      </Button>
                    </div>

                    <div className="text-center text-secondary">
                      Already have an account? <Link to="/login" className="text-primary fw-bold">Sign In</Link>
                    </div>
                  </Form>
                </Card.Body>
              </Card>
            </Col>
          </Row>
        </Container>
      </main>
      <Footer />
    </div>
  )
}

export default SignupScreen
