import { useContext, useEffect, useMemo, useRef, useState } from "react";
import AuthContext from "../../context/AuthContext";
import {
  getRobsonChatContext,
  getRobsonChatStatus,
  sendRobsonChatMessage,
} from "../../services/robsonChat";
import "./RobsonCommandDock.css";

const COMMAND_ALIASES = {
  "/balance":
    "Resuma meu saldo atual, capital disponível e qualquer restrição relevante da conta.",
  "/positions":
    "Resuma minhas posições abertas agora com lado, tamanho, P&L, risco e o que merece atenção imediata.",
  "/risk":
    "Me dê um snapshot de risco atual com posições abertas, margin health, risco por posição e P&L mensal.",
  "/btc":
    "Analise o contexto atual de BTC usando meu trading context. Seja objetivo e prático.",
  "/thesis":
    "Quero estruturar uma trading thesis com contexto, racional, gatilho e invalidação.",
  "/context": "Resuma meu trading context atual e o que ele sugere como foco agora.",
};

const LOCAL_COMMANDS = new Set(["/help", "/clear", "/refresh"]);

const buildMessage = ({
  role,
  content,
  intent = null,
  requiresConfirmation = false,
  localOnly = false,
  error = false,
  model = null,
}) => ({
  id: `${role}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
  role,
  content,
  intent,
  requiresConfirmation,
  localOnly,
  error,
  model,
});

const buildWelcomeMessage = () =>
  buildMessage({
    role: "assistant",
    content:
      "Robson online. Digite normalmente ou use /help, /balance, /positions, /risk, /btc, /thesis, /refresh ou /clear.",
    localOnly: true,
  });

const loadSessionState = (storageKey) => {
  try {
    const raw = sessionStorage.getItem(storageKey);
    if (!raw) {
      return null;
    }
    return JSON.parse(raw);
  } catch {
    return null;
  }
};

const sanitizeMessages = (messages) => {
  if (!Array.isArray(messages)) {
    return [buildWelcomeMessage()];
  }

  const sanitized = messages
    .filter((message) => message && typeof message.content === "string")
    .slice(-40)
    .map((message) => ({
      id: message.id || `message-${Math.random().toString(36).slice(2, 8)}`,
      role: message.role === "user" ? "user" : "assistant",
      content: message.content,
      intent: message.intent || null,
      requiresConfirmation: Boolean(message.requiresConfirmation),
      localOnly: Boolean(message.localOnly),
      error: Boolean(message.error),
      model: message.model || null,
    }));

  return sanitized.length > 0 ? sanitized : [buildWelcomeMessage()];
};

const formatCurrencyLike = (value) => {
  const numeric = Number(value);
  if (Number.isNaN(numeric)) {
    return value || "0";
  }
  return `${numeric >= 0 ? "+" : ""}${numeric.toFixed(2)}`;
};

const summarizeBalances = (balances) => {
  if (!balances || typeof balances !== "object") {
    return null;
  }

  return Object.entries(balances)
    .slice(0, 3)
    .map(([asset, amount]) => `${asset}:${amount}`)
    .join(" · ");
};

const buildHistoryPayload = (messages) =>
  messages
    .filter((message) => !message.localOnly)
    .slice(-12)
    .map((message) => ({
      role: message.role,
      content: message.content,
    }));

function RobsonCommandDock() {
  const { authTokens, user } = useContext(AuthContext);
  const baseUrl = import.meta.env.VITE_API_BASE_URL;
  const token = authTokens?.access;
  const storageKey = useMemo(
    () => `robson-command-dock:${user?.user_id || user?.username || "session"}`,
    [user?.user_id, user?.username]
  );

  const [messages, setMessages] = useState([buildWelcomeMessage()]);
  const [inputValue, setInputValue] = useState("");
  const [conversationId, setConversationId] = useState(null);
  const [serviceStatus, setServiceStatus] = useState({
    available: false,
    provider: "Groq",
    model: null,
  });
  const [contextData, setContextData] = useState(null);
  const [isSending, setIsSending] = useState(false);
  const [error, setError] = useState(null);

  const inputRef = useRef(null);
  const feedEndRef = useRef(null);

  const refreshRuntimeState = async ({ silent = false } = {}) => {
    if (!baseUrl || !token) {
      return;
    }

    try {
      const [statusData, context] = await Promise.all([
        getRobsonChatStatus({ baseUrl, token }),
        getRobsonChatContext({ baseUrl, token }),
      ]);

      setServiceStatus(statusData);
      setContextData(context);

      if (!silent) {
        setMessages((previous) => [
          ...previous,
          buildMessage({
            role: "assistant",
            content: `Contexto sincronizado. ${context.positions?.length || 0} posições abertas e P&L mensal ${formatCurrencyLike(context.monthly_pnl)}.`,
            localOnly: true,
          }),
        ]);
      }
    } catch (runtimeError) {
      setServiceStatus((previous) => ({
        ...previous,
        available: false,
      }));
      if (!silent) {
        setMessages((previous) => [
          ...previous,
          buildMessage({
            role: "assistant",
            content: `Falha ao atualizar contexto: ${runtimeError.message}`,
            localOnly: true,
            error: true,
          }),
        ]);
      }
    }
  };

  useEffect(() => {
    const sessionState = loadSessionState(storageKey);
    if (!sessionState) {
      return;
    }

    setMessages(sanitizeMessages(sessionState.messages));
    setConversationId(sessionState.conversationId || null);
  }, [storageKey]);

  useEffect(() => {
    sessionStorage.setItem(
      storageKey,
      JSON.stringify({
        messages: messages.slice(-40),
        conversationId,
      })
    );
  }, [messages, conversationId, storageKey]);

  useEffect(() => {
    refreshRuntimeState({ silent: true });
  }, [baseUrl, token]);

  useEffect(() => {
    feedEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages, isSending]);

  useEffect(() => {
    const onKeyDown = (event) => {
      const target = event.target;
      const isInteractiveTarget =
        target instanceof HTMLElement &&
        (target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.tagName === "SELECT" ||
          target.isContentEditable);

      if (!isInteractiveTarget && event.key === "/") {
        event.preventDefault();
        inputRef.current?.focus();
        setInputValue((previous) => previous || "/");
      }
    };

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  const handleLocalCommand = async (command) => {
    if (command === "/help") {
      setMessages((previous) => [
        ...previous,
        buildMessage({
          role: "assistant",
          content:
            "Comandos disponíveis: /balance, /positions, /risk, /btc, /thesis, /context, /refresh e /clear. Você também pode conversar naturalmente com o Robson.",
          localOnly: true,
        }),
      ]);
      return true;
    }

    if (command === "/clear") {
      setMessages([buildWelcomeMessage()]);
      setConversationId(null);
      setError(null);
      return true;
    }

    if (command === "/refresh") {
      await refreshRuntimeState();
      return true;
    }

    return false;
  };

  const handleSend = async () => {
    const rawInput = inputValue.trim();
    if (!rawInput || isSending || !token || !baseUrl) {
      return;
    }

    setInputValue("");
    setError(null);

    const command = rawInput.split(" ")[0].toLowerCase();
    if (LOCAL_COMMANDS.has(command)) {
      const handled = await handleLocalCommand(command);
      if (handled) {
        return;
      }
    }

    const outgoingText = COMMAND_ALIASES[command] || rawInput;
    const userMessage = buildMessage({ role: "user", content: rawInput });
    const historyPayload = buildHistoryPayload(messages);

    setMessages((previous) => [...previous, userMessage]);
    setIsSending(true);

    try {
      const response = await sendRobsonChatMessage({
        baseUrl,
        token,
        message: outgoingText,
        history: historyPayload,
        conversationId,
      });

      setConversationId(response.conversation_id || conversationId);
      setMessages((previous) => [
        ...previous,
        buildMessage({
          role: "assistant",
          content: response.message,
          intent: response.detected_intent,
          requiresConfirmation: response.requires_confirmation,
          model: response.model,
        }),
      ]);

      if (response.context_summary) {
        setContextData((previous) => ({
          ...(previous || {}),
          monthly_pnl: response.context_summary.monthly_pnl,
          risk_metrics: {
            ...(previous?.risk_metrics || {}),
            lowest_margin_level: response.context_summary.lowest_margin_level,
            open_positions_count: response.context_summary.open_positions_count,
          },
        }));
      }
    } catch (sendError) {
      setError(sendError.message);
      setMessages((previous) => [
        ...previous,
        buildMessage({
          role: "assistant",
          content: `Falha ao falar com o Robson: ${sendError.message}`,
          localOnly: true,
          error: true,
        }),
      ]);
    } finally {
      setIsSending(false);
    }
  };

  const handleKeyDown = (event) => {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      handleSend();
    }
  };

  const isMonthlyPnlNegative = Number(contextData?.monthly_pnl || 0) < 0;
  const lowestMarginLevel = contextData?.risk_metrics?.lowest_margin_level;
  const hasCriticalPositions =
    Array.isArray(contextData?.risk_metrics?.critical_positions) &&
    contextData.risk_metrics.critical_positions.length > 0;

  return (
    <div className="robson-command-dock" role="complementary" aria-label="Robson command dock">
      <div className="robson-command-dock__shell">
        <div className="robson-command-dock__topline">
          <div className="robson-command-dock__brand">
            <span
              className={`robson-command-dock__indicator ${
                serviceStatus.available ? "is-online" : "is-offline"
              }`}
            />
            <span>ROBSON // agent</span>
            <span>{serviceStatus.provider || "Groq"}</span>
            <span>{serviceStatus.model || "model pending"}</span>
          </div>
          <div className="robson-command-dock__commands">
            <span>/help</span>
            <span>/balance</span>
            <span>/positions</span>
            <span>/risk</span>
            <span>/btc</span>
            <span>/thesis</span>
            <span>/refresh</span>
            <span>/clear</span>
          </div>
        </div>

        <div className="robson-command-dock__contextline">
          <span className="robson-command-dock__chip">
            posições {contextData?.positions?.length || contextData?.risk_metrics?.open_positions_count || 0}
          </span>
          <span
            className={`robson-command-dock__chip ${
              isMonthlyPnlNegative ? "is-negative" : "is-positive"
            }`}
          >
            pnl mês {formatCurrencyLike(contextData?.monthly_pnl || "0")}
          </span>
          {lowestMarginLevel && (
            <span
              className={`robson-command-dock__chip ${
                Number(lowestMarginLevel) < 1.3 ? "is-warning" : ""
              }`}
            >
              menor margin {lowestMarginLevel}
            </span>
          )}
          {summarizeBalances(contextData?.balances) && (
            <span className="robson-command-dock__chip">
              {summarizeBalances(contextData?.balances)}
            </span>
          )}
          {hasCriticalPositions && (
            <span className="robson-command-dock__chip is-warning">
              atenção em {contextData.risk_metrics.critical_positions.join(", ")}
            </span>
          )}
        </div>

        <div className="robson-command-dock__feed">
          {messages.slice(-10).map((message) => (
            <div
              key={message.id}
              className={`robson-command-dock__message ${
                message.role === "user" ? "is-user" : ""
              } ${message.error ? "is-error" : ""}`}
            >
              <div className="robson-command-dock__message-role">
                {message.role === "user" ? "you" : "robson"}
              </div>
              <div className="robson-command-dock__message-content">
                {message.content}
                {(message.intent || message.requiresConfirmation || message.model) && (
                  <div className="robson-command-dock__message-meta">
                    {message.intent && <span>intent {message.intent}</span>}
                    {message.requiresConfirmation && <span>needs confirmation</span>}
                    {message.model && <span>{message.model}</span>}
                  </div>
                )}
              </div>
            </div>
          ))}
          {isSending && (
            <div className="robson-command-dock__message">
              <div className="robson-command-dock__message-role">robson</div>
              <div className="robson-command-dock__message-content">
                pensando com contexto de trading...
              </div>
            </div>
          )}
          <div ref={feedEndRef} />
        </div>

        <div className="robson-command-dock__promptline">
          <div className="robson-command-dock__prompt">
            <label className="robson-command-dock__prompt-label" htmlFor="robson-command-input">
              robson&gt;
            </label>
            <input
              id="robson-command-input"
              ref={inputRef}
              className="robson-command-dock__input"
              type="text"
              value={inputValue}
              onChange={(event) => setInputValue(event.target.value)}
              onKeyDown={handleKeyDown}
              placeholder="Converse com o Robson ou use /command"
              autoComplete="off"
              spellCheck={false}
              disabled={!token || !baseUrl || isSending}
            />
            <button
              type="button"
              className="robson-command-dock__send"
              onClick={handleSend}
              disabled={!inputValue.trim() || !token || !baseUrl || isSending}
            >
              {isSending ? "..." : "send"}
            </button>
          </div>
          <div className="robson-command-dock__hint">
            Enter envia. Digite <code>/</code> de qualquer tela privada para focar o prompt.
            {error && ` Último erro: ${error}`}
          </div>
        </div>
      </div>
    </div>
  );
}

export default RobsonCommandDock;
