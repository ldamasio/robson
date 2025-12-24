import React from 'react'
import { Container, Row, Col, Form, Button, Card } from 'react-bootstrap'
import Header from "../components/common/Header"
import Footer from "../components/common/Footer"

function ContactScreen() {
  return (
    <div className="d-flex flex-column min-vh-100">
      <Header />
      <main className="flex-grow-1">
        <section className="py-5">
          <Container>
            <Row className="justify-content-center">
              <Col lg={8}>
                <Card className="card-premium border-0 shadow-lg">
                  <Card.Body className="p-5">
                    <h1 className="fw-bold text-center mb-5">Get in Touch</h1>
                    <Form>
                      <Row>
                        <Col md={6} className="mb-3">
                          <Form.Group controlId="formName">
                            <Form.Label className="text-secondary">Name</Form.Label>
                            <Form.Control type="text" placeholder="Your Name" className="bg-dark text-light border-secondary" />
                          </Form.Group>
                        </Col>
                        <Col md={6} className="mb-3">
                          <Form.Group controlId="formEmail">
                            <Form.Label className="text-secondary">Email</Form.Label>
                            <Form.Control type="email" placeholder="Your Email" className="bg-dark text-light border-secondary" />
                          </Form.Group>
                        </Col>
                      </Row>

                      <Form.Group className="mb-3" controlId="formSubject">
                        <Form.Label className="text-secondary">Subject</Form.Label>
                        <Form.Control type="text" placeholder="How can we help?" className="bg-dark text-light border-secondary" />
                      </Form.Group>

                      <Form.Group className="mb-4" controlId="formMessage">
                        <Form.Label className="text-secondary">Message</Form.Label>
                        <Form.Control as="textarea" rows={5} placeholder="Your message..." className="bg-dark text-light border-secondary" />
                      </Form.Group>

                      <div className="text-center">
                        <Button variant="primary" type="submit" size="lg" className="rounded-pill px-5">
                          Send Message
                        </Button>
                      </div>
                    </Form>
                  </Card.Body>
                </Card>
              </Col>
            </Row>
          </Container>
        </section>
      </main>
      <Footer />
    </div>
  )
}

export default ContactScreen
