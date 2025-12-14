# api/views.py 

from .services import BinanceService  # NOVA IMPORT

from binance.client import Client
# import time
import datetime
import json
import pandas as pd
from decimal import Decimal
from django.shortcuts import render
from django.http import JsonResponse
from decouple import config
from django.conf import settings
from rest_framework.response import Response
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework_simplejwt.serializers import TokenObtainPairSerializer
from rest_framework_simplejwt.views import TokenObtainPairView
from .serializers import StrategySerializer
from .models import Strategy
# from django_multitenant import views
# from django_multitenant.views import TenantModelViewSet
from clients.models import CustomUser

# Application layer imports (Hexagonal Architecture INSIDE Django)
from .application import Symbol as DomainSymbol, get_place_order_uc



client=Client(settings.BINANCE_API_KEY_TEST, settings.BINANCE_SECRET_KEY_TEST)

# class MyTokenObtainPairSerializer(TokenObtainPairSerializer):
#     @classmethod
#     def get_token(cls, user):
#         token = super().get_token(user)
#         token['username'] = user.username
#         return token

# class MyTokenObtainPairView(TokenObtainPairView):
#     serializer_class = MyTokenObtainPairSerializer


@api_view(['GET'])
@permission_classes([IsAuthenticated])
def getStrategies (request):
    try:
        tenant_id = request.user.client.id
    except:
        tenant_id = 0
    strategies = Strategy.objects.filter(client=tenant_id)
    serializer = StrategySerializer(strategies, many=True)
    return Response(serializer.data)
    # return JsonResponse({"name":"a"})
 
# def Ping(request):
#     result_ping = client.ping()
#     return JsonResponse({"pong": result_ping})

def Ping(request):
    """Refatorada para usar BinanceService"""
    try:
        service = BinanceService()
        result = service.ping()
        return JsonResponse({"pong": result})
    except Exception as e:
        return JsonResponse({"error": str(e)}, status=500)

# def ServerTime(request):
#     result_time = client.get_server_time()
#     return JsonResponse({"time": result_time})

def ServerTime(request):
    """Refatorada para usar BinanceService"""
    try:
        service = BinanceService()
        result = service.get_server_time()
        return JsonResponse({"time": result})
    except Exception as e:
        return JsonResponse({"error": str(e)}, status=500)

def SystemStatus(request):
    
    return JsonResponse({})

def ExchangeInfo(request):
    return JsonResponse({})

def SymbolInfo(request):
    return JsonResponse({})

def AllCoinInfo(request):
    return JsonResponse({})

def AccountSnapshot(request):
    return JsonResponse({})

def Products(request):
    return JsonResponse({})

# Spot Account Info Endpoints

def Info(request):
    return JsonResponse({})

def Balance(request):
    return JsonResponse({})

def Status(request):
    return JsonResponse({})

def ApiTradingStatus(request):
    return JsonResponse({})

def TradesFees(request):
    return JsonResponse({})

def AssetDetails(request):
    return JsonResponse({})

def DustLog(request):
    return JsonResponse({})

def TransferDust(request):
    return JsonResponse({})

def AssetDividendHistory(request):
    return JsonResponse({})

def EnableFastWithdrawSwitch(request):
    return JsonResponse({})

def DisableFastWithdrawSwitch(request):
    return JsonResponse({})

# Spot Account Orders Endpoints

def Orders(request):
    return JsonResponse({})

def PlaceOrder(request):
    """Place order via hexagonal use case. Input JSON:
    {"base":"BTC","quote":"USDT","side":"BUY","qty":0.1,"limit":50000}
    """
    if request.method != 'POST':
        return JsonResponse({"error": "Method not allowed"}, status=405)
    try:
        data = json.loads(request.body.decode()) if request.body else {}
        base = data.get("base")
        quote = data.get("quote")
        side = data.get("side")
        qty = Decimal(str(data.get("qty"))) if data.get("qty") is not None else None
        limit = Decimal(str(data.get("limit"))) if data.get("limit") is not None else None
        if not all([base, quote, side, qty is not None]):
            return JsonResponse({"error": "Missing required fields"}, status=400)

        symbol = DomainSymbol(base, quote) if DomainSymbol else f"{base}{quote}"
        uc = get_place_order_uc()
        placed = uc.execute(symbol, side, qty, limit)
        return JsonResponse({
            "id": placed["id"],
            "symbol": getattr(symbol, "as_pair", lambda: str(symbol))(),
            "side": side,
            "qty": str(qty),
            "price": str(placed["price"]),
        }, status=201)
    except Exception as e:
        return JsonResponse({"error": str(e)}, status=500)

def PlaceTestOrder(request):
    return JsonResponse({})

def OrderStatus(request):
    return JsonResponse({})

def CancelOrder(request):
    return JsonResponse({})

def OpenOrders(request):
    return JsonResponse({})

# Sub Account Orders Endpoints

