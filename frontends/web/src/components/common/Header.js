import React from 'react'
import { Container, Nav, Navbar, NavDropdown } from 'react-bootstrap'
import { LinkContainer } from 'react-router-bootstrap'

function Header() {
  return(
    <header>
      <Navbar collapseOnSelect expand="lg" bg="dark" variant="dark">
        <Container>
          <LinkContainer to ="/">
            <Navbar.Brand>
              Robson
            </Navbar.Brand>
          </LinkContainer>
          <Navbar.Toggle aria-controls="responsive-navbar-nav" />
          <Navbar.Collapse id="responsive-navbar-nav">
            <Nav className="justify-content-end" style={{ width: "100%", marginRight: "40px" }}>
              <Nav.Link href="/features">Features</Nav.Link>
              <Nav.Link href="/pricing">Pricing</Nav.Link>
              <NavDropdown title="Company" id="collasible-nav-dropdown">
                <NavDropdown.Item href="/about-us">About Us</NavDropdown.Item>
                <NavDropdown.Item href="/our-team">
                  Our Team
                </NavDropdown.Item>
                <NavDropdown.Item href="/careers">Careers</NavDropdown.Item>
                <NavDropdown.Divider />
                <NavDropdown.Item href="/contact">
                  Contact
                </NavDropdown.Item>
              </NavDropdown>
            </Nav>
            <Nav className="text-nowrap">
              <Nav.Link href="/signup">Sign up</Nav.Link>
              <Nav.Link eventKey={2} href="/login">
                Login
              </Nav.Link>
            </Nav>
          </Navbar.Collapse>
        </Container>
      </Navbar>
    </header>
  )
}

export default Header
