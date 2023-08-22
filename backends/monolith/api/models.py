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

    class Meta:
        verbose_name_plural = 'strategies'
