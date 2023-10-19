import React from 'react'
import { Container, Row, Col, Button } from 'react-bootstrap'
import { LinkContainer } from 'react-router-bootstrap'

function Download() {
  return(
    <footer>
      <Container>
        <Row>
          <Col className="text-left py-3" xs={6}>
            <h4>Open source code for download.</h4>
          </Col>
          <Col className="text-right py-3" xs={6}>
            <LinkContainer to='/download'>
              <Button variant="primary" size="lg">
                Download
              </Button>
            </LinkContainer>
          </Col>
        </Row>
      </Container>
    </footer>
  )
}

export default Download
