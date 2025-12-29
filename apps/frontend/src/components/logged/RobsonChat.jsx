import { useState, useRef, useEffect } from "react";
import PropTypes from "prop-types";
import axios from "axios";
import "./RobsonChat.css";

const API_BASE = import.meta.env.VITE_API_URL || "http://localhost:8000/api";

/**
 * RobsonChat - AI Trading Assistant Chat Component
 *
 * A floating chat interface for conversational AI trading assistance.
 * Uses Groq API for fast LLM inference.
 */
const RobsonChat = ({ initialOpen = false }) => {
  const [isOpen, setIsOpen] = useState(initialOpen);
  const [messages, setMessages] = useState([
    {
      id: "welcome",
      role: "assistant",
      content:
        "Hello! I am Robson, your trading assistant. How can I help you today? 🤖\n\nI can analyze the market, check your positions, calculate risks, or help execute trades.",
    },
  ]);
  const [inputValue, setInputValue] = useState("");
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState(null);

  const messagesEndRef = useRef(null);
  const inputRef = useRef(null);

  // Auto-scroll to bottom when new messages arrive
  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages]);

  // Focus input when chat opens
  useEffect(() => {
    if (isOpen) {
      inputRef.current?.focus();
    }
  }, [isOpen]);

  const getAuthToken = () => {
    return (
      localStorage.getItem("accessToken") ||
      sessionStorage.getItem("accessToken")
    );
  };

  const sendMessage = async () => {
    if (!inputValue.trim() || isLoading) return;

    const userMessage = {
      id: `user-${Date.now()}`,
      role: "user",
      content: inputValue.trim(),
    };

    setMessages((prev) => [...prev, userMessage]);
    setInputValue("");
    setIsLoading(true);
    setError(null);

    try {
      const token = getAuthToken();
      const response = await axios.post(
        `${API_BASE}/chat/`,
        { message: userMessage.content },
        {
          headers: {
            Authorization: `Bearer ${token}`,
            "Content-Type": "application/json",
          },
        },
      );

      const assistantMessage = {
        id: `assistant-${Date.now()}`,
        role: "assistant",
        content: response.data.message,
        intent: response.data.detected_intent,
        requiresConfirmation: response.data.requires_confirmation,
      };

      setMessages((prev) => [...prev, assistantMessage]);
    } catch (err) {
      console.error("Chat error:", err);
      setError(err.response?.data?.error || "Error sending message");

      const errorMessage = {
        id: `error-${Date.now()}`,
        role: "assistant",
        content: "❌ Sorry, an error occurred. Please try again.",
        isError: true,
      };
      setMessages((prev) => [...prev, errorMessage]);
    } finally {
      setIsLoading(false);
    }
  };

  const handleKeyPress = (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      sendMessage();
    }
  };

  const quickActions = [
    { label: "📈 Analyze BTC", message: "Analyze the current BTC situation" },
    { label: "💰 Balance", message: "What is my current balance?" },
    { label: "📊 Positions", message: "What are my open positions?" },
    { label: "⚠️ Risk", message: "What is my current risk level?" },
  ];

  const handleQuickAction = (message) => {
    setInputValue(message);
    inputRef.current?.focus();
  };

  const clearChat = () => {
    setMessages([
      {
        id: "welcome",
        role: "assistant",
        content: "Chat cleared! How can I help? 🤖",
      },
    ]);
  };

  return (
    <>
      {/* Floating Chat Button */}
      {!isOpen && (
        <button
          className="robson-chat-button"
          onClick={() => setIsOpen(true)}
          aria-label="Open chat with Robson"
        >
          <span className="chat-button-icon">🤖</span>
          <span className="chat-button-pulse" />
        </button>
      )}

      {/* Chat Panel */}
      {isOpen && (
        <div className="robson-chat-panel">
          {/* Header */}
          <div className="robson-chat-header">
            <div className="header-title">
              <span className="header-icon">🤖</span>
              <span className="header-text">Robson AI</span>
              <span className="header-status">● Online</span>
            </div>
            <div className="header-actions">
              <button
                className="header-btn"
                onClick={clearChat}
                title="Clear chat"
              >
                🗑️
              </button>
              <button
                className="header-btn"
                onClick={() => setIsOpen(false)}
                title="Minimize"
              >
                ✕
              </button>
            </div>
          </div>

          {/* Messages */}
          <div className="robson-chat-messages">
            {messages.map((msg) => (
              <div
                key={msg.id}
                className={`chat-message ${msg.role} ${msg.isError ? "error" : ""}`}
              >
                {msg.role === "assistant" && (
                  <span className="message-avatar">🤖</span>
                )}
                <div className="message-content">
                  <p>{msg.content}</p>
                  {msg.intent && (
                    <span className="message-intent">
                      Detected intent: {msg.intent}
                    </span>
                  )}
                </div>
                {msg.role === "user" && (
                  <span className="message-avatar">👤</span>
                )}
              </div>
            ))}

            {isLoading && (
              <div className="chat-message assistant loading">
                <span className="message-avatar">🤖</span>
                <div className="message-content">
                  <div className="typing-indicator">
                    <span></span>
                    <span></span>
                    <span></span>
                  </div>
                </div>
              </div>
            )}

            <div ref={messagesEndRef} />
          </div>

          {/* Quick Actions */}
          <div className="robson-chat-quick-actions">
            {quickActions.map((action, index) => (
              <button
                key={index}
                className="quick-action-btn"
                onClick={() => handleQuickAction(action.message)}
              >
                {action.label}
              </button>
            ))}
          </div>

          {/* Input */}
          <div className="robson-chat-input">
            {error && <div className="chat-error">{error}</div>}
            <div className="input-container">
              <input
                ref={inputRef}
                type="text"
                value={inputValue}
                onChange={(e) => setInputValue(e.target.value)}
                onKeyPress={handleKeyPress}
                placeholder="Type your message..."
                disabled={isLoading}
              />
              <button
                className="send-btn"
                onClick={sendMessage}
                disabled={isLoading || !inputValue.trim()}
              >
                {isLoading ? "⏳" : "➤"}
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
};

RobsonChat.propTypes = {
  initialOpen: PropTypes.bool,
};

export default RobsonChat;
