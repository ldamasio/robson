import os
import django
from django.contrib.auth import get_user_model

os.environ.setdefault('DJANGO_SETTINGS_MODULE', 'backend.settings')
django.setup()

User = get_user_model()
username = 'robson'
password = '123'
email = 'robson@example.com'

try:
    user = User.objects.get(username=username)
    user.set_password(password)
    user.is_active = True
    user.save()
    print(f"Successfully updated user '{username}' with password '{password}'")
except User.DoesNotExist:
    User.objects.create_user(username=username, email=email, password=password)
    print(f"Successfully created user '{username}' with password '{password}'")
except Exception as e:
    print(f"Error: {e}")
