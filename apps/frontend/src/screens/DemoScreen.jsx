import React, { useState } from 'react'
import { Container, Row, Col, Card, Button, Form, Alert } from 'react-bootstrap'
import { LinkContainer } from 'react-router-bootstrap'
import Header from "../components/common/Header"
import Footer from "../components/common/Footer"

function DemoScreen() {
  const [demoMode, setDemoMode] = useState(null)
  const [apiKey, setApiKey] = useState('')
  const [secretKey, setSecretKey] = useState('')
  const [isLoading, setIsLoading] = useState(false)
  const [error, setError] = useState('')

  const handleViewOnlyDemo = () => {
    setIsLoading(true)
    setError('')
    
    setTimeout(() => {
      setIsLoading(false)
      window.location.href = '/dashboard?demo=true&mode=viewonly'
    }, 1000)
  }

  const handleJoinWaitlist = async () => {
    setIsLoading(true)
    setError('')
    
    try {
      const email = prompt('Digite seu email para entrar na lista de espera do plano Pro:')
      
      if (!email) {
        setIsLoading(false)
        return
      }
      
      if (!email.includes('@')) {
        throw new Error('Por favor, digite um email válido')
      }
      
      const response = await fetch('/api/waitlist/join/', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({
          email: email,
          is_demo_user: true
        })
      })
      
      if (!response.ok) {
        const errorData = await response.json()
        throw new Error(errorData.error || 'Erro ao entrar na lista de espera')
      }
      
      const result = await response.json()
      alert(`✅ ${result.message}`)
      
    } catch (error) {
      setError(error.message)
    } finally {
      setIsLoading(false)
    }
  }

  const handleTestnetDemo = async (e) => {
    e.preventDefault()
    
    if (!apiKey || !secretKey) {
      setError('Por favor, preencha ambas as chaves da API')
      return
    }

    setIsLoading(true)
    setError('')
    
    try {
      // Validar credenciais primeiro
      const validateResponse = await fetch('/api/demo/validate-credentials/', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({
          api_key: apiKey,
          secret_key: secretKey
        })
      })
      
      if (!validateResponse.ok) {
        const errorData = await validateResponse.json()
        throw new Error(errorData.error || 'Credenciais inválidas')
      }
      
      // Criar conta demo
      const demoResponse = await fetch('/api/demo/create/', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({
          username: `demo_${Date.now()}`,
          email: `demo_${Date.now()}@robsonbot.com`,
          password: 'demo_password_123',
          api_key: apiKey,
          secret_key: secretKey
        })
      })
      
      if (!demoResponse.ok) {
        const errorData = await demoResponse.json()
        throw new Error(errorData.error || 'Erro ao criar conta demo')
      }
      
      const demoData = await demoResponse.json()
      
      // Armazenar tokens e redirecionar
      localStorage.setItem('accessToken', demoData.tokens.access)
      localStorage.setItem('refreshToken', demoData.tokens.refresh)
      
      window.location.href = '/dashboard'
      
    } catch (error) {
      setIsLoading(false)
      setError(error.message || 'Erro ao criar conta demo')
    }
  }

  return (
    <div className="d-flex flex-column min-vh-100">
      <Header />
      <main className="flex-grow-1 py-5">
        <Container>
          <Row className="justify-content-center">
            <Col md={8} lg={6}>
              <div className="text-center mb-5">
                <h1 className="fw-bold text-primary mb-3">Demo Inteligente</h1>
                <p className="text-secondary lead">
                  Escolha como você quer experimentar o Robson
                </p>
              </div>

              {!demoMode ? (
                <Row className="g-4">
                  <Col md={6}>
                    <Card className="h-100 border-0 shadow-sm demo-card">
                      <Card.Body className="text-center p-4">
                        <div className="mb-4">
                          <div className="bg-primary bg-opacity-10 rounded-circle d-inline-flex align-items-center justify-content-center" style={{ width: '80px', height: '80px' }}>
                            <span className="fs-2 text-primary">👁️</span>
                          </div>
                        </div>
                        <h4 className="fw-bold mb-3">Modo Visualização</h4>
                        <p className="text-secondary mb-4">
                          Apenas observe o Robson operando com uma conta demo administrada.
                          Sem riscos, sem movimentação de fundos.
                        </p>
                        <Button 
                          variant="primary" 
                          size="lg"
                          onClick={() => setDemoMode('viewonly')}
                          className="w-100"
                        >
                          Começar Demo
                        </Button>
                      </Card.Body>
                    </Card>
                  </Col>

                  <Col md={6}>
                    <Card className="h-100 border-0 shadow-sm demo-card">
                      <Card.Body className="text-center p-4">
                        <div className="mb-4">
                          <div className="bg-success bg-opacity-10 rounded-circle d-inline-flex align-items-center justify-content-center" style={{ width: '80px', height: '80px' }}>
                            <span className="fs-2 text-success">🔑</span>
                          </div>
                        </div>
                        <h4 className="fw-bold mb-3">Testnet com Suas Chaves</h4>
                        <p className="text-secondary mb-4">
                          Use suas próprias chaves de testnet da Binance para testar 
                          com fundos virtuais. Limite de 3 dias.
                        </p>
                        <Button 
                          variant="success" 
                          size="lg"
                          onClick={() => setDemoMode('testnet')}
                          className="w-100"
                        >
                          Usar Minhas Chaves
                        </Button>
                      </Card.Body>
                    </Card>
                  </Col>
                </Row>
              ) : (
                <Card className="border-0 shadow-sm">
                  <Card.Body className="p-5">
                    {demoMode === 'viewonly' ? (
                      <div className="text-center">
                        <div className="mb-4">
                          <div className="bg-primary bg-opacity-10 rounded-circle d-inline-flex align-items-center justify-content-center mx-auto" style={{ width: '100px', height: '100px' }}>
                            <span className="fs-1 text-primary">👁️</span>
                          </div>
                        </div>
                        <h3 className="fw-bold mb-3">Demo de Visualização</h3>
                        <p className="text-secondary mb-4">
                          Você entrará no dashboard em modo de visualização onde poderá 
                          observar o Robson operando com uma conta demo administrada.
                        </p>
                        
                        <div className="alert alert-info mb-4">
                          <strong>⚠️ Modo Somente Leitura</strong><br />
                          Nenhuma operação real será executada. Apenas visualização.
                        </div>

                        <div className="d-grid gap-2 d-md-flex justify-content-md-center">
                          <Button 
                            variant="outline-secondary" 
                            onClick={() => setDemoMode(null)}
                            disabled={isLoading}
                          >
                            Voltar
                          </Button>
                          <Button 
                            variant="primary" 
                            onClick={handleViewOnlyDemo}
                            disabled={isLoading}
                          >
                            {isLoading ? 'Carregando...' : 'Iniciar Demo'}
                          </Button>
                        </div>

                        <div className="text-center mt-4 pt-3 border-top">
                          <p className="text-muted mb-2">Gostou da demo?</p>
                          <Button 
                            variant="outline-primary" 
                            onClick={handleJoinWaitlist}
                            disabled={isLoading}
                            size="sm"
                          >
                            Entrar na Lista de Espera do Plano Pro
                          </Button>
                        </div>
                      </div>
                    ) : (
                      <div>
                        <div className="text-center mb-4">
                          <div className="bg-success bg-opacity-10 rounded-circle d-inline-flex align-items-center justify-content-center mx-auto" style={{ width: '100px', height: '100px' }}>
                            <span className="fs-1 text-success">🔑</span>
                          </div>
                        </div>
                        <h3 className="fw-bold text-center mb-4">Demo com Testnet</h3>
                        
                        {error && (
                          <Alert variant="danger" className="mb-4">
                            {error}
                          </Alert>
                        )}

                        <Form onSubmit={handleTestnetDemo}>
                          <Form.Group className="mb-3">
                            <Form.Label className="fw-bold">API Key da Testnet</Form.Label>
                            <Form.Control
                              type="text"
                              placeholder="Sua API Key da Binance Testnet"
                              value={apiKey}
                              onChange={(e) => setApiKey(e.target.value)}
                              disabled={isLoading}
                              className="bg-light"
                            />
                            <Form.Text className="text-muted">
                              Disponível no dashboard da Binance Testnet
                            </Form.Text>
                          </Form.Group>

                          <Form.Group className="mb-4">
                            <Form.Label className="fw-bold">Secret Key da Testnet</Form.Label>
                            <Form.Control
                              type="password"
                              placeholder="Sua Secret Key da Binance Testnet"
                              value={secretKey}
                              onChange={(e) => setSecretKey(e.target.value)}
                              disabled={isLoading}
                              className="bg-light"
                            />
                            <Form.Text className="text-muted">
                              Mantenha esta chave em segredo
                            </Form.Text>
                          </Form.Group>

                          <div className="alert alert-warning mb-4">
                            <strong>⚠️ Importante</strong><br />
                            • Use apenas chaves da Binance Testnet<br />
                            • Demo limitada a 3 dias<br />
                            • Se gostar, assine o plano Pro para usar chaves de produção
                          </div>

                          <div className="d-grid gap-2 d-md-flex justify-content-md-center">
                            <Button 
                              variant="outline-secondary" 
                              onClick={() => setDemoMode(null)}
                              disabled={isLoading}
                            >
                              Voltar
                            </Button>
                            <Button 
                              type="submit"
                              variant="success" 
                              disabled={isLoading}
                            >
                              {isLoading ? 'Conectando...' : 'Iniciar Demo com Minhas Chaves'}
                            </Button>
                          </div>
                        </Form>
                      </div>
                    )}
                  </Card.Body>
                </Card>
              )}

              <div className="text-center mt-5 pt-4">
                <p className="text-muted">
                  Após a demo, se quiser continuar usando o Robson com sua conta real,
                  <LinkContainer to="/pricing">
                    <Button variant="link" className="p-0 ms-1">assine o plano Pro</Button>
                  </LinkContainer>
                </p>
              </div>
            </Col>
          </Row>
        </Container>
      </main>
      <Footer />
    </div>
  )
}

export default DemoScreen
