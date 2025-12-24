import React from 'react'
import { Container, Row, Col, Card, Button } from 'react-bootstrap'
import { LinkContainer } from 'react-router-bootstrap'
import Header from "../components/common/Header"
import Footer from "../components/common/Footer"

function CareersScreen() {
  return (
    <div className="d-flex flex-column min-vh-100">
      <Header />
      <main className="flex-grow-1">
        <section className="py-5">
          <Container>
            <div className="text-center mb-5">
              <h1 className="fw-bold mb-3">Join Us</h1>
              <p className="text-secondary lead">Help us build the financial infrastructure of tomorrow.</p>
            </div>

            <Row className="justify-content-center">
              <Col lg={8}>
                <Card className="card-premium border-0 mb-4">
                  <Card.Body className="p-4 d-flex justify-content-between align-items-center flex-wrap gap-3">
                    <div>
                      <h4 className="fw-bold mb-1">Senior Python Engineer</h4>
                      <span className="text-secondary me-3">Remote</span>
                      <span className="text-secondary">Full-time</span>
                    </div>
                    <LinkContainer to="/contact">
                      <Button variant="outline-primary" className="rounded-pill px-4">Apply Now</Button>
                    </LinkContainer>
                  </Card.Body>
                </Card>

                <Card className="card-premium border-0 mb-4">
                  <Card.Body className="p-4 d-flex justify-content-between align-items-center flex-wrap gap-3">
                    <div>
                      <h4 className="fw-bold mb-1">Frontend Developer (React)</h4>
                      <span className="text-secondary me-3">Remote</span>
                      <span className="text-secondary">Contract</span>
                    </div>
                    <LinkContainer to="/contact">
                      <Button variant="outline-primary" className="rounded-pill px-4">Apply Now</Button>
                    </LinkContainer>
                  </Card.Body>
                </Card>

                <div className="text-center mt-5">
                  <p className="text-secondary">Don't see a perfect fit? Send us your resume anyway.</p>
                  <LinkContainer to="/contact">
                    <Button variant="link" className="text-primary fw-bold">Contact Recruiting</Button>
                  </LinkContainer>
                </div>
              </Col>
            </Row>
          </Container>
        </section>
      </main>
      <Footer />
    </div>
  )
}

export default CareersScreen
