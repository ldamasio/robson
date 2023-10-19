import React from 'react'
import { Container, Row, Col } from 'react-bootstrap'

function Robson() {
  return(
    <footer>
      <Container>
        <Row>
          <Col className="text-center py-3">
            <img src="./rbs.png" className="photo" alt="Robson Bot" />
            <h1>Just another crypto robot</h1>
          </Col>
        </Row>
      </Container>
    </footer>
  )
}

export default Robson
