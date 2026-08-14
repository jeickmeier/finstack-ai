import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./src",
  testMatch: "*.test.ts",
  fullyParallel: false,
  retries: 0,
  use: {
    baseURL: "http://127.0.0.1:4173",
    browserName: "chromium",
    headless: true,
  },
  webServer: {
    command: "node scripts/serve.mjs",
    url: "http://127.0.0.1:4173/harness.html",
    reuseExistingServer: !process.env.CI,
  },
});
