import React from 'react'
import { Container, Row, Col, Card, Button, Badge } from 'react-bootstrap'
import { LinkContainer } from 'react-router-bootstrap'
import Header from "../components/common/Header"
import Footer from "../components/common/Footer"

function PricingScreen() {
  const plans = [
    {
      name: "Community",
      price: "Free",
      description: "Essential tools for hobbyist traders.",
      features: ["Manual Trading", "Basic Charts", "1% Risk Calculator", "Community Support"],
      variant: "outline-light",
      btnText: "Download Now",
      link: "/download"
    },
    {
      name: "Pro",
      price: "$29",
      period: "/month",
      description: "Advanced automation for serious traders.",
      features: ["All Free Features", "Unlimited Strategies", "Automated Execution", "Priority Support", "Advanced Analytics"],
      variant: "primary",
      btnText: "Start Free Trial",
      link: "/signup",
      popular: true
    },
    {
      name: "Enterprise",
      price: "Custom",
      description: "Scalable infrastructure for funds.",
      features: ["Multi-Tenant Support", "Custom Integrations", "SLA Guarantee", "Dedicated Account Manager"],
      variant: "outline-light",
      btnText: "Contact Sales",
      link: "/contact"
    }
  ]

  return (
    <div className="d-flex flex-column min-vh-100">
      <Header />
      <main className="flex-grow-1">
        <section className="py-5 text-center">
          <Container>
            <h1 className="fw-bold mb-3">Simple, Transparent Pricing</h1>
            <p className="text-secondary lead mb-5">Choose the plan that fits your trading style.</p>
            <Row className="justify-content-center g-4">
              {plans.map((plan, idx) => (
                <Col key={idx} md={4}>
                  <Card className={`h-100 card-premium ${plan.popular ? 'border-primary' : ''} p-2`}>
                    <Card.Body className="d-flex flex-column">
                      {plan.popular && <Badge bg="primary" className="align-self-end mb-2">Most Popular</Badge>}
                      <h3 className="fw-bold">{plan.name}</h3>
                      <div className="my-3">
                        <span className="display-4 fw-bold">{plan.price}</span>
                        {plan.period && <span className="text-secondary">{plan.period}</span>}
                      </div>
                      <p className="text-secondary">{plan.description}</p>
                      <hr className="border-secondary my-4" />
                      <ul className="list-unstyled mb-4 text-start flex-grow-1">
                        {plan.features.map((feature, i) => (
                          <li key={i} className="mb-2">
                            <span className="text-success me-2">✓</span> {feature}
                          </li>
                        ))}
                      </ul>
                      <LinkContainer to={plan.link}>
                        <Button variant={plan.variant} size="lg" className="w-100 rounded-pill">
                          {plan.btnText}
                        </Button>
                      </LinkContainer>
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

export default PricingScreen
