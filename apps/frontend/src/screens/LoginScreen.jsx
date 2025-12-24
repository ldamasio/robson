import React, { useContext } from 'react'
import AuthContext from '../context/AuthContext'
import { Container, Row, Col, Form, Button, Card, Alert } from 'react-bootstrap'
import { Link } from 'react-router-dom'
import Header from "../components/common/Header"
import Footer from "../components/common/Footer"

const LoginScreen = () => {
  let { loginUser, error } = useContext(AuthContext)

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
                    <h2 className="fw-bold mb-2">Welcome Back</h2>
                    <p className="text-secondary">Sign in to your Robson account</p>
                  </div>

                  {error && (
                    <Alert variant="danger" className="mb-4">
                      {error}
                    </Alert>
                  )}

                  <Form onSubmit={loginUser}>
                    <Form.Group className="mb-3" controlId="formUsername">
                      <Form.Label className="text-secondary">Username</Form.Label>
                      <Form.Control
                        type="text"
                        name="username"
                        placeholder="Enter Username"
                        required
                        className="bg-dark text-light border-secondary"
                      />
                    </Form.Group>

                    <Form.Group className="mb-4" controlId="formPassword">
                      <Form.Label className="text-secondary">Password</Form.Label>
                      <Form.Control
                        type="password"
                        name="password"
                        placeholder="Enter Password"
                        required
                        className="bg-dark text-light border-secondary"
                      />
                    </Form.Group>

                    <div className="d-grid gap-2 mb-4">
                      <Button variant="primary" type="submit" size="lg">
                        Sign In
                      </Button>
                    </div>

                    <div className="text-center text-secondary">
                      Don't have an account? <Link to="/signup" className="text-primary fw-bold">Sign up</Link>
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

export default LoginScreen
