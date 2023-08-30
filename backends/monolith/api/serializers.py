from rest_framework.serializers import ModelSerializer
from .models import Strategy

class StrategySerializer(ModelSerializer):
    class Meta:
        model = Strategy
        fields = ['id', 'name']

