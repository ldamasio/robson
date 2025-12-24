import React from 'react'
import { Container, Row, Col, Button } from 'react-bootstrap'
import { LinkContainer } from 'react-router-bootstrap'

function Hire() {
  return (
    <section>
      <Container>
        <Row>
          <Col className="text-right py-3" xs={6}>
            <h4>Professional web platform plan</h4>
          </Col>
          <Col className="text-left py-3" xs={6}>
            <LinkContainer to='/hire'>
              <Button variant="danger" size="lg">
                Hire Now
              </Button>
            </LinkContainer>
          </Col>
        </Row>
      </Container>
    </section>
  )
}

export default Hire
