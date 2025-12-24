import React from 'react'
import { Container, Row, Col, Card } from 'react-bootstrap'
import Header from "../components/common/Header"
import Footer from "../components/common/Footer"

function AboutUsScreen() {
  return (
    <div className="d-flex flex-column min-vh-100">
      <Header />
      <main className="flex-grow-1">
        <section className="py-5 bg-dark">
          <Container>
            <Row className="align-items-center mb-5">
              <Col lg={6}>
                <h1 className="fw-bold display-4 mb-4">About <span className="text-gradient">RBX Robótica</span></h1>
                <p className="lead text-secondary mb-4">
                  We are a technology company specialized in developing advanced solutions for the financial market.
                  Our mission is to democratize access to professional-grade algorithmic trading tools.
                </p>
                <div className="d-flex gap-4">
                  <div>
                    <h2 className="fw-bold text-primary">5+</h2>
                    <p className="text-secondary">Years Experience</p>
                  </div>
                  <div>
                    <h2 className="fw-bold text-primary">100+</h2>
                    <p className="text-secondary">Strategies Tested</p>
                  </div>
                  <div>
                    <h2 className="fw-bold text-primary">24/7</h2>
                    <p className="text-secondary">System Uptime</p>
                  </div>
                </div>
              </Col>
              <Col lg={6}>
                <Card className="card-premium border-0 shadow-lg p-4 bg-glass mt-4 mt-lg-0">
                  <Card.Body>
                    <h3 className="fw-bold mb-3">Our Vision</h3>
                    <p className="text-secondary mb-0">
                      To build the most reliable and transparent automated trading infrastructure, empowering individuals to reclaim their time while their capital works efficiently.
                    </p>
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

export default AboutUsScreen
