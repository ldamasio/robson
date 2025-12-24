import React from 'react'
import { Container, Row, Col, Card } from 'react-bootstrap'
import Header from "../components/common/Header"
import Footer from "../components/common/Footer"

function FeaturesScreen() {
  const features = [
    {
      title: "Secure",
      icon: "🔒",
      items: [
        'Multi-layer security architecture',
        'End-to-end encryption for all data',
        'JWT Authentication for API & WebSockets',
        'Role-based access control (RBAC)'
      ]
    },
    {
      title: "Transparent",
      icon: "👁️",
      items: [
        'Fully auditable operations',
        'Open Source code available on GitHub',
        'Clear decision-making logs',
        'Real-time status updates'
      ]
    },
    {
      title: "Intelligent",
      icon: "🧠",
      items: [
        'Real-time market data analysis',
        'Automated trend calculation',
        'Smart position sizing (1% Rule)',
        'Docker-compose ready for easy deployment'
      ]
    },
    {
      title: "Disciplined",
      icon: "⚖️",
      items: [
        'Zero-emotion execution',
        'Automated Stop-Loss management',
        'Strict risk adherence',
        'Consistent strategy execution'
      ]
    }
  ]

  return (
    <div className="d-flex flex-column min-vh-100">
      <Header />
      <main className="flex-grow-1">
        <section className="py-5 bg-dark text-center">
          <Container>
            <h1 className="display-4 fw-bold mb-3">Power Under the Hood</h1>
            <p className="lead text-secondary mb-5 mx-auto" style={{ maxWidth: '800px' }}>
              Robson combines security, transparency, and intelligence to give you the trading edge.
            </p>
            <Row className="g-4">
              {features.map((section, idx) => (
                <Col key={idx} md={6}>
                  <Card className="h-100 card-premium border-0 shadow text-start">
                    <Card.Body className="p-4">
                      <div className="d-flex align-items-center mb-4">
                        <span className="fs-1 me-3">{section.icon}</span>
                        <h3 className="card-title fw-bold mb-0">{section.title}</h3>
                      </div>
                      <ul className="list-unstyled mb-0">
                        {section.items.map((item, i) => (
                          <li key={i} className="mb-3 d-flex align-items-start">
                            <span className="text-primary me-2">➜</span>
                            <span className="text-light">{item}</span>
                          </li>
                        ))}
                      </ul>
                    </Card.Body>
                  </Card>
                </Col>
              ))}
            </Row>
          </Container>
        </section>
      </main>
      <Footer />
    </div>
  )
}

export default FeaturesScreen
