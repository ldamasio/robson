# api/models/patterns.py

from django.db import models
from .base import BaseTechnicalModel

class ChartPattern(BaseTechnicalModel):
    """Classe base para padrões de gráfico"""
    name = models.CharField(max_length=100)
    reliability = models.DecimalField(max_digits=5, decimal_places=2)
    
    class Meta:
        abstract = True

class Rectangle(ChartPattern):
    """Padrão Retângulo"""
    width = models.DecimalField(max_digits=10, decimal_places=4)
    height = models.DecimalField(max_digits=10, decimal_places=4)

class Triangle(ChartPattern):
    """Padrão Triângulo"""
    TRIANGLE_TYPES = [
        ('ASCENDING', 'Ascending'),
        ('DESCENDING', 'Descending'),
        ('SYMMETRICAL', 'Symmetrical'),
    ]
    
    triangle_type = models.CharField(max_length=20, choices=TRIANGLE_TYPES)
