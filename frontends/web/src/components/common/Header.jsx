import React, { useContext } from 'react'
import { Container, Nav, Navbar, NavDropdown } from 'react-bootstrap'
import { LinkContainer } from 'react-router-bootstrap'
import AuthContext from '../../context/AuthContext'

function Header() {
  let { user, logoutUser } = useContext(AuthContext)
  return (
    <header>
      <Navbar collapseOnSelect expand="lg" bg="dark" variant="dark">
        <Container>
          <LinkContainer to="/">
            <Navbar.Brand>Robson</Navbar.Brand>
          </LinkContainer>
          <Navbar.Toggle aria-controls="responsive-navbar-nav" />
          <Navbar.Collapse id="responsive-navbar-nav">
            <Nav className="justify-content-end" style={{ width: "100%", marginRight: "40px" }}>
              <LinkContainer to="/features">
                <Nav.Link>Features</Nav.Link>
              </LinkContainer>
              <LinkContainer to="/pricing">
                <Nav.Link>Pricing</Nav.Link>
              </LinkContainer>
              <NavDropdown title="Company" id="collasible-nav-dropdown">
                <LinkContainer to="/about-us">
                  <NavDropdown.Item>About Us</NavDropdown.Item>
                </LinkContainer>
                <LinkContainer to="/our-team">
                  <NavDropdown.Item>Our Team</NavDropdown.Item>
                </LinkContainer>
                <LinkContainer to="/careers">
                  <NavDropdown.Item>Careers</NavDropdown.Item>
                </LinkContainer>
                <NavDropdown.Divider />
                <LinkContainer to="/contact">
                  <NavDropdown.Item>Contact</NavDropdown.Item>
                </LinkContainer>
              </NavDropdown>
            </Nav>
            <Nav className="text-nowrap">
              {user ? (
                <p onClick={logoutUser}>Logout</p>
              ) : (
                <LinkContainer to="/login">
                  <Nav.Link>Login</Nav.Link>
                </LinkContainer>
              )}
              <LinkContainer to="/signup">
                <Nav.Link>Sign up</Nav.Link>
              </LinkContainer>
            </Nav>
            {user &&
              <p>Hello {user.username}</p>
            }
          </Navbar.Collapse>
        </Container>
      </Navbar>
    </header>
  )
}

export default Header;