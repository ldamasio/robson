# Reset Admin Password - Production Guide

This guide explains how to reset the admin user password in production environment.

## Prerequisites

- Access to Kubernetes cluster (kubectl configured)
- Access to production namespace (`robson`)
- Backend pod running in production

## Method 1: Using Django Management Command (Recommended)

### Step 1: Find Backend Pod

```bash
# List backend pods
kubectl get pods -n robson | grep backend

# Example output:
# backend-7d8f9c4b5-abc123   1/1     Running   0          2h
```

### Step 2: Execute Command in Pod

```bash
# Replace POD_NAME with actual pod name
kubectl exec -it -n robson POD_NAME -- python manage.py reset_admin_password --username admin --password YOUR_NEW_PASSWORD
```

### Step 3: Verify Password Reset

```bash
# Test login via API
curl -X POST https://api.robsonbot.com/api/token/ \
  -H "Content-Type: application/json" \
  -d '{"username":"admin","password":"YOUR_NEW_PASSWORD"}'
```

## Method 2: Interactive Password Entry (More Secure)

If you prefer not to pass password via command line:

```bash
# Execute command (will prompt for password)
kubectl exec -it -n robson POD_NAME -- python manage.py reset_admin_password --username admin
```

The command will prompt you to enter and confirm the password securely.

## Method 3: Create Admin User if Not Exists

If the admin user doesn't exist:

```bash
kubectl exec -it -n robson POD_NAME -- python manage.py reset_admin_password \
  --username admin \
  --password YOUR_NEW_PASSWORD \
  --email admin@robsonbot.com \
  --create-if-not-exists \
  --make-superuser
```

## Method 4: Using Django Shell (Alternative)

If you prefer using Django shell directly:

```bash
# Open Django shell
kubectl exec -it -n robson POD_NAME -- python manage.py shell
```

Then in the shell:

```python
from django.contrib.auth import get_user_model
User = get_user_model()

# Get or create user
username = 'admin'
password = 'YOUR_NEW_PASSWORD'
email = 'admin@robsonbot.com'

try:
    user = User.objects.get(username=username)
    user.set_password(password)
    user.is_active = True
    user.save()
    print(f'Password reset for user: {username}')
except User.DoesNotExist:
    user = User.objects.create_superuser(username=username, email=email, password=password)
    print(f'Created user: {username}')
```

## Security Best Practices

1. **Use strong passwords**: Minimum 8 characters, mix of letters, numbers, and symbols
2. **Don't log passwords**: Avoid passing passwords via command line in scripts that might be logged
3. **Use interactive mode**: Prefer Method 2 for production to avoid password in command history
4. **Verify immediately**: Test login after resetting password
5. **Rotate regularly**: Change admin password periodically

## Troubleshooting

### Error: "User does not exist"

Use `--create-if-not-exists` flag to create the user.

### Error: "Password too short"

Ensure password is at least 8 characters long.

### Error: "Cannot connect to pod"

Verify:
- Pod is running: `kubectl get pods -n robson`
- You have access to the cluster: `kubectl get nodes`
- Namespace is correct: `kubectl get namespaces | grep robson`

### Error: "Permission denied"

Ensure you have proper Kubernetes RBAC permissions to exec into pods.

## Quick Reference

```bash
# One-liner: Reset admin password
kubectl exec -it -n robson $(kubectl get pods -n robson -l app=backend -o jsonpath='{.items[0].metadata.name}') -- \
  python manage.py reset_admin_password --username admin --password NEW_PASSWORD

# One-liner: Create admin if not exists
kubectl exec -it -n robson $(kubectl get pods -n robson -l app=backend -o jsonpath='{.items[0].metadata.name}') -- \
  python manage.py reset_admin_password --username admin --password NEW_PASSWORD --create-if-not-exists --make-superuser
```

## Related Commands

- `python manage.py createsuperuser` - Create superuser interactively
- `python manage.py changepassword <username>` - Django's built-in password change command

