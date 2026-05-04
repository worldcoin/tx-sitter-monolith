# tx-sitter admin UI

Simple Vite + React + TypeScript admin frontend for tx-sitter.

## Prerequisites

- Node.js 20+
- npm
- A running tx-sitter backend with the admin endpoints enabled
- HTTP Basic auth credentials for the admin API. Local/dev default is usually `admin:admin`.

## Run locally

```bash
cd admin-ui
VITE_API_BASE_URL=http://localhost:3000 npm run dev
```

Open the Vite URL printed by the command and enter the admin Basic auth credentials in the header.

## Build for static hosting

```bash
cd admin-ui
VITE_API_BASE_URL=https://your-tx-sitter.example.com npm run build
```

The static build is emitted to `admin-ui/dist/`.

## Notes

- The backend URL is read from `VITE_API_BASE_URL` and defaults to `http://localhost:3000`.
- Admin API calls use HTTP Basic auth.
- Relayer balances are fetched best-effort by calling the network `httpRpc` URL returned by `GET /1/admin/networks`. If that RPC does not allow browser CORS, the balance column shows `unavailable`.
- The current admin transaction response does not expose `from` or `createdAt`; the UI marks those fields as `not exposed` rather than inventing values.
