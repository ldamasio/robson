import React from 'react'
import { Container, Row, Col, Card } from 'react-bootstrap'
import Header from "../components/common/Header"
import Footer from "../components/common/Footer"

function OurTeamScreen() {
  const team = [
    { name: "Leandro Damasio", role: "Founder & Lead Engineer", desc: "Expert in distributed systems and crypto market mechanics." },
    { name: "Deepmind Team", role: "AI Architects", desc: "Advanced AI agents building the future of autonomous coding." },
  ]

  return (
    <div className="d-flex flex-column min-vh-100">
      <Header />
      <main className="flex-grow-1">
        <section className="py-5 text-center">
          <Container>
            <h1 className="fw-bold mb-3">Meet the Team</h1>
            <p className="text-secondary lead mb-5">The minds behind the machines.</p>
            <Row className="justify-content-center g-4">
              {team.map((member, idx) => (
                <Col key={idx} md={4}>
                  <Card className="h-100 card-premium border-0 shadow">
                    <Card.Body className="p-4">
                      <div className="mb-3 rounded-circle bg-dark d-inline-flex align-items-center justify-content-center border border-secondary" style={{ width: '80px', height: '80px' }}>
                        <span className="fs-2">👤</span>
                      </div>
                      <h4 className="fw-bold">{member.name}</h4>
                      <p className="text-primary fw-bold mb-3">{member.role}</p>
                      <p className="text-secondary">{member.desc}</p>
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

export default OurTeamScreen
