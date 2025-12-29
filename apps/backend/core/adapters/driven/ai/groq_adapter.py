"""
Groq AI adapter for chat functionality.

Uses the Groq API for fast LLM inference.
Groq offers free tier with generous rate limits.
"""

from __future__ import annotations

import logging
import os

from django.conf import settings

logger = logging.getLogger(__name__)


class GroqAdapter:
    """
    Adapter for Groq AI API.

    Groq provides fast inference for open-source models like Llama.
    Free tier includes:
    - llama-3.3-70b-versatile: 30 req/min, 6000 tokens/min
    - llama-3.1-8b-instant: 30 req/min, 20000 tokens/min
    - mixtral-8x7b-32768: 30 req/min, 5000 tokens/min

    We use llama-3.3-70b-versatile for best quality.
    """

    DEFAULT_MODEL = "llama-3.3-70b-versatile"
    FALLBACK_MODEL = "llama-3.1-8b-instant"

    def __init__(
        self,
        api_key: str | None = None,
        model: str | None = None,
    ):
        """
        Initialize Groq adapter.

        Args:
            api_key: Groq API key (defaults to GROQ_API_KEY env var)
            model: Model to use (defaults to llama-3.3-70b-versatile)
        """
        self.api_key = api_key or os.environ.get(
            "GROQ_API_KEY", getattr(settings, "GROQ_API_KEY", None)
        )

        if not self.api_key:
            raise ValueError("GROQ_API_KEY not found. Set it in environment or Django settings.")

        self.model = model or self.DEFAULT_MODEL
        self._client = None

        logger.info(f"GroqAdapter initialized with model: {self.model}")

    @property
    def client(self):
        """Lazy-load Groq client."""
        if self._client is None:
            try:
                from groq import Groq

                self._client = Groq(api_key=self.api_key)
            except ImportError:
                raise ImportError("groq package not installed. Run: pip install groq")
        return self._client

    def generate_response(
        self,
        messages: list[dict[str, str]],
        system_prompt: str,
        max_tokens: int = 1024,
        temperature: float = 0.7,
    ) -> str:
        """
        Generate a response from Groq.

        Args:
            messages: Conversation history
            system_prompt: System instructions
            max_tokens: Maximum response length
            temperature: Creativity (0-1)

        Returns:
            The AI's response text
        """
        # Prepare messages with system prompt
        full_messages = [{"role": "system", "content": system_prompt}] + messages

        try:
            logger.debug(f"Calling Groq API with {len(messages)} messages")

            response = self.client.chat.completions.create(
                model=self.model,
                messages=full_messages,
                max_tokens=max_tokens,
                temperature=temperature,
            )

            content = response.choices[0].message.content

            logger.debug(
                f"Groq response received: {len(content)} chars, "
                f"usage: {response.usage.total_tokens} tokens"
            )

            return content

        except Exception as e:
            logger.error(f"Groq API error: {e}")

            # Try fallback model
            if self.model != self.FALLBACK_MODEL:
                logger.warning(f"Trying fallback model: {self.FALLBACK_MODEL}")
                try:
                    response = self.client.chat.completions.create(
                        model=self.FALLBACK_MODEL,
                        messages=full_messages,
                        max_tokens=max_tokens,
                        temperature=temperature,
                    )
                    return response.choices[0].message.content
                except Exception as fallback_error:
                    logger.error(f"Fallback also failed: {fallback_error}")

            raise RuntimeError(f"Failed to generate response: {e}")

    def get_model_name(self) -> str:
        """Get the name of the model being used."""
        return self.model

    def list_available_models(self) -> list[str]:
        """List available models from Groq."""
        try:
            models = self.client.models.list()
            return [m.id for m in models.data]
        except Exception as e:
            logger.error(f"Failed to list models: {e}")
            return [self.DEFAULT_MODEL, self.FALLBACK_MODEL]
