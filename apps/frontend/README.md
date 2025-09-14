# robson-frontend

React (Vite) app organized with Ports & Adapters on the client side.

- `src/domain`: pure types and logic
- `src/ports`: interfaces for HTTP/WS/storage
- `src/adapters`: implementations (fetch/WebSocket/localStorage)
- `src/application`: client-side use cases/state orchestration

The previous frontend at `frontends/web/` will be migrated here.

Environment variables
- `VITE_API_BASE_URL`: Base URL for backend REST API (e.g., http://127.0.0.1:8000)
- `VITE_WS_URL`: Base URL for WebSocket gateway (e.g., ws://127.0.0.1:8000/ws)
- `VITE_WS_URL_BINANCE`: public Binance WS override (optional)

See `.env.example` and copy to `.env.local` for development.

Migration note
- Deprecated `REACT_APP_BACKEND_URL` from the old app is replaced by `VITE_API_BASE_URL`.
- If you had `frontends/web/.env.development`, map its values into `.env.local` here.
