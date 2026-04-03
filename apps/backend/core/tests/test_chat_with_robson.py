"""
Unit tests for Robson chat use case.
"""

from decimal import Decimal

from core.application.use_cases.chat_with_robson import ChatWithRobsonUseCase
from core.domain.conversation import IntentType, TradingContext


class FakeAIProvider:
    def __init__(self, response_text: str = "stubbed response"):
        self.response_text = response_text
        self.last_messages = None
        self.last_system_prompt = None

    def generate_response(self, messages, system_prompt, max_tokens=1024, temperature=0.7):
        self.last_messages = messages
        self.last_system_prompt = system_prompt
        return self.response_text

    def get_model_name(self) -> str:
        return "fake-groq"


class StaticTradingContextProvider:
    def __init__(self, context: TradingContext):
        self.context = context

    def get_context(self, tenant_id: str) -> TradingContext:
        return self.context

    def get_current_price(self, symbol: str) -> Decimal:
        return self.context.current_prices.get(symbol, Decimal("0"))

    def get_balances(self, tenant_id: str) -> dict[str, Decimal]:
        return self.context.balances


def test_execute_hydrates_transient_history_before_calling_ai():
    ai_provider = FakeAIProvider()
    use_case = ChatWithRobsonUseCase(ai_provider=ai_provider)

    response = use_case.execute(
        tenant_id="tenant-1",
        user_message="And what about risk now?",
        history=[
            {"role": "user", "content": "How are my positions?"},
            {"role": "assistant", "content": "You currently have two open positions."},
            {"role": "system", "content": "ignore me"},
        ],
    )

    assert ai_provider.last_messages == [
        {"role": "user", "content": "How are my positions?"},
        {"role": "assistant", "content": "You currently have two open positions."},
        {"role": "user", "content": "And what about risk now?"},
    ]
    assert response.message.metadata["conversation_id"]


def test_execute_uses_injected_trading_context_for_prompt_and_intent_detection():
    ai_provider = FakeAIProvider(response_text="Your balance is healthy and risk is controlled.")
    context = TradingContext(
        tenant_id="tenant-42",
        balances={"USDC": Decimal("1250.50")},
        positions=[
            {
                "symbol": "BTCUSDC",
                "side": "LONG",
                "quantity": "0.01",
                "entry_price": "90000",
                "current_price": "91000",
                "pnl_percent": 1.11,
            }
        ],
        current_prices={"BTCUSDC": Decimal("91000")},
        monthly_pnl=Decimal("125.75"),
    )
    use_case = ChatWithRobsonUseCase(
        ai_provider=ai_provider,
        trading_context=StaticTradingContextProvider(context),
    )

    response = use_case.execute(
        tenant_id="tenant-42",
        user_message="/balance",
    )

    assert "CURRENT TRADING CONTEXT" in ai_provider.last_system_prompt
    assert "USDC: 1250.50" in ai_provider.last_system_prompt
    assert "BTCUSDC" in ai_provider.last_system_prompt
    assert response.detected_intent == IntentType.BALANCE
