# api/models/analysis.py

from django.db import models
from .base import BaseTechnicalModel
from .trading import Strategy

class TechnicalAnalysisInterpretation(BaseTechnicalModel):
    """Interpretações de análise técnica"""
    name = models.CharField(max_length=255)
    experience = models.IntegerField(help_text="Nível de experiência necessário")

class TechnicalEvent(BaseTechnicalModel):
    """Eventos técnicos identificados"""
    interpretation = models.ForeignKey(
        TechnicalAnalysisInterpretation, 
        on_delete=models.CASCADE
    )
    strategy = models.ForeignKey(Strategy, on_delete=models.CASCADE)
    confidence = models.DecimalField(
        max_digits=5, 
        decimal_places=2, 
        help_text="Nível de confiança (0-100)"
    )