def Accounts(request):
    return JsonResponse({})

def History(request):
    return JsonResponse({})

def Assets(request):
    return JsonResponse({})

# Margin Market Data

def CrossMarginAsset(request):
    return JsonResponse({})

def CrossMarginSymbol(request):
    return JsonResponse({})

def IsolatedMarginAsset(request):
    return JsonResponse({})

def IsolatedMarginSymbol(request):
    return JsonResponse({})

def MarginPriceIndex(request):
    return JsonResponse({})

# Margin Order

def MarginOrders(request):
    return JsonResponse({})

def MarginOrders(request):
    return JsonResponse({})

def MarginOrderStatus(request):
    return JsonResponse({})

def MarginOrderStatus(request):
    return JsonResponse({})

def OpenMarginOrders(request):
    return JsonResponse({})

# Margin Account

def MarginAccount(request):
    return JsonResponse({})

def CreateIsolatedMarginAccount(request):
    return JsonResponse({})

def IsolatedMarginAccount(request):
    return JsonResponse({})

def TransferSpotToCross(request):
    return JsonResponse({})

def TransferCrossToSpot(request):
    return JsonResponse({})

def TransferSpotToIsolated(request):
    return JsonResponse({})

def TransferIsolatedToSpot(request):
    return JsonResponse({})

def MaxMarginTransfer(request):
    return JsonResponse({})

# Margin Trades

def MarginTrades(request):
    return JsonResponse({})

# Margin Loans

def CreateMarginLoan(request):
    return JsonResponse({})

def RepayMarginLoan(request):
    return JsonResponse({})

def MarginLoanDetails(request):
    return JsonResponse({})

def MarginRepayDetails(request):
    return JsonResponse({})

def MaxMarginLoan(request):
    return JsonResponse({})

# Test Views

def Patrimony(request):
    # ~ balance = Balance(request)
    # ~ balance = balance.content.decode('utf-8')
    # ~ balance = json.loads(balance)
    # ~ print (balance)
    # ~ balance_spot = balance['balance']['spot']
    # ~ balance_isolated_margin = balance['balance']['isolated_margin']
    # ~ total = float(balance_spot) + float(balance_isolated_margin)
    result_patrimony = {"patrimony": 400}
    return JsonResponse(result_patrimony)

# def Balance(request):
    # ~ balance_spot = client.get_account()['balances']
    # ~ print (balance_spot)
    # ~ total = 0
    # ~ for x in balance_spot:
        # ~ if str(x['asset']) == 'USDT':
            # ~ asset_balance = float(x['free'])
        # ~ else:
            # ~ actual_price = ActualPrice(request, str(x['asset']))
            # ~ actual_price = actual_price.content.decode('utf-8')
            # ~ actual_price = json.loads(actual_price)
            # ~ print (actual_price)
            # ~ actual_price = float(actual_price['actual_price'])
            # ~ asset_balance = float(x['free']) * actual_price
        # ~ total = total + asset_balance
    # ~ balance_isolated_margin = client.get_isolated_margin_account()['totalNetAssetOfBtc']
    # ~ balance_isolated_margin = float(balance_isolated_margin)
    # ~ actual_price = ActualPrice(request)
    # ~ actual_price= json.loads(actual_price.content)['actual_price']
    # ~ balance_isolated_margin = round(balance_isolated_margin * actual_price, 2)
    # ~ result_balance = {
      # ~ "spot": round(total, 2),
      # ~ "isolated_margin": balance_isolated_margin
    # ~ }
    # ~ print(result_balance)
    # result_balance = {
      # "spot": 300,
      # "isolated_margin": 300
    # }
    # return JsonResponse(result_balance)

def Week(request):
    client=Client(settings.BINANCE_API_KEY_TEST, settings.BINANCE_SECRET_KEY_TEST)
    asset="BTCUSDT"
    today_date = datetime.date.today()
    last_week = today_date + datetime.timedelta(days=-7)
    year = today_date.year
    month = today_date.month
    day = today_date.day
    start=str(last_week.year)+"."+str(last_week.month)+"."+str(last_week.day)
    end=str(year)+"."+str(month)+"."+str(day)
    timeframe="1d"
    df= pd.DataFrame(client.get_historical_klines(asset, timeframe, start, end))
    df=df.iloc[:,:6]
    df.columns=["Date", "Open", "High", "Low", "Close", "Volume"]
    df=df.set_index("Date")
    df['Date'] = df.index
    df.index=pd.to_datetime(df.index,unit="ms")
    df=df.astype("float")
    return JsonResponse({"last_week": df.to_json(orient='records', index=True)})

def Chart(request):
    # ~ client=Client(API_KEY, SECRET_KEY)
    # ~ asset="BTCUSDT"
    # ~ timeframe="4h"
    # ~ df= pd.DataFrame(client.get_historical_klines(asset, timeframe))
    return JsonResponse({})
