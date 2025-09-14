import React from 'react'
import { Container, Row, Col } from 'react-bootstrap'
import Header from "./common/Header"
import Footer from "./common/Footer"

function Contact() {
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

export default Contact
