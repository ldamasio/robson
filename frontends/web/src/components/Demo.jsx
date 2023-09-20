import React from 'react'
import { Container, Row, Col, Button } from 'react-bootstrap'
import { LinkContainer } from 'react-router-bootstrap'

function Demo() {
  return(
    <footer>
      <Container>
        <Row>
          <Col className="text-right py-3" xs={6}>
            <h4>Try a free demo here</h4>
          </Col>
          <Col className="text-left py-3" xs={6}>
            <LinkContainer to='/demo'>
              <Button variant="info" size="lg">
                Try Demo
              </Button>
            </LinkContainer>
          </Col>
        </Row>
      </Container>
    </footer>
  )
}

export default Demo
