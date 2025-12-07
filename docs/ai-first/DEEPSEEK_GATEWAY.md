# DeepSeek Gateway Protocol

**Last Updated**: 2025-12-07
**Related ADR**: ADR-0008
**Status**: Proposed

---

## Overview

The DeepSeek Gateway is a self-hosted LLM inference service providing OpenAI-compatible API endpoints.

---

## API Endpoints

### POST /v1/completions

Generate text completion from a prompt.

**Request**:
```json
{
  "model": "deepseek-r1-distill-qwen-1.5b",
  "prompt": "Explain hexagonal architecture:",
  "max_tokens": 512,
  "temperature": 0.7
}
```

**Response**:
```json
{
  "id": "cmpl-8a3d2f1e",
  "object": "text_completion",
  "choices": [{
    "text": "Hexagonal architecture separates...",
    "finish_reason": "stop"
  }],
  "usage": {
    "prompt_tokens": 5,
    "completion_tokens": 85,
    "total_tokens": 90
  }
}
```

---

### POST /v1/embeddings

Generate dense vector embeddings for text.

**Request**:
```json
{
  "model": "deepseek-r1-distill-qwen-1.5b",
  "input": "Hexagonal architecture separates domain from infrastructure."
}
```

**Response**:
```json
{
  "object": "list",
  "data": [{
    "object": "embedding",
    "index": 0,
    "embedding": [0.123, -0.456, 0.789, ...]
  }],
  "model": "deepseek-r1-distill-qwen-1.5b",
  "usage": {
    "prompt_tokens": 10,
    "total_tokens": 10
  }
}
```

---

### GET /health

Health check endpoint for Kubernetes probes.

**Response**:
```json
{
  "status": "healthy",
  "model_loaded": true,
  "model_name": "deepseek-r1-distill-qwen-1.5b"
}
```

---

## Configuration

**Environment Variables**:
- `DEEPSEEK_MODEL_NAME`: Model ID (default: deepseek-r1-distill-qwen-1.5b)
- `DEEPSEEK_MAX_TOKENS`: Default max tokens (default: 512)
- `DEEPSEEK_TEMPERATURE`: Default temperature (default: 0.7)
- `DEEPSEEK_DEVICE`: Inference device (cpu or cuda)

---

**Maintainers**: Robson Bot Core Team
**License**: Same as project
