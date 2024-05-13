from django.db import models
from clients.models import Client, CustomUser

class Symbol(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    name = models.CharField(max_length=255)
    description = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

    class Meta(object):
        unique_together = ["id", "client"]

class Order(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    symbol_orderd = models.ForeignKey(
        Symbol, blank=True, null=True, on_delete=models.SET_NULL)

class Strategy(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    name = models.CharField(max_length=255)

    class Meta:
        verbose_name_plural = 'strategies'

    def __str__(self):
        return self.name

class Operation(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    strategy = models.ForeignKey(
        Strategy, blank=True, null=True, on_delete=models.SET_NULL)
    side = models.CharField(max_length=5)
    stop_gain_percent = models.IntegerField(blank=True)
    stop_loss_percent = models.IntegerField(blank=True)

# Technica Analysis

class TechnicalEvent():
    type = ('Bullish', 'Bearish')

class Argument():
    type = ('Bullish', 'Bearish')

class Accumulation():
    type = ('Bullish', 'Bearish')

# Chart Patterns

class Rectangle():
    type = ('Bullish', 'Bearish')

class Triangle():
    type = ('Bullish', 'Bearish')

class Hammer():
    type = ('Bullish', 'Bearish')

class InvertedHammer():
    type = ('Bullish', 'Bearish')

class HangingMan():
    type = ('Bullish', 'Bearish')

class Piercing():
    type = ('Bullish', 'Bearish')

# Reversal Patterns

class Engulfing():
    type = ('Bullish', 'Bearish')

class ShootingStart():
    thre_candles = ('Closure', 'Opening', 'Closure')
    type = ('Bullish')

class MorningStart():
    thre_candles = ('Closure', 'Opening', 'Closure')
    type = ('Bullish')

class EveningStart():
    thre_candles = ('Closure', 'Opening', 'Closure')
    type = ('Bullish')
