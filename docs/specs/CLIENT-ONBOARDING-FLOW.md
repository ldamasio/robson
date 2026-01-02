# Client Onboarding & Billing Flow

## Current State Analysis

### Existing Components
- ✅ `LoginScreen.jsx` - Functional (uses AuthContext)
- ❌ `SignupScreen.jsx` - NOT functional (only HTML form, no submit handler)
- ❌ No backend API endpoint for user registration
- ❌ No client approval workflow
- ❌ No billing/usage tracking screens

### Multi-Tenant Architecture
- `CustomUser` has `client` ForeignKey (nullable)
- `Client` model represents trading account with API credentials
- **ALL views require `request.user.client`** - no superuser exception
- Signal now auto-assigns default client on user creation

---

## Proposed End-to-End Flow

```
┌─────────────────────────────────────────────────────────────────────────┐
│                        CLIENT ONBOARDING FLOW                            │
└─────────────────────────────────────────────────────────────────────────┘

  USER                 SIGNUP SCREEN              BACKEND                  ADMIN
   │                       │                        │                       │
   │  1. Click Sign Up     │                        │                       │
   │──────────────────────>│                        │                       │
   │                       │                        │                       │
   │  2. Fill Form         │                        │                       │
   │  (name, email, user,  │                        │                       │
   │   password)           │                        │                       │
   │──────────────────────>│                        │                       │
   │                       │                        │                       │
   │                       │  3. POST /api/auth/signup                      │
   │                       │───────────────────────>│                       │
   │                       │                        │                       │
   │                       │                        │  4. Create User       │
   │                       │                        │  (status=PENDING)     │
   │                       │                        │  Create Client        │
   │                       │                        │  (status=TRIAL)       │
   │                       │                        │  Assign User->Client  │
   │                       │                        │                       │
   │                       │                        │  5. Send email to     │
   │                       │                        │  admin & user         │
   │                       │                        │──────────────────────>│
   │                       │                        │                       │
   │  6. Show Pending      │<───────────────────────│                       │
   │     Approval Screen   │                        │                       │
   │<──────────────────────│                        │                       │
   │                       │                        │                       │
   │                       │                        │                       │
   │                       │                        │     7. Review Client  │
   │                       │                        │<──────────────────────│
   │                       │                        │                       │
   │                       │                        │  8. Approve/Reject    │
   │                       │                        │<──────────────────────│
   │                       │                        │──────────────────────>│
   │                       │                        │                       │
   │                       │                        │  9. Update Client     │
   │                       │                        │  (status=ACTIVE)      │
   │                       │                        │                       │
   │                       │  10. Email: Approved   │                       │
   │                       │<───────────────────────│                       │
   │                       │                        │                       │
   │  11. Can Now Login    │                        │                       │
   │<──────────────────────│                        │                       │
```

---

## Database Schema Changes

### Client Model - Add Status Field

```python
class Client(models.Model):
    # ... existing fields ...

    # NEW: Approval status
    class Status(models.TextChoices):
        PENDING = 'PENDING', 'Pending Approval'
        TRIAL = 'TRIAL', 'Trial Period'
        ACTIVE = 'ACTIVE', 'Active'
        SUSPENDED = 'SUSPENDED', 'Suspended'
        REJECTED = 'REJECTED', 'Rejected'

    status = models.CharField(
        max_length=20,
        choices=Status.choices,
        default=Status.PENDING
    )

    # NEW: Trial tracking
    trial_started_at = models.DateTimeField(blank=True, null=True)
    trial_expires_at = models.DateTimeField(blank=True, null=True)

    # NEW: Admin notes
    admin_notes = models.TextField(blank=True)

    # NEW: Rejection reason (if rejected)
    rejection_reason = models.TextField(blank=True)
```

### Usage Tracking Models

