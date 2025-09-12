# Fluxo de Autenticação JWT - Explicação Detalhada

## 1. Login (Obter Tokens)

### Rotas disponíveis:
- `POST /api/token/` (legacy)
- `POST /api/auth/token/` (organizada)
- `POST /api/login/` (alternativa)

### Request:
```json
{
    "username": "user@example.com",
    "password": "password123"
}
```

### Response:
```json
{
    "access": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9...",
    "refresh": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9...",
    "username": "user@example.com",
    "email": "user@example.com",
    "user_id": 123,
    "client_id": 456,
    "client_name": "Company ABC"
}
```

## 2. Usar Access Token

### Headers para requests autenticados:
```
Authorization: Bearer eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9...
```

### Exemplo:
```bash
curl -X GET https://127.0.0.1:8000/api/strategies/ \
  -H "Authorization: Bearer YOUR_ACCESS_TOKEN" \
  -H "Content-Type: application/json"
```

## 3. Refresh Token (quando access expira)

### Rota:
- `POST /api/token/refresh/`
- `POST /api/auth/token/refresh/`

### Request:
```json
{
    "refresh": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9..."
}
```

### Response:
```json
{
    "access": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9..."
}
```

## 4. Verificar Token

### Rota:
- `POST /api/token/verify/`
- `POST /api/auth/token/verify/`

### Request:
```json
{
    "token": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9..."
}
```

### Response:
- `200 OK` se válido
- `401 Unauthorized` se inválido

## 5. Logout (Invalidar Tokens)

### Rotas:
- `POST /api/token/blacklist/`
- `POST /api/auth/token/blacklist/`
- `POST /api/logout/` (alternativa)

### Request:
```json
{
    "refresh": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9..."
}
```

## 6. Perfil do Usuário

### Rota:
- `GET /api/user/`

### Headers:
```
Authorization: Bearer YOUR_ACCESS_TOKEN
```

### Response:
```json
{
    "user_id": 123,
    "username": "user@example.com",
    "email": "user@example.com",
    "first_name": "John",
    "last_name": "Doe",
    "client_id": 456,
    "client_name": "Company ABC",
    "is_active": true,
    "date_joined": "2025-01-01T00:00:00Z",
    "last_login": "2025-06-15T15:30:00Z"
}
```

## 7. Teste de Autenticação

### Rota:
- `POST /api/test-auth/`

### Response:
```json
{
    "message": "JWT authentication is working",
    "authenticated": true,
    "user": "user@example.com"
}
```

## Por que tantas rotas?

### 🎯 **Flexibilidade**
- Frontend pode escolher o padrão que prefere
- `/api/token/` para compatibilidade
- `/api/auth/token/` para organização

### 🔄 **Migração gradual**
- Sistemas antigos continuam funcionando
- Novos sistemas usam rotas organizadas

### 🛠️ **Funcionalidades extras**
- `/api/user/` para dados do perfil
- `/api/test-auth/` para debugging

### 🏢 **Multi-tenant**
- Suporte a `client_id` nos tokens
- Isolamento de dados por cliente