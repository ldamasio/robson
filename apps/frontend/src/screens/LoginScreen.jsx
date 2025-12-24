import React, { useContext } from 'react'
import AuthContext from '../context/AuthContext'
import { Container, Row, Col, Form, Button } from 'react-bootstrap'
import Header from "../components/common/Header"
import Footer from "../components/common/Footer"

const LoginScreen = () => {
  let { loginUser } = useContext(AuthContext)

  return (
    <div className="d-flex flex-column min-vh-100">
      <Header />
      <main className="flex-grow-1 py-5">
        <Container>
          <Row className="justify-content-md-center">
            <Col xs={12} md={6} lg={4}>
              <div className="shadow p-4 rounded bg-white">
                <h2 className="text-center mb-4">Login</h2>
                <Form onSubmit={loginUser}>
                  <Form.Group className="mb-3" controlId="formUsername">
                    <Form.Label>Username</Form.Label>
                    <Form.Control type="text" name="username" placeholder="Enter Username" required />
                  </Form.Group>

                  <Form.Group className="mb-3" controlId="formPassword">
                    <Form.Label>Password</Form.Label>
                    <Form.Control type="password" name="password" placeholder="Enter Password" required />
                  </Form.Group>

                  <div className="d-grid gap-2">
                    <Button variant="primary" type="submit" size="lg">
                      Sign In
                    </Button>
                  </div>
                </Form>
              </div>
            </Col>
          </Row>
        </Container>
      </main>
      <Footer />
    </div>
  )
}

export default LoginScreen