```python
class APIUsage(models.Model):
    """Track API usage per client for billing."""
    client = models.ForeignKey(Client, on_delete=models.CASCADE)
    timestamp = models.DateTimeField(auto_now_add=True)
    endpoint = models.CharField(max_length=255)
    method = models.CharField(max_length=10)  # GET, POST, etc.
    response_status = models.IntegerField()

    class Meta:
        indexes = [
            models.Index(fields=['client', '-timestamp']),
        ]

class AIUsage(models.Model):
    """Track AI model usage per client."""
    client = models.ForeignKey(Client, on_delete=models.CASCADE)
    timestamp = models.DateTimeField(auto_now_add=True)
    model_name = models.CharField(max_length=100)  # claude-3, gpt-4, etc.
    tokens_used = models.IntegerField()
    operation_type = models.CharField(max_length=50)  # chat, analysis, etc.
    cost_usd = models.DecimalField(max_digits=10, decimal_places=4)

    class Meta:
        indexes = [
            models.Index(fields=['client', '-timestamp']),
        ]
```

---

## Backend Implementation

### 1. Signup Endpoint

**File:** `api/urls/auth.py` (new)

```python
# POST /api/auth/signup
{
    "full_name": "John Doe",
    "email": "john@example.com",
    "username": "johndoe",
    "password": "securepassword123"
}

# Response 201
{
    "message": "Account created. Awaiting admin approval.",
    "user_id": 123,
    "client_id": 45
}

# Response 400
{
    "error": "Email already registered"
}
```

**Implementation:** `api/views/auth_views.py`

```python
@api_view(['POST'])
@permission_classes([AllowAny])
def signup(request):
    """Create new user and client account."""
    serializer = SignupSerializer(data=request.data)
    serializer.is_valid(raise_exception=True)

    # Create Client (TRIAL status)
    client = Client.objects.create(
        name=serializer.validated_data['client_name'],
        email=serializer.validated_data['email'],
        status=Client.Status.TRIAL,
        is_demo_account=True,
        trial_expires_at=now() + timedelta(days=3)
    )

    # Create User (PENDING status - can't login yet)
    user = CustomUser.objects.create_user(
        username=serializer.validated_data['username'],
        email=serializer.validated_data['email'],
        password=serializer.validated_data['password'],
        client=client,
        is_active=False  # Inactive until approved
    )

    # Send notification emails
    send_signup_notification_email(user, client)
    send_admin_review_email(user, client)

    return Response({
        "message": "Account created. Awaiting admin approval.",
        "user_id": user.id,
        "client_id": client.id
    }, status=201)
```

### 2. Admin Approval Endpoint

**File:** `api/views/admin_views.py` (new)

```python
# GET /api/admin/pending-clients
# List all clients pending approval

# POST /api/admin/clients/{id}/approve
# Approve client and activate user

# POST /api/admin/clients/{id}/reject
# Reject client with reason
```

### 3. Billing & Usage Endpoints

**File:** `api/views/billing_views.py` (new)

```python
# GET /api/billing/usage
# Get current usage summary

# GET /api/billing/usage/history
# Get usage history with filters

# GET /api/billing/invoices
# Get invoices (future)
```

---

## Frontend Implementation

### 1. Functional Signup Screen

**File:** `apps/frontend/src/screens/SignupScreen.jsx`

```jsx
const SignupScreen = () => {
  const [formData, setFormData] = useState({
    full_name: '',
    email: '',
    username: '',
    password: '',
    confirm_password: ''
  })
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState(null)
  const [success, setSuccess] = useState(false)
  const navigate = useNavigate()

  const handleSubmit = async (e) => {
    e.preventDefault()
    setLoading(true)
    setError(null)

    try {
      await api.post('/auth/signup', formData)
      setSuccess(true)
      setTimeout(() => navigate('/pending-approval'), 3000)
    } catch (err) {
      setError(err.response?.data?.error || 'Signup failed')
    } finally {
      setLoading(false)
    }
  }

  if (success) {
    return <SuccessMessage />
  }

  return <Form onSubmit={handleSubmit}>...</Form>
}
```

### 2. Pending Approval Screen (NEW)

**File:** `apps/frontend/src/screens/PendingApprovalScreen.jsx`

