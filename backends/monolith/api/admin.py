from django.contrib import admin
from .models import Symbol, Order, Operation, Strategy

# Register your models here.
admin.site.register(Symbol)
admin.site.register(Order)
admin.site.register(Operation)
admin.site.register(Strategy)
