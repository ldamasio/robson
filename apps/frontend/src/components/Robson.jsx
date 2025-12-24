import React from 'react'
import { Container, Row, Col } from 'react-bootstrap'

function Robson() {
  return (
    <section>
      <Container>
        <Row>
          <Col className="text-center py-3">
            <img src="./rbs.png" className="photo" alt="Robson Bot" />
            <h1>Just another crypto robot</h1>
          </Col>
        </Row>
      </Container>
    </section>
  )
}

export default Robson