```jsx
const PendingApprovalScreen = () => {
  return (
    <Container className="py-5 text-center">
      <Alert variant="info">
        <h4>📧 Account Created - Awaiting Approval</h4>
        <p>Your account is pending review by our team.</p>
        <p>You'll receive an email once your account is activated.</p>
      </Alert>
    </Container>
  )
}
```

### 3. Admin Approval Screen (NEW)

**File:** `apps/frontend/src/screens/admin/ClientApprovalScreen.jsx`

```jsx
const ClientApprovalScreen = () => {
  const [pendingClients, setPendingClients] = useState([])

  useEffect(() => {
    fetchPendingClients()
  }, [])

  const handleApprove = async (clientId) => {
    await api.post(`/admin/clients/${clientId}/approve`)
    fetchPendingClients()
  }

  const handleReject = async (clientId, reason) => {
    await api.post(`/admin/clients/${clientId}/reject`, { reason })
    fetchPendingClients()
  }

  return (
    <Table>
      {pendingClients.map(client => (
        <tr key={client.id}>
          <td>{client.name}</td>
          <td>{client.email}</td>
          <td>{client.created_at}</td>
          <td>
            <Button onClick={() => handleApprove(client.id)}>Approve</Button>
            <Button onClick={() => handleReject(client.id)}>Reject</Button>
          </td>
        </tr>
      ))}
    </Table>
  )
}
```

### 4. Client Billing Screen (NEW)

**File:** `apps/frontend/src/screens/client/BillingScreen.jsx`

```jsx
const BillingScreen = () => {
  const [usage, setUsage] = useState(null)

  useEffect(() => {
    fetchUsage()
  }, [])

  return (
    <Container>
      <h2>Billing & Usage</h2>

      <Card>
        <Card.Header>Current Plan: {usage?.plan || 'Trial'}</Card.Header>
        <Card.Body>
          <Row>
            <Col>
              <h3>API Calls</h3>
              <p>{usage?.api_calls_this_month || 0}</p>
            </Col>
            <Col>
              <h3>AI Tokens</h3>
              <p>{usage?.ai_tokens_this_month || 0}</p>
            </Col>
            <Col>
              <h3>Estimated Cost</h3>
              <p>${usage?.estimated_cost || 0}</p>
            </Col>
          </Row>
        </Card.Body>
      </Card>

      <UsageChart data={usage?.history} />
    </Container>
  )
}
```

---

## Implementation Phases

### Phase 1: Core Signup Flow (Priority)
1. ✅ Create signal for auto-assign client
2. ⏳ Fix admin user in production
3. ⏳ Add status field to Client model
4. ⏳ Create signup API endpoint
5. ⏳ Make SignupScreen functional
6. ⏳ Create PendingApprovalScreen
7. ⏳ Test end-to-end signup flow

### Phase 2: Admin Approval
1. ⏳ Create admin approval API endpoints
2. ⏳ Create ClientApprovalScreen (admin only)
3. ⏳ Add email notifications
4. ⏳ Test approval workflow

### Phase 3: Billing & Usage
1. ⏳ Create APIUsage and AIUsage models
2. ⏳ Create middleware to track usage
3. ⏳ Create billing API endpoints
4. ⏳ Create BillingScreen for clients
5. ⏳ Add usage charts

### Phase 4: Advanced Features
1. ⏳ Payment integration (Stripe)
2. ⏳ Plan upgrade/downgrade
3. ⏳ Invoice generation
4. ⏳ Usage alerts/limits

---

## Security Considerations

1. **Rate limiting** on signup endpoint (prevent abuse)
2. **Email verification** before activation
3. **Admin-only** approval endpoints (permission checks)
4. **Data isolation** - users can only see their own usage
5. **Audit trail** - log all approval/rejection actions

---

## Open Questions

1. **Who can be admin?** Should we have a dedicated `is_staff` flag?
2. **Should superuser see all clients?** For multi-admin support?
3. **Trial period length?** Default 3 days, configurable?
4. **Payment model?** Free tier, paid tiers, usage-based?

---

**Next Steps:**
1. Execute production fix for admin user
2. Create migration for Client.status field
3. Implement signup API endpoint
4. Update SignupScreen with functionality
