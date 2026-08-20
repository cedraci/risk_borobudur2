import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: { proxy: { "/api": "http://127.0.0.1:8787" } },
  // The UI half of the authorization contract — which denial markers get
  // rendered, and which nav links a grant set unlocks — is not reachable from
  // `cargo test`, so it is pinned here instead. `globals: true` is what lets
  // @testing-library/react register its automatic per-test cleanup.
  test: {
    environment: "jsdom",
    globals: true,
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
