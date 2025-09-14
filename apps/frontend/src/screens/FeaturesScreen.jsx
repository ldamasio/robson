import React from 'react'
import { Container, Row, Col } from 'react-bootstrap'
import Header from "../components/common/Header"
import Footer from "../components/common/Footer"

function FeaturesScreen() {

  const secure = [
  'Robson adopts several layers of security',
  'All data is protected with encryption',
  'All backend servers are secured with Json Web Token',
  'Websockets servers are also secured with Json Web Token'
  ];

  const transparent = [
  'Robson is auditable',
  'Robson\'s operational setup is different from the human being',
  'Robson calculates short-term trend'
  ];
  
  const inteligent = [
  'Robson consumes data in real time',
  'Docker-compose',
  ];

  const disciplined = [
  'Robson does not pay to see',
  'Technical stop on each trade',
  ];

  return(
    <div>
      <Header />
      <main className="py-5">
        <Container fluid="md">
          <Row>
            <Col>
              <h2>Features</h2>
              <p>
                <h4>Secure</h4>
                <ul>
                  { secure.map( (secure_item) => <li>{ secure_item }</li>) }
                </ul>
              </p>
              <p>
                <h4>Tranparent</h4>
                <ul>
                  { transparent.map( (transparent_item) => <li>{ transparent_item }</li>) }
                </ul>
              </p>
              <p>
                <h4>Inteligent</h4>
                <ul>
                  { inteligent.map( (inteligent_item) => <li>{ inteligent_item }</li>) }
                </ul>
              </p>               <p>
                <h4>Web Development</h4>
                <ul>
                  { disciplined.map( (disciplined_item) => <li>{ disciplined_item }</li>) }
                </ul>
              </p>             
            </Col>
          </Row>
        </Container> 
      </main>
      <Footer />
    </div>
  )
}

export default FeaturesScreen
