ADR-0008: Adopt DeepSeek as Primary Local LLM

Status: Proposed
Date: 2025-12-07

Context
- AI-First Robson requires a local Large Language Model (LLM) for:
  - RAG-powered query answering over GitHub knowledge (PRs, issues, docs).
  - Trading strategy analysis and recommendations (future).
  - Code generation and refactoring assistance (future).
- Relying exclusively on external APIs (OpenAI, Anthropic) introduces:
  - **Latency**: Network round-trips increase response time.
  - **Cost**: Per-token pricing becomes expensive at scale.
  - **Privacy**: Sending sensitive trading data to third parties.
  - **Availability**: Dependency on external service uptime.
- Self-hosted LLM aligns with Robson's open-source, self-sovereign philosophy.
- Contabo VPS constraints:
  - **CPU-only** initially (GPU optional future upgrade).
  - **RAM**: 32GB available (after OS/k3s overhead).
  - **Disk**: SSD storage for model weights (~10-20GB per model).
- DeepSeek models (DeepSeek-R1, DeepSeek-V3) offer:
  - **Strong performance**: Competitive with GPT-4 on reasoning tasks.
  - **Efficient inference**: Optimized for CPU/low-resource environments.
  - **Open weights**: Self-hostable without API dependencies.
  - **Multilingual**: Supports English (primary) and other languages.

Needs Input
- Inference latency benchmarks for DeepSeek models on Contabo VPS (CPU-only).
- Memory usage for DeepSeek-R1-Distill-Qwen-1.5B vs larger variants.
- Acceptable response times for RAG queries (target: <2s for 95th percentile).

*(If CPU inference is too slow (>5s), defer self-hosting and use external API temporarily.)*

Decision
- **Adopt DeepSeek as the primary self-hosted LLM for Robson Bot**.
- **Gateway abstraction**: Implement `deepseek-gateway` service to:
  - Provide unified REST/gRPC API for LLM requests.
  - Handle model loading, prompt formatting, streaming responses.
  - Support fallback to external APIs (OpenAI, Anthropic) if self-hosted model is unavailable.
- **Model selection**:
  - Start with **DeepSeek-R1-Distill-Qwen-1.5B** (small, fast, CPU-friendly).
  - Upgrade to larger models (7B, 14B) as resources allow or GPU is added.
- **Deployment**:
  - Production: k3s Deployment with resource limits (CPU, memory).
  - Dev: Docker container in docker-compose for local testing.
- **Integration**:
  - RAG retriever calls `deepseek-gateway` for answer generation.
  - Gateway formats prompts with retrieved context + user query.
  - Streaming responses for real-time UX.

Consequences
- Positive
  - **Low latency**: Local inference eliminates network round-trips.
  - **Cost control**: No per-token charges; fixed infrastructure cost.
  - **Privacy**: Trading data and user queries stay on-premise.
  - **Reliability**: No dependency on external API uptime.
  - **Flexibility**: Full control over model selection, prompt engineering, fine-tuning.
  - **Open-source alignment**: DeepSeek weights are freely available.
- Negative/Trade-offs
  - **Operational complexity**: Model serving, updates, monitoring required.
  - **Resource consumption**: CPU/RAM dedicated to inference (competes with Django, ParadeDB).
  - **Inference speed**: CPU inference slower than cloud GPUs (acceptable for async queries).
  - **Model size limits**: Cannot run largest models (70B+) on VPS without GPU.
  - **Fallback dependency**: External API still needed for high-complexity queries or overload scenarios.

Alternatives
- **External API only (OpenAI GPT-4, Anthropic Claude)**
  - Pros: Zero ops, fastest inference (cloud GPUs), largest models.
  - Cons: High cost at scale, privacy concerns, network dependency, violates self-hosting principle.
  - Why not chosen: Conflicts with open-source philosophy; acceptable as fallback only.

- **Ollama + Llama 3 / Mistral**
  - Pros: Mature local LLM serving, wide model support.
  - Cons: Similar resource constraints; DeepSeek performs better on reasoning tasks.
  - Why not chosen: DeepSeek offers better performance/efficiency trade-off.

- **vLLM + Qwen / Yi models**
  - Pros: Fast inference server, good Asian language support.
  - Cons: vLLM optimized for GPUs (less benefit on CPU).
  - Why not chosen: DeepSeek models perform better on CPU.

Implementation Notes
- **Code paths**:
  - Gateway service: `apps/backend/deepseek-gateway/` (new Python service).
  - Protocol: REST API (`/v1/completions`, `/v1/chat/completions`) compatible with OpenAI spec.
  - Streaming: Server-Sent Events (SSE) for real-time token delivery.
  - Model loader: `transformers` library with DeepSeek weights from Hugging Face.

- **Infrastructure**:
  - Production: `infra/k8s/apps/deepseek-gateway/deployment.yaml` (new).
  - Production: `infra/k8s/apps/deepseek-gateway/service.yaml` (ClusterIP, internal only).
  - Dev: `docker-compose.yml` (new service: `deepseek-gateway`).
  - ArgoCD: New Application manifest.

- **Resource Limits**:
  - CPU: 4 cores (shared with other workloads).
  - Memory: 8GB (for 1.5B model; increase for larger models).
  - Disk: 10GB for model weights + cache.

- **Configuration**:
  - Environment variables: `DEEPSEEK_MODEL_NAME`, `DEEPSEEK_MAX_TOKENS`, `DEEPSEEK_TEMPERATURE`.
  - Fallback API: `OPENAI_API_KEY` (optional, for overload scenarios).

- **Testing**:
  - Unit tests: Mock gateway responses in RAG retriever tests.
  - Integration tests: Verify gateway can load model and respond to prompts.
  - Performance tests: Measure latency (p50, p95, p99) and throughput (tokens/sec).
  - Load tests: Ensure graceful degradation under concurrent requests.

- **Related**:
  - ADR-0009: RAG Architecture (consumes DeepSeek gateway).
  - Execution Plan: `docs/plan/03-deepseek-gateway-setup.prompt`.
  - Documentation: `docs/ai-first/DEEPSEEK_GATEWAY.md`.

- **Monitoring**:
  - Metrics: Request count, latency, token throughput, model load time.
  - Logs: Prompt inputs (sanitized), error traces, fallback triggers.
  - Alerts: High latency (>5s), OOM errors, model load failures.

References
- DeepSeek: https://www.deepseek.com
- DeepSeek Models (Hugging Face): https://huggingface.co/deepseek-ai
- Transformers library: https://huggingface.co/docs/transformers
