import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./e2e",
  timeout: 30_000,
  reporter: "line",
  use: {
    baseURL: process.env.MOVIEBOX_E2E_URL ?? "http://127.0.0.1:8420",
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
});
