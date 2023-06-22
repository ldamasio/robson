import React from 'react'
import { Container, Row, Col } from 'react-bootstrap'
import Header from "../components/common/Header"
import Footer from "../components/common/Footer"

function HireScreen() {
  return(
    <div>
      <Header />
      <main className="py-5">
        <Container fluid="md">
          <Row>
            <Col>
              teste
            </Col>
          </Row>
        </Container> 
      </main>
      <Footer />
    </div>
  )
}

export default HireScreen
