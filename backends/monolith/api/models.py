from django.db import models
from django_multitenant.fields import *
from django_multitenant.models import *
from clients.models import Client

class Symbol(TenantModel):
    client = models.ForeignKey(Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id='client_id'
    name = models.CharField(max_length=255)
    description = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

    class Meta(object):
      unique_together = ["id", "client"]

class Order(TenantModel):
    client = models.ForeignKey(Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id='client_id'
    symbol_orderd = TenantForeignKey(Symbol, blank=True, null=True, on_delete=models.SET_NULL)

class Operation(TenantModel):
    client = models.ForeignKey(Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id='client_id'

class Strategy(TenantModel):
    client = models.ForeignKey(Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id='client_id'

# class Client(TenantModel):
#     tenant_id = 'id'
#     name = models.CharField(max_length=50)
#     address = models.CharField(max_length=255)
#     email = models.CharField(max_length=50, unique=True)
#     api_url = models.CharField(max_length=255)
#     stream_url = models.CharField(max_length=255)
#     access_key = models.CharField(max_length=255)
#     secret_key = models.CharField(max_length=255)
#     created_at = models.DateTimeField(auto_now_add=True)
#     updated_at = models.DateTimeField(auto_now=True)

# class SymbolManager(TenantManagerMixin, models.Manager):
#     pass

# class Symbol(TenantModelMixin, models.Model):
#     client = models.ForeignKey(Client, blank=True, null=True, on_delete=models.SET_NULL)
#     tenant_id='client_id'
#     name = models.CharField(max_length=255)
#     description = models.TextField()
#     created_at = models.DateTimeField(auto_now_add=True)
#     updated_at = models.DateTimeField(auto_now=True)
#     objects = SymbolManager()

#     class Meta(object):
#       unique_together = ["id", "client"]

# class OrderManager(TenantManagerMixin, models.Manager):
#     pass

# class Order(TenantModelMixin, models.Model):
#     client = models.ForeignKey(Client, blank=True, null=True, on_delete=models.SET_NULL)
#     tenant_id='client_id'
#     symbol_orderd = TenantForeignKey(Symbol, blank=True, null=True, on_delete=models.SET_NULL)

#     objects = OrderManager()

# class Operation(TenantModelMixin, models.Model):
#     client = models.ForeignKey(Client, blank=True, null=True, on_delete=models.SET_NULL)
#     tenant_id='client_id'


