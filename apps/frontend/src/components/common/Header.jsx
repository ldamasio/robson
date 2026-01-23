import React, { useContext } from 'react'
import { Container, Nav, Navbar, NavDropdown, Button } from 'react-bootstrap'
import { LinkContainer } from 'react-router-bootstrap'
import AuthContext from '../../context/AuthContext'

function Header() {
  let { user, logoutUser } = useContext(AuthContext)

  return (
    <header className="sticky-top">
      <Navbar collapseOnSelect expand="lg" variant="dark" className="bg-glass py-3">
        <Container>
          <LinkContainer to="/">
            <Navbar.Brand className="fw-bold fs-4">
              <span className="text-gradient">Robson</span>
            </Navbar.Brand>
          </LinkContainer>
          <Navbar.Toggle aria-controls="responsive-navbar-nav" className="border-0" />
          <Navbar.Collapse id="responsive-navbar-nav">
            <Nav className="align-items-center gap-3">
              {user ? (
                <>
                  <span className="text-light">Hi, {user.username}</span>
                  <LinkContainer to="/dashboard">
                    <Button variant="outline-light" size="sm">Dashboard</Button>
                  </LinkContainer>
                  <LinkContainer to="/patterns">
                    <Button variant="outline-info" size="sm">🎯 Opportunity Detector</Button>
                  </LinkContainer>
                  <Button variant="link" className="text-decoration-none text-light" onClick={logoutUser}>Logout</Button>
                </>
              ) : (
                <>
                  <LinkContainer to="/login">
                    <Nav.Link>Login</Nav.Link>
                  </LinkContainer>
                  <LinkContainer to="/signup">
                    <Button variant="primary" className="btn-primary rounded-pill px-4">Sign Up</Button>
                  </LinkContainer>
                </>
              )}
            </Nav>
          </Navbar.Collapse>
        </Container>
      </Navbar>
    </header>
  )
}

export default Header;