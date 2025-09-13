# JWT Authentication Flow – Detailed Guide

## 1) Login (Obtain Tokens)

### Available routes
- `POST /api/token/` (legacy)
- `POST /api/auth/token/` (organized)
- `POST /api/login/` (alternative)

### Request
```json
{
  "username": "user@example.com",
  "password": "password123"
}
```

### Response
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

## 2) Use Access Token

### Headers for authenticated requests
```
Authorization: Bearer eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9...
```

### Example
```bash
curl -X GET https://127.0.0.1:8000/api/strategies/ \
  -H "Authorization: Bearer YOUR_ACCESS_TOKEN" \
  -H "Content-Type: application/json"
```

## 3) Refresh Token (when access expires)

### Routes
- `POST /api/token/refresh/`
- `POST /api/auth/token/refresh/`

### Request
```json
{
  "refresh": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9..."
}
```

### Response
```json
{
  "access": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9..."
}
```

## 4) Verify Token

### Routes
- `POST /api/token/verify/`
- `POST /api/auth/token/verify/`

### Request
```json
{
  "token": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9..."
}
```

### Response
- `200 OK` if valid
- `401 Unauthorized` if invalid

## 5) Logout (Blacklist Tokens)

### Routes
- `POST /api/token/blacklist/`
- `POST /api/auth/token/blacklist/`
- `POST /api/logout/` (alternative)

### Request
```json
{
  "refresh": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9..."
}
```

## 6) User Profile

### Route
- `GET /api/user/`

### Headers
```
Authorization: Bearer YOUR_ACCESS_TOKEN
```

### Response
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

## 7) Auth Test Endpoint

### Route
- `POST /api/test-auth/`

### Response
```json
{
  "message": "JWT authentication is working",
  "authenticated": true,
  "user": "user@example.com"
}
```

## Why multiple routes?

### 🎯 Flexibility
- Frontend can choose preferred pattern
- `/api/token/` for compatibility
- `/api/auth/token/` for organization

### 🔄 Gradual Migration
- Legacy systems keep working
- New systems use organized routes

### 🛠️ Extra Functionality
- `/api/user/` for profile data
- `/api/test-auth/` for debugging

### 🏢 Multi‑tenant
- `client_id` support in tokens
- Per‑client data isolation
