import React from 'react'
import { Container, Row, Col } from 'react-bootstrap'
import Header from "../components/common/Header"
import Footer from "../components/common/Footer"
import Robson from "../components/Robson"
import Download from "../components/Download"
import Hire from "../components/Hire"
import Demo from "../components/Demo"

function HomeScreen() {
  return(
    <div>
      <Header />
      <main className="py-5">
        <Container fluid="md">
          <Row>
            <Col>
              <Robson />
            </Col>
            <Col>
              <Download />
              <Hire />
              <Demo />
            </Col>
          </Row>
        </Container> 
      </main>
      <Footer />
    </div>
  )
}

export default HomeScreen
