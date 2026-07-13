import { defineConfig } from "vite";

export default defineConfig({
  server: {
    port: 5173,
    proxy: {
      "/api": "http://localhost:3000",
      // Keep /auth/callback in the Vite SPA; only the OAuth start endpoint
      // belongs to the backend.
      "/auth/github": "http://localhost:3000",
    },
  },
});
