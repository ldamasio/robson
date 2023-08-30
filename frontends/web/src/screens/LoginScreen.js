import React, { useContext } from 'react'
import AuthContext from 'context/AuthContext'
// import { Link, useHistory } from 'react-router-dom';
import { Container, Row, Col } from 'react-bootstrap'
import Header from "../components/common/Header"
import Footer from "../components/common/Footer"

const LoginScreen = () => {
  let { loginUser } = useContext(AuthContext)
  return (
    <div>
      <Header />
      <main className="py-5">
        <Container fluid="md">
          <form onSubmit={loginUser}>
            <input type="text" name="username" placeholder="Enter Username" />
            <input type="password" name="password" placeholder="Enter Password" />
            <input type="submit" />
          </form>
          <Row className="justify-content-md-center">
            <Col md="auto">
              {/* <Link to="/" classname="justify-content-center">
                <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 448 512">
                  <path d="M224 256A128 128 0 1 0 224 0a128 128 0 1 0 0 256zm-45.7 48C79.8 304 0 383.8 0 482.3C0 498.7 13.3 512 29.7 512H418.3c16.4 0 29.7-13.3 29.7-29.7C448 383.8 368.2 304 269.7 304H178.3z"/>
                </svg>
              </Link> */}
              teste
            </Col>
          </Row>
        </Container>
      </main>
      <Footer />
    </div>
  )
}

export default LoginScreen
