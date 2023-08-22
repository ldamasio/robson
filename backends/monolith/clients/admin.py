from django.contrib import admin
from .models import Client, CustomUser

admin.site.register(Client)
admin.site.register(CustomUser)