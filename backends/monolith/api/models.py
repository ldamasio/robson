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

# The position size for  each trade is automatically
# calculated based on the  second technical event
# on the 15-minute chart stop loss.

# Principles

class OddsYourFavor(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    name = models.CharField(max_length=255)
    experience = models.IntegerField()
    description = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

class LimitLosses(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    name = models.CharField(max_length=255)
    experience = models.IntegerField()
    description = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

# Attributes

class Attribute(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    name = models.CharField(max_length=255)
    experience = models.IntegerField()
    description = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

    def context(self):
        return self.name

    def primary_implication(self):
        return self.name

    def underlying_objective(self):
        return self.name

    def volume(self):
        return self.name

    def perspective(self):
        return self.name

# Technica Analysis

class TechnicalAnalysisInterpretation(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    name = models.CharField(max_length=255)
    experience = models.IntegerField()
    description = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

class TechnicalEvent(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    interpretation = models.ForeignKey(
        TechnicalAnalysisInterpretation, blank=True, null=True, on_delete=models.SET_NULL)
    type = ('Bullish', 'Bearish')

class Argument(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    technical_event = models.ForeignKey(
        TechnicalEvent, blank=True, null=True, on_delete=models.SET_NULL)
    type = ('Bullish', 'Bearish')
    description = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

class Reason(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    argument = models.ForeignKey(
        Argument, blank=True, null=True, on_delete=models.SET_NULL)
    type = ('Bullish', 'Bearish')
    description = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

# Facts

class Resistance(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    argument = models.ForeignKey(
        Argument, blank=True, null=True, on_delete=models.SET_NULL)
    type = ('Bullish', 'Bearish')
    description = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

class Support(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    argument = models.ForeignKey(
        Argument, blank=True, null=True, on_delete=models.SET_NULL)
    type = ('Bullish', 'Bearish')
    description = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

class Line(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    argument = models.ForeignKey(
        Argument, blank=True, null=True, on_delete=models.SET_NULL)
    type = ('Bullish', 'Bearish')
    description = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

class TrendLine(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    argument = models.ForeignKey(
        Argument, blank=True, null=True, on_delete=models.SET_NULL)
    type = ('Bullish', 'Bearish')
    description = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

class Channel(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    argument = models.ForeignKey(
        Argument, blank=True, null=True, on_delete=models.SET_NULL)
    type = ('Bullish', 'Bearish')
    description = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

class Accumulation(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    argument = models.ForeignKey(
        Argument, blank=True, null=True, on_delete=models.SET_NULL)
    type = ('Bullish', 'Bearish')
    description = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

class Sideways(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    argument = models.ForeignKey(
        Argument, blank=True, null=True, on_delete=models.SET_NULL)
    type = ('Bullish', 'Bearish')
    description = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

class Breakout(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    argument = models.ForeignKey(
        Argument, blank=True, null=True, on_delete=models.SET_NULL)
    type = ('Bullish', 'Bearish')
    description = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

class Uptrend(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    argument = models.ForeignKey(
        Argument, blank=True, null=True, on_delete=models.SET_NULL)
    type = ('Bullish', 'Bearish')
    description = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

class Downtrend(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    argument = models.ForeignKey(
        Argument, blank=True, null=True, on_delete=models.SET_NULL)
    type = ('Bullish', 'Bearish')
    description = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)


# Chart Patterns

class Rectangle(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    type = ('Bullish', 'Bearish')
    description = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

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

class ShootingStar():
    thre_candles = ('Closure', 'Opening', 'Closure')
    type = ('Bullish')

class MorningStar():
    thre_candles = ('Closure', 'Opening', 'Closure')
    type = ('Bullish')

class EveningStar():
    thre_candles = ('Closure', 'Opening', 'Closure')
    type = ('Bullish')
    description = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)
    tip = models.TextField(default='Observe evening star on the 5-minute chart. The Evening Star pattern is a reversal pattern that forms when a bearish candlestick is followed by two bullish candlesticks, with the second bullish candlestick closing higher than the first bullish candlestick.')


# Statistical Indicators

class MovingAverage():
    type = ('Bullish', 'Bearish')

class RelativeStrengthIndex():
    type = ('Bullish', 'Bearish')

class MovingAverageConvergenceDivergence():
    type = ('Bullish', 'Bearish')

class BollingerBands():
    type = ('Bullish', 'Bearish')

class StochasticOscillator():
        type = ('Bullish', 'Bearish')

# Rules

class OnePercentOfCapital(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    type = ('Bullish', 'Bearish')
    description = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

class JustBet4percent(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    type = ('Bullish', 'Bearish')
    description = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

# Config

class OnlyTradeReversal(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    type = ('Bullish', 'Bearish')
    description = models.TextField(default = 'Reversals reinforce the trend of the opposing technical event within the chart pattern.')
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

class MaxTradePerDay(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    type = ('Bullish', 'Bearish')
    description = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)

# Reports

class AlocatedCapitalPercent(models.Model):
    client = models.ForeignKey(
        Client, blank=True, null=True, on_delete=models.SET_NULL)
    tenant_id = 'client_id'
    type = ('Bullish', 'Bearish')
    description = models.TextField()
    created_at = models.DateTimeField(auto_now_add=True)
    updated_at = models.DateTimeField(auto_now=True)
