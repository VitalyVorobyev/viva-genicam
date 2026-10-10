import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5183,
    strictPort: true,
  },
  test: {
    // Pure functions (`*.test.ts`) run in node; components and hooks
    // (`*.test.tsx`) get a DOM and the Tauri IPC mock reset in setup-dom.ts.
    projects: [
      {
        extends: true,
        test: {
          name: "unit",
          include: ["src/**/*.test.ts"],
          environment: "node",
          // Vitest stubs CSS out; the contrast test reads the tokens' text.
          css: { include: [/tokens\.css/] },
        },
      },
      {
        extends: true,
        test: {
          name: "dom",
          include: ["src/**/*.test.tsx"],
          environment: "jsdom",
          setupFiles: ["./src/test/setup-dom.ts"],
        },
      },
    ],
  },
});
