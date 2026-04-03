const parseJsonResponse = async (response) => {
  const contentType = response.headers.get("content-type") || "";
  if (!contentType.includes("application/json")) {
    return {};
  }
  return response.json().catch(() => ({}));
};

const buildHeaders = (token) => ({
  "Content-Type": "application/json",
  Authorization: `Bearer ${token}`,
});

const assertRequired = (name, value) => {
  if (!value) {
    throw new Error(`${name} is required`);
  }
};

export const getRobsonChatStatus = async ({ baseUrl, token }) => {
  assertRequired("baseUrl", baseUrl);
  assertRequired("token", token);

  const response = await fetch(`${baseUrl}/api/chat/status/`, {
    method: "GET",
    headers: buildHeaders(token),
  });

  const data = await parseJsonResponse(response);
  if (!response.ok) {
    throw new Error(data.error || `Failed to fetch chat status (${response.status})`);
  }

  return data;
};

export const getRobsonChatContext = async ({ baseUrl, token }) => {
  assertRequired("baseUrl", baseUrl);
  assertRequired("token", token);

  const response = await fetch(`${baseUrl}/api/chat/context/`, {
    method: "GET",
    headers: buildHeaders(token),
  });

  const data = await parseJsonResponse(response);
  if (!response.ok) {
    throw new Error(data.error || `Failed to fetch chat context (${response.status})`);
  }

  return data.context || {};
};

export const sendRobsonChatMessage = async ({
  baseUrl,
  token,
  message,
  history = [],
  conversationId,
}) => {
  assertRequired("baseUrl", baseUrl);
  assertRequired("token", token);
  assertRequired("message", message);

  const response = await fetch(`${baseUrl}/api/chat/`, {
    method: "POST",
    headers: buildHeaders(token),
    body: JSON.stringify({
      message,
      history,
      conversation_id: conversationId || null,
    }),
  });

  const data = await parseJsonResponse(response);
  if (!response.ok) {
    throw new Error(data.error || `Failed to send chat message (${response.status})`);
  }

  return data;
};
