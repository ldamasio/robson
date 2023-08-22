from django.db import models
from django.contrib.auth.models import AbstractUser
from django_multitenant.fields import *
from django_multitenant.models import *

class Client(TenantModel):
    tenant_id = 'id'
    name = models.CharField(max_length=50)
    address = models.CharField(max_length=255)
    email = models.CharField(max_length=50, unique=True)
    api_url = models.CharField(max_length=255)
    stream_url = models.CharField(max_length=255)
    access_key = models.CharField(max_length=255)
    secret_key = models.CharField(max_length=255)
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

class CustomUser(AbstractUser):
    client = models.ForeignKey(Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id='client_id'
    account = models.BooleanField(default=False)

    def __str__(self):
        return self.username
