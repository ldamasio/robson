from django.contrib import admin
from api.models import Symbol, Strategy, Order, Operation, Position, Trade

admin.site.register(Symbol)
admin.site.register(Strategy)
admin.site.register(Order)
admin.site.register(Operation)
admin.site.register(Position)
admin.site.register(Trade)
